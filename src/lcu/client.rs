//! HTTP-клиент LCU: только GET-запросы к 127.0.0.1.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::ACCEPT;
use serde::de::DeserializeOwned;
use serde_json::json;
use tracing::{debug, warn};

use super::lockfile::Lockfile;
use super::models::{
    ChatParticipant, ChatParticipants, LcuChampSelectSession, LcuChampionSummary, LcuLobby,
    LcuGame, LcuGameflowSession, LcuInventoryItem, LcuMatchHistory, LcuOwnedChampion, LcuRankedStats, LcuRegionLocale, LcuSummoner,
};
use super::riot_client::RiotClientAuth;
use crate::error::{AppError, AppResult};
use crate::riot::client::encode_segment;
use crate::riot::models::FETCH_MATCHES;
use crate::riot::{AccountSnapshot, DataSource, Inventory, MatchSummary, RankedQueues, Region, RiotId};
use crate::riot_login::{self, AuthStep};

/// Что сейчас происходит с клиентом League.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LcuStatus {
    NotRunning,
    /// Запущен, но ещё загружается или вход не выполнен.
    Loading,
    Connected {
        region: Region,
        riot_id: RiotId,
        /// PUUID в формате клиента (не Riot API).
        puuid: String,
    },
}

/// Клиент LCU. Клонирование дешёвое (внутри указатели), как у `RiotClient`.
#[derive(Clone)]
pub struct LcuClient {
    http: reqwest::Client,
    /// Где искать lockfile (см. `lockfile::candidate_dirs`). Список меняется, когда
    /// пользователь указывает папку игры в настройках, поэтому он под `RwLock`
    /// (общий для всех клонов). Внутри `Arc<[PathBuf]>`: чтобы прочитать список,
    /// копируем только указатель и сразу отпускаем замок.
    dirs: Arc<RwLock<Arc<[PathBuf]>>>,
}

impl LcuClient {
    pub fn new(dirs: Vec<PathBuf>) -> AppResult<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("lol-account-manager/", env!("CARGO_PKG_VERSION")))
            // Сертификат клиента League выпущен собственным центром сертификации Riot,
            // и стандартную проверку он не проходит (нет имени 127.0.0.1 в нужном поле).
            // Поэтому проверку отключаем — но ТОЛЬКО в этом клиенте, который ходит
            // исключительно на 127.0.0.1 (адрес собираем сами, см. get_json).
            // Подменить сервер на 127.0.0.1 может лишь программа на этом же компьютере,
            // а она и так может прочитать lockfile с паролем.
            .tls_danger_accept_invalid_certs(true)
            // Никаких прокси (иначе пароль из lockfile мог бы уйти на прокси-сервер)
            // и никаких перенаправлений на другие адреса.
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            dirs: Arc::new(RwLock::new(dirs.into())),
        })
    }

    /// Новый список папок (пользователь сменил папку игры). Видно всем клонам.
    pub fn set_dirs(&self, dirs: Vec<PathBuf>) {
        *self.dirs.write().unwrap_or_else(PoisonError::into_inner) = dirs.into();
    }

    fn current_dirs(&self) -> Arc<[PathBuf]> {
        Arc::clone(&self.dirs.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Порт и пароль локального API Riot Client — из его собственного lockfile
    /// (`%LOCALAPPDATA%\Riot Games\Riot Client\Config\lockfile`). `None` — он не запущен.
    async fn riot_lockfile(&self) -> AppResult<Option<Lockfile>> {
        let Some(dir) = dirs::data_local_dir() else {
            return Ok(None);
        };
        Lockfile::find(&[dir.join("Riot Games").join("Riot Client").join("Config")]).await
    }

    /// Выполнен ли вход в Riot Client. `None` — он не запущен или ещё не отвечает.
    pub async fn riot_session(&self) -> Option<RiotSession> {
        let lock = self.riot_lockfile().await.ok()??;
        let url = format!("https://127.0.0.1:{}/rso-auth/v1/session", lock.port);
        let response = self.get(&url, lock.password()).await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let session: RiotSessionDto = response.json().await.ok()?;
        Some(RiotSession::from_type(&session.r#type))
    }

    /// Выйти из аккаунта в Riot Client (то же, что «Выйти» в его окне).
    pub async fn riot_sign_out(&self) -> AppResult<()> {
        let lock = self.riot_lockfile().await?.ok_or(AppError::LcuNotRunning)?;
        let url = format!("https://127.0.0.1:{}/rso-auth/v1/session", lock.port);
        self.http
            .delete(&url)
            .basic_auth("riot", Some(lock.password()))
            .send()
            .await
            .map_err(lcu_error)?;
        Ok(())
    }

    /// POST к локальному API Riot Client с телом JSON; возвращает тело ответа.
    async fn riot_post(&self, path: &str, body: &serde_json::Value) -> AppResult<Vec<u8>> {
        let lock = self.riot_lockfile().await?.ok_or(AppError::LcuNotRunning)?;
        let url = format!("https://127.0.0.1:{}{path}", lock.port);
        let response = self
            .http
            .post(&url)
            .basic_auth("riot", Some(lock.password()))
            .json(body)
            .send()
            .await
            .map_err(lcu_error)?;
        Ok(response.bytes().await.map_err(lcu_error)?.to_vec())
    }

    /// Начать вход: ответ скажет, нужна ли капча (см. [`crate::riot_login`]).
    pub async fn riot_auth_start(&self, product_id: &str, intended: &str, language: &str) -> AppResult<AuthStep> {
        let body = json!({"productId": product_id, "intendedProductId": intended, "language": language});
        let bytes = self.riot_post("/rso-authenticator/v1/authentication/riot-identity/start", &body).await?;
        Ok(riot_login::parse(&bytes))
    }

    /// Отправить логин, пароль и токен капчи. `captcha` пустой — если капча не нужна.
    pub async fn riot_auth_complete(&self, username: &str, password: &str, captcha: &str, language: &str) -> AppResult<AuthStep> {
        let body = json!({
            "username": username,
            "password": password,
            "captcha": captcha,
            "remember": true,
            "language": language,
        });
        let bytes = self.riot_post("/rso-authenticator/v1/authentication/riot-identity/complete", &body).await?;
        Ok(riot_login::parse(&bytes))
    }

    /// Отправить код подтверждения (двухфакторная защита).
    pub async fn riot_auth_multifactor(&self, otp: &str) -> AppResult<AuthStep> {
        let body = json!({"multifactor": {"otp": otp, "rememberDevice": true}});
        let bytes = self.riot_post("/rso-authenticator/v1/authentication/multifactor", &body).await?;
        Ok(riot_login::parse(&bytes))
    }

    /// Состояние клиента для индикатора в окне. Ошибки сводятся к трём состояниям.
    pub async fn status(&self) -> LcuStatus {
        let region = match self.region().await {
            Ok(region) => region,
            Err(AppError::LcuNotRunning) => return LcuStatus::NotRunning,
            Err(_) => return LcuStatus::Loading,
        };
        match self.current_summoner().await {
            Ok(me) if !me.game_name.is_empty() => LcuStatus::Connected {
                region,
                riot_id: RiotId {
                    game_name: me.game_name,
                    tag_line: me.tag_line,
                },
                puuid: me.puuid,
            },
            Err(AppError::LcuNotRunning) => LcuStatus::NotRunning,
            _ => LcuStatus::Loading,
        }
    }

    /// Сервер, к которому подключён клиент.
    pub async fn region(&self) -> AppResult<Region> {
        let locale: LcuRegionLocale = self
            .get_json("/riotclient/region-locale")
            .await?
            .ok_or(AppError::LcuNotReady)?;
        Region::from_lcu(&locale.region).ok_or(AppError::UnknownRegion(locale.region))
    }

    /// Аккаунт, в который выполнен вход.
    pub async fn current_summoner(&self) -> AppResult<LcuSummoner> {
        self.get_json("/lol-summoner/v1/current-summoner")
            .await?
            .ok_or(AppError::LcuNotLoggedIn)
    }

    /// Поиск игрока по Riot ID (на сервере клиента).
    pub async fn summoner_by_riot_id(&self, id: &RiotId) -> AppResult<LcuSummoner> {
        // Параметр запроса кодируем сами: «Имя#TAG» → «%D0%98%D0%BC%D1%8F%23TAG».
        let path = format!("/lol-summoner/v1/summoners?name={}", encode_segment(&id.to_string()));
        self.get_json(&path).await?.ok_or(AppError::AccountNotFound)
    }

    /// Поиск по PUUID клиента — находит игрока, даже если он сменил Riot ID.
    pub async fn summoner_by_puuid(&self, puuid: &str) -> AppResult<LcuSummoner> {
        let path = format!("/lol-summoner/v2/summoners/puuid/{}", encode_segment(puuid));
        self.get_json(&path).await?.ok_or(AppError::AccountNotFound)
    }

    /// Ранги игрока. Поражения клиент отдаёт только для аккаунта, в который в нём выполнен
    /// вход, — у остальных там всегда 0. Чтобы не показывать ложные «100% побед», у чужих
    /// аккаунтов помечаем поражения как неизвестные (`losses: None`).
    pub async fn ranked_by_puuid(&self, puuid: &str) -> AppResult<RankedQueues> {
        let path = format!("/lol-ranked/v1/ranked-stats/{}", encode_segment(puuid));
        let stats: Option<LcuRankedStats> = self.get_json(&path).await?;
        let mut ranked = stats.map(|s| s.to_queues()).unwrap_or_default();
        let is_current = self.current_summoner().await.is_ok_and(|me| me.puuid == puuid);
        if !is_current {
            ranked.hide_losses();
        }
        Ok(ranked)
    }

    /// Последние [`FETCH_MATCHES`] игр игрока (новые первыми). Клиент отдаёт историю
    /// и других игроков своего сервера, не только того, в кого выполнен вход.
    pub async fn recent_matches(&self, puuid: &str) -> AppResult<Vec<MatchSummary>> {
        let path = format!(
            "/lol-match-history/v1/products/lol/{}/matches?begIndex=0&endIndex={}",
            encode_segment(puuid),
            FETCH_MATCHES - 1 // endIndex включительно
        );
        let history: Option<LcuMatchHistory> = self.get_json(&path).await?;
        Ok(history.map(|h| h.to_summaries()).unwrap_or_default())
    }

    /// Одна игра целиком — все десять игроков (окно «Подробности игры»).
    pub async fn match_details(&self, game_id: i64) -> AppResult<LcuGame> {
        self.get_json(&format!("/lol-match-history/v1/games/{game_id}"))
            .await?
            .ok_or(AppError::MatchNotFound)
    }

    /// Новый аккаунт: Riot ID → профиль, ранги и последние игры.
    /// `region` — сервер аккаунта; вызывающий код проверяет, что он совпадает с сервером клиента.
    pub async fn fetch_by_riot_id(&self, id: &RiotId, region: Region) -> AppResult<AccountSnapshot> {
        let summoner = self.summoner_by_riot_id(id).await?;
        self.snapshot(summoner, id, region).await
    }

    /// Известный аккаунт по PUUID клиента.
    pub async fn fetch_by_puuid(
        &self,
        puuid: &str,
        current_id: &RiotId,
        region: Region,
    ) -> AppResult<AccountSnapshot> {
        let summoner = self.summoner_by_puuid(puuid).await?;
        self.snapshot(summoner, current_id, region).await
    }

    async fn snapshot(
        &self,
        summoner: LcuSummoner,
        fallback_id: &RiotId,
        region: Region,
    ) -> AppResult<AccountSnapshot> {
        let ranked = self.ranked_by_puuid(&summoner.puuid).await?;
        // Игры — дополнение к рангам: не загрузились — ранги всё равно сохраним.
        let matches = match self.recent_matches(&summoner.puuid).await {
            Ok(matches) => Some(matches),
            Err(e) => {
                warn!(error = %e, "последние игры не загрузились (клиент League)");
                None
            }
        };
        let riot_id = if summoner.game_name.is_empty() || summoner.tag_line.is_empty() {
            fallback_id.clone()
        } else {
            RiotId {
                game_name: summoner.game_name,
                tag_line: summoner.tag_line,
            }
        };
        Ok(AccountSnapshot {
            source: DataSource::Lcu,
            puuid: summoner.puuid,
            riot_id,
            region,
            profile_icon_id: summoner.profile_icon_id,
            summoner_level: summoner.summoner_level,
            ranked,
            matches,
            // Подробности из клиента — отдельный запрос на игру; их догружает
            // фоновый поток после сохранения (backend::archive_details).
            details: Vec::new(),
        })
    }

    // ----- Лобби и выбор чемпионов -------------------------------------------

    /// Этап игры: "None", "Lobby", "Matchmaking", "ReadyCheck", "ChampSelect",
    /// "GameStart", "InProgress", "EndOfGame"…
    pub async fn gameflow_phase(&self) -> AppResult<String> {
        self.get_json("/lol-gameflow/v1/gameflow-phase")
            .await?
            .ok_or(AppError::LcuNotReady)
    }

    /// Сколько валюты в кошельке: «lol_blue_essence», «lol_orange_essence», «RP».
    async fn wallet(&self, currency: &str) -> AppResult<i64> {
        let wallet: HashMap<String, i64> = self
            .get_json(&format!("/lol-inventory/v1/wallet/{currency}"))
            .await?
            .unwrap_or_default();
        Ok(wallet.get(currency).copied().unwrap_or(0))
    }

    /// Инвентарь аккаунта, в который выполнен вход: чемпионы, образы, валюта.
    /// Только чтение — пять GET-запросов.
    pub async fn inventory(&self, fetched_at: i64) -> AppResult<Inventory> {
        let champions: Vec<LcuOwnedChampion> = self
            .get_json("/lol-champions/v1/owned-champions-minimal")
            .await?
            .ok_or(AppError::LcuNotReady)?;
        let skins: Vec<LcuInventoryItem> = self
            .get_json("/lol-inventory/v2/inventory/CHAMPION_SKIN")
            .await?
            .unwrap_or_default();
        Ok(Inventory {
            champions: champions.iter().filter(|c| c.ownership.owned).count() as u32,
            skins: skins.iter().filter(|s| s.owned).count() as u32,
            blue_essence: self.wallet("lol_blue_essence").await?,
            orange_essence: self.wallet("lol_orange_essence").await?,
            rp: self.wallet("RP").await?,
            fetched_at,
        })
    }

    /// Текущая игра (`None` — игры нет).
    pub async fn gameflow_session(&self) -> AppResult<Option<LcuGameflowSession>> {
        self.get_json("/lol-gameflow/v1/session").await
    }

    /// Выбор чемпионов (`None` — его сейчас нет).
    pub async fn champ_select_session(&self) -> AppResult<Option<LcuChampSelectSession>> {
        self.get_json("/lol-champ-select/v1/session").await
    }

    /// PUUID участников группы (пусто — группы нет).
    pub async fn lobby_member_puuids(&self) -> AppResult<Vec<String>> {
        let lobby: Option<LcuLobby> = self.get_json("/lol-lobby/v2/lobby").await?;
        Ok(lobby
            .map(|l| l.members.into_iter().map(|m| m.puuid).filter(|p| !p.is_empty()).collect())
            .unwrap_or_default())
    }

    /// Названия чемпионов по id (на языке клиента).
    pub async fn champion_names(&self) -> AppResult<HashMap<i32, String>> {
        let list: Vec<LcuChampionSummary> = self
            .get_json("/lol-game-data/assets/v1/champion-summary.json")
            .await?
            .unwrap_or_default();
        // id -1 — «нет чемпиона».
        Ok(list.into_iter().filter(|c| c.id > 0).map(|c| (c.id, c.name)).collect())
    }

    /// Участники чатов из Riot Client (в том числе комнаты выбора чемпионов).
    /// Порт и токен — из аргументов LeagueClientUx.exe (см. `riot_client::read_auth`).
    pub async fn chat_participants(&self, auth: &RiotClientAuth) -> AppResult<Vec<ChatParticipant>> {
        let url = format!("https://127.0.0.1:{}/chat/v5/participants", auth.port);
        let response = self.get(&url, auth.token()).await?;
        let status = response.status();
        debug!(status = status.as_u16(), "Riot Client: участники чата");
        match status {
            s if s.is_success() => Ok(response.json::<ChatParticipants>().await?.participants),
            // Токен устарел (клиент перезапустили) — вызывающий код прочитает его заново.
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(AppError::LcuNotReady),
            // «not connected to chat» — чат ещё подключается.
            s if s.is_server_error() => Err(AppError::ChatUnavailable),
            s => Err(AppError::UnexpectedStatus(s.as_u16())),
        }
    }

    // ----- HTTP ---------------------------------------------------------------

    /// GET на 127.0.0.1 с Basic-авторизацией `riot:<токен>`.
    async fn get(&self, url: &str, token: &str) -> AppResult<reqwest::Response> {
        self.http
            .get(url)
            // basic_auth сам помечает заголовок как секретный (не попадёт в отладочный вывод).
            .basic_auth("riot", Some(token))
            .header(ACCEPT, "application/json")
            .send()
            .await
            .map_err(lcu_error)
    }

    /// GET к LCU. `Ok(None)` на 404.
    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> AppResult<Option<T>> {
        // lockfile читаем каждый раз: после перезапуска клиента порт и пароль новые.
        // Замок на список папок держим только на время копирования указателя —
        // через `.await` его держать нельзя (другие задачи ждали бы).
        let dirs = self.current_dirs();
        let lock = Lockfile::find(&dirs)
            .await?
            .ok_or(AppError::LcuNotRunning)?;
        let url = format!("https://127.0.0.1:{}{path}", lock.port);
        let response = self.get(&url, lock.password()).await?;

        let status = response.status();
        debug!(path, status = status.as_u16(), "LCU");
        match status {
            s if s.is_success() => Ok(Some(response.json::<T>().await?)),
            StatusCode::NOT_FOUND => Ok(None),
            // Остался lockfile от прошлого запуска (клиент упал) — пароль уже не тот.
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(AppError::LcuNotRunning),
            // Клиент запущен, но нужная его часть ещё не загрузилась.
            s if s.is_server_error() => Err(AppError::LcuNotReady),
            s => Err(AppError::UnexpectedStatus(s.as_u16())),
        }
    }
}

/// Ответ `/rso-auth/v1/session` Riot Client.
#[derive(serde::Deserialize)]
struct RiotSessionDto {
    #[serde(default)]
    r#type: String,
}

/// Что с входом в Riot Client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiotSession {
    /// Вход выполнен.
    Authenticated,
    /// Клиент ждёт код подтверждения (из почты или приложения).
    Multifactor,
    /// Вход не выполнен — показано окно входа.
    NeedsLogin,
}

impl RiotSession {
    /// Из поля `type`: «authenticated», «needs_multifactor_verification», иначе — входа нет
    /// («needs_authentication», «needs_credentials», «error»…).
    pub fn from_type(kind: &str) -> Self {
        if kind == "authenticated" {
            Self::Authenticated
        } else if kind.contains("multifactor") {
            Self::Multifactor
        } else {
            Self::NeedsLogin
        }
    }
}

/// Ошибки соединения с LCU переводим в понятные состояния клиента.
fn lcu_error(e: reqwest::Error) -> AppError {
    if e.is_connect() {
        AppError::LcuNotRunning // lockfile есть, а порт закрыт: клиент только что закрылся
    } else if e.is_timeout() {
        AppError::LcuNotReady
    } else {
        AppError::from(e)
    }
}

#[cfg(test)]
mod riot_session_tests {
    use super::RiotSession;

    #[test]
    fn session_type_maps_to_three_states() {
        assert_eq!(RiotSession::from_type("authenticated"), RiotSession::Authenticated);
        assert_eq!(RiotSession::from_type("needs_multifactor_verification"), RiotSession::Multifactor);
        assert_eq!(RiotSession::from_type("needs_authentication"), RiotSession::NeedsLogin);
        // Незнакомое и пустое — «входа нет»: печатать пароль мы всё равно будем только
        // в окно Riot Client.
        assert_eq!(RiotSession::from_type(""), RiotSession::NeedsLogin);
    }
}
