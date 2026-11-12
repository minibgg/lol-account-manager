//! HTTP-клиент Riot API: ключ, лимиты запросов, повторы при 429, понятные ошибки.
//!
//! Про async: функция `async fn` при вызове ничего не делает, а возвращает Future —
//! «обещание» результата. `.await` ждёт его, не блокируя поток: пока ответ
//! от сервера в пути, tokio-runtime выполняет другие задачи. Это как Promise/await в JS,
//! только Future в Rust «ленивый» — без `.await` (или spawn) он вообще не запустится.

use std::fmt;
use std::num::NonZeroU32;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant};

use governor::{DefaultKeyedRateLimiter, Quota};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::StatusCode;
use reqwest::header::{HeaderValue, RETRY_AFTER};
use serde::de::DeserializeOwned;
use tracing::{debug, warn};

use super::models::{
    API_NEW_MATCHES_PER_REFRESH, AccountDto, AccountSnapshot, DataSource, FETCH_MATCHES, LeagueEntryDto,
    MatchDetails, MatchDto, MatchSummary, RankedQueues, SummonerDto,
};
use super::routing::{Region, RiotId};
use crate::error::{AppError, AppResult};

const DDRAGON_VERSIONS_URL: &str = "https://ddragon.leagueoflegends.com/api/versions.json";

/// Сколько раз всего пробуем запрос (первая попытка + повторы после 429 / 5xx).
const MAX_ATTEMPTS: u32 = 4;

/// Если Riot просит подождать дольше — не ждём молча, а сообщаем об ошибке.
const MAX_AUTO_WAIT: Duration = Duration::from_secs(130);

/// Пауза после 429 без заголовка Retry-After (такое бывает при перегрузке сервиса Riot).
const DEFAULT_RETRY_WAIT: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// API-ключ
// ---------------------------------------------------------------------------

/// API-ключ Riot.
///
/// Обёртка нужна, чтобы ключ никогда не попал в лог: `Debug` печатает `ApiKey(***)`,
/// а `Display` не реализован вовсе — `format!("{key}")` просто не скомпилируется.
/// Внутри — готовое значение заголовка с пометкой «sensitive» (reqwest тоже не печатает его).
#[derive(Clone)]
pub struct ApiKey(HeaderValue);

impl ApiKey {
    pub fn new(raw: &str) -> AppResult<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(AppError::MissingApiKey);
        }
        // Заголовок может содержать только видимые ASCII-символы.
        let mut value = HeaderValue::from_str(raw).map_err(|_| AppError::InvalidApiKey)?;
        value.set_sensitive(true);
        Ok(Self(value))
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(***)")
    }
}

// ---------------------------------------------------------------------------
// Лимиты запросов
// ---------------------------------------------------------------------------

/// Лимиты ключа разработчика: 20 запросов в секунду и 100 за 2 минуты.
/// Riot считает их отдельно для каждого региона (хоста), поэтому лимитеры «по ключу»:
/// ключ — хост (`euw1`, `europe`, ...), у каждого свой счётчик.
///
/// governor работает как «ведро токенов»: есть запас (burst), который пополняется
/// с постоянной скоростью. Riot же считает запросы в окнах по 1 с и 2 мин. Чтобы не
/// превысить лимит ни в одном окне, нужно: запас + скорость × длина окна ≤ лимит.
///   окно 1 с:   10 + 10/с × 1 с         = 20
///   окно 2 мин: 30 + (1 / 1,8 с) × 120 с ≈ 97  (небольшой запас на задержки сети)
struct RateLimits {
    per_second: DefaultKeyedRateLimiter<&'static str>,
    per_two_minutes: DefaultKeyedRateLimiter<&'static str>,
}

impl RateLimits {
    fn dev_key() -> Self {
        let burst = |n| NonZeroU32::new(n).expect("burst > 0");
        let per_second = Quota::with_period(Duration::from_millis(100))
            .expect("период > 0")
            .allow_burst(burst(10));
        let per_two_minutes = Quota::with_period(Duration::from_millis(1800))
            .expect("период > 0")
            .allow_burst(burst(30));
        Self {
            per_second: DefaultKeyedRateLimiter::keyed(per_second),
            per_two_minutes: DefaultKeyedRateLimiter::keyed(per_two_minutes),
        }
    }

    /// Ждёт, пока оба лимита разрешат запрос к `host`.
    async fn wait(&self, host: &'static str) {
        let started = Instant::now();
        // Сначала «медленный» лимит, потом «быстрый»: быстрый проверяется последним,
        // прямо перед отправкой, поэтому его гарантия соблюдается точно.
        self.per_two_minutes.until_key_ready(&host).await;
        self.per_second.until_key_ready(&host).await;
        let waited = started.elapsed();
        if waited > Duration::from_millis(200) {
            debug!(host, waited_ms = waited.as_millis() as u64, "ждали лимит запросов");
        }
    }
}

// ---------------------------------------------------------------------------
// Клиент
// ---------------------------------------------------------------------------

/// Колбэк «Riot попросил подождать N секунд» — чтобы показать это в UI.
/// `dyn Fn(...)` — любая функция/замыкание с такой сигнатурой;
/// `Send + Sync` — её можно безопасно вызывать из разных потоков.
type WaitNotifier = Arc<dyn Fn(Duration) + Send + Sync>;

/// Общая «ячейка» с ключом.
///
/// `RwLock` — замок «много читателей ИЛИ один писатель»: запросы читают ключ
/// параллельно, а смена ключа в настройках ненадолго их останавливает.
/// Это «внутренняя изменяемость» (interior mutability): менять значение можно
/// через обычную ссылку `&self`, а безопасность обеспечивает замок, а не компилятор.
type KeySlot = Arc<RwLock<Option<ApiKey>>>;

/// Клиент Riot API.
///
/// `#[derive(Clone)]`, и клонирование дешёвое: `reqwest::Client` и `Arc` внутри —
/// это счётчики ссылок на общие данные, а не копии. Клоны разделяют пул соединений,
/// лимиты и ключ, поэтому их можно раздавать параллельным задачам: новый ключ из
/// настроек сразу увидят все клоны.
#[derive(Clone)]
pub struct RiotClient {
    http: reqwest::Client,
    api_key: KeySlot,
    limits: Arc<RateLimits>,
    on_wait: Option<WaitNotifier>,
}

impl RiotClient {
    /// Клиент без ключа: Data Dragon работает, Riot API вернёт `MissingApiKey`.
    pub fn new() -> AppResult<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("lol-account-manager/", env!("CARGO_PKG_VERSION")))
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            http,
            api_key: Arc::new(RwLock::new(None)),
            limits: Arc::new(RateLimits::dev_key()),
            on_wait: None,
        })
    }

    /// Builder-стиль: `RiotClient::new()?.with_api_key(key)`.
    /// `mut self` — метод забирает клиента во владение, меняет и возвращает обратно.
    ///
    /// Ключ кладётся в НОВУЮ ячейку: `client.clone().with_api_key(k)` — это отдельный
    /// клиент со своим ключом (например, проверить введённый ключ, не сохраняя его),
    /// но с теми же соединениями и лимитами.
    pub fn with_api_key(mut self, key: ApiKey) -> Self {
        self.api_key = Arc::new(RwLock::new(Some(key)));
        self
    }

    /// Сменить ключ (пользователь сохранил новый в настройках). Смена видна всем клонам.
    pub fn set_api_key(&self, key: Option<ApiKey>) {
        // write() вернёт Err, только если другой поток запаниковал, держа замок.
        // Ключ при этом цел, поэтому просто продолжаем (into_inner).
        *self.api_key.write().unwrap_or_else(PoisonError::into_inner) = key;
    }

    pub fn has_api_key(&self) -> bool {
        self.current_key().is_some()
    }

    /// Копия текущего ключа. Замок держим только на время копирования
    /// и НЕ держим во время запроса (через `.await`).
    fn current_key(&self) -> Option<ApiKey> {
        self.api_key
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Подписаться на ожидания после ответа 429.
    /// `'static` — замыкание не держит ссылок на временные данные и может жить сколько угодно.
    pub fn on_rate_limit_wait(mut self, f: impl Fn(Duration) + Send + Sync + 'static) -> Self {
        self.on_wait = Some(Arc::new(f));
        self
    }

    // ----- Эндпоинты Riot API -----------------------------------------------

    /// account-v1: Riot ID → PUUID.
    pub async fn account_by_riot_id(&self, id: &RiotId, region: Region) -> AppResult<AccountDto> {
        let path = format!(
            "/riot/account/v1/accounts/by-riot-id/{}/{}",
            encode_segment(&id.game_name),
            encode_segment(&id.tag_line)
        );
        self.get_json(region.account_route().host(), &path)
            .await?
            .ok_or(AppError::AccountNotFound)
    }

    /// account-v1: PUUID → актуальный Riot ID (если игрок его сменил).
    pub async fn account_by_puuid(&self, puuid: &str, region: Region) -> AppResult<AccountDto> {
        let path = format!("/riot/account/v1/accounts/by-puuid/{}", encode_segment(puuid));
        self.get_json(region.account_route().host(), &path)
            .await?
            .ok_or(AppError::AccountNotFound)
    }

    /// summoner-v4: уровень и иконка профиля.
    pub async fn summoner_by_puuid(&self, puuid: &str, region: Region) -> AppResult<SummonerDto> {
        let path = format!("/lol/summoner/v4/summoners/by-puuid/{}", encode_segment(puuid));
        self.get_json(region.platform(), &path)
            .await?
            .ok_or(AppError::SummonerNotFound(region.label()))
    }

    /// league-v4: ранги Solo/Duo и Flex. Пустой список = без ранга.
    pub async fn ranked_by_puuid(&self, puuid: &str, region: Region) -> AppResult<RankedQueues> {
        let path = format!("/lol/league/v4/entries/by-puuid/{}", encode_segment(puuid));
        let entries: Vec<LeagueEntryDto> = self
            .get_json(region.platform(), &path)
            .await?
            .unwrap_or_default();
        Ok(RankedQueues::from_entries(&entries))
    }

    /// match-v5: последние [`FETCH_MATCHES`] игр, новые первыми, и подробности тех, что
    /// пришлось загрузить (игра и так приходит целиком — обе команды достаются даром).
    /// Игры, которые уже есть в `known` (архив), заново не загружаем: обычно новых 0–2,
    /// и обновление стоит 1–3 запроса. Новых больше [`API_NEW_MATCHES_PER_REFRESH`] —
    /// остальные загрузятся при следующих обновлениях (их не будет в списке).
    pub async fn recent_matches(
        &self,
        puuid: &str,
        region: Region,
        known: &[MatchSummary],
    ) -> AppResult<(Vec<MatchSummary>, Vec<MatchDetails>)> {
        // match-v5 живёт на региональных кластерах (europe, americas, asia, sea).
        let host = region.regional().host();
        let path = format!(
            "/lol/match/v5/matches/by-puuid/{}/ids?start=0&count={FETCH_MATCHES}",
            encode_segment(puuid)
        );
        let ids: Vec<String> = self.get_json(host, &path).await?.unwrap_or_default();

        let mut matches = Vec::with_capacity(ids.len());
        let mut details = Vec::new();
        let mut budget = API_NEW_MATCHES_PER_REFRESH;
        for id in ids {
            // Из архива — только записи со всеми показателями (старые без урона и предметов
            // загрузим заново).
            if let Some(cached) = known.iter().find(|m| m.id == id && m.has_full_stats()) {
                matches.push(cached.clone());
                continue;
            }
            if budget == 0 {
                continue; // дождётся следующего обновления
            }
            budget -= 1;
            let path = format!("/lol/match/v5/matches/{}", encode_segment(&id));
            // 404 — игру не отдают (бывает у очень старых или служебных) — пропускаем.
            if let Some(game) = self.get_json::<MatchDto>(host, &path).await?
                && let Some(summary) = game.summary_for(puuid)
            {
                matches.push(summary);
                details.push(game.details(Some(puuid)));
            }
        }
        Ok((matches, details))
    }

    /// match-v5: одна игра целиком (окно «Подробности игры»).
    pub async fn match_by_id(&self, match_id: &str, region: Region) -> AppResult<MatchDto> {
        let path = format!("/lol/match/v5/matches/{}", encode_segment(match_id));
        self.get_json(region.regional().host(), &path)
            .await?
            .ok_or(AppError::MatchNotFound)
    }

    /// Новый аккаунт: Riot ID → PUUID → профиль, ранги и последние игры.
    pub async fn fetch_by_riot_id(&self, id: &RiotId, region: Region) -> AppResult<AccountSnapshot> {
        let account = self.account_by_riot_id(id, region).await?;
        self.snapshot(account, id, region, &[]).await
    }

    /// Обновление известного аккаунта по PUUID. Работает, даже если игрок
    /// сменил Riot ID: новый придёт в ответе account-v1.
    /// `known_matches` — игры из кэша (их не загружаем повторно).
    pub async fn fetch_by_puuid(
        &self,
        puuid: &str,
        current_id: &RiotId,
        region: Region,
        known_matches: &[MatchSummary],
    ) -> AppResult<AccountSnapshot> {
        let account = self.account_by_puuid(puuid, region).await?;
        self.snapshot(account, current_id, region, known_matches).await
    }

    async fn snapshot(
        &self,
        account: AccountDto,
        fallback_id: &RiotId,
        region: Region,
        known_matches: &[MatchSummary],
    ) -> AppResult<AccountSnapshot> {
        // Профиль и ранги не зависят друг от друга — запрашиваем параллельно.
        // try_join! ждёт оба Future; если любой вернёт Err — сразу отдаёт эту ошибку.
        let (summoner, ranked) = tokio::try_join!(
            self.summoner_by_puuid(&account.puuid, region),
            self.ranked_by_puuid(&account.puuid, region),
        )?;
        // Игры — дополнение к рангам: если они не загрузились, ранги всё равно сохраним.
        let (matches, details) = match self.recent_matches(&account.puuid, region, known_matches).await {
            Ok((matches, details)) => (Some(matches), details),
            Err(e) => {
                warn!(error = %e, "последние игры не загрузились (Riot API)");
                (None, Vec::new())
            }
        };

        // match по кортежу из двух Option: Riot ID берём из ответа, если он есть целиком.
        let riot_id = match (account.game_name, account.tag_line) {
            (Some(game_name), Some(tag_line)) => RiotId { game_name, tag_line },
            _ => fallback_id.clone(),
        };

        Ok(AccountSnapshot {
            source: DataSource::RiotApi,
            puuid: account.puuid,
            riot_id,
            region,
            profile_icon_id: summoner.profile_icon_id,
            summoner_level: summoner.summoner_level,
            ranked,
            matches,
            details,
        })
    }

    /// Проверка ключа: запрос статуса сервера (lol-status-v4) требует валидный ключ
    /// и почти ничего не стоит.
    pub async fn check_api_key(&self, region: Region) -> AppResult<()> {
        self.get_json::<serde_json::Value>(region.platform(), "/lol/status/v4/platform-data")
            .await?;
        Ok(())
    }

    // ----- Data Dragon (статика, без ключа) ---------------------------------

    /// Актуальная версия Data Dragon (первая в versions.json).
    /// Ключ сюда НЕ отправляем: X-Riot-Token уходит только на *.api.riotgames.com.
    pub async fn ddragon_latest_version(&self) -> AppResult<String> {
        let versions: Vec<String> = self
            .http
            .get(DDRAGON_VERSIONS_URL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        versions
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Decode("versions.json пустой".into()))
    }

    /// URL иконки профиля. Обычная функция без `self`: вызывается как `RiotClient::profile_icon_url(..)`.
    /// URL иконки предмета (64×64).
    pub fn item_icon_url(ddragon_version: &str, item_id: i32) -> String {
        format!("https://ddragon.leagueoflegends.com/cdn/{ddragon_version}/img/item/{item_id}.png")
    }

    pub fn profile_icon_url(ddragon_version: &str, icon_id: i32) -> String {
        format!("https://ddragon.leagueoflegends.com/cdn/{ddragon_version}/img/profileicon/{icon_id}.png")
    }

    // ----- Общий GET с ключом, лимитами и повторами -------------------------

    /// GET к Riot API. Возвращает `Ok(None)` на 404 — что именно «не найдено»,
    /// решает вызывающий метод (аккаунт? профиль на сервере?).
    ///
    /// `<T: DeserializeOwned>` — обобщённый тип: функция умеет разбирать JSON
    /// в любой тип, который serde умеет создавать.
    async fn get_json<T: DeserializeOwned>(
        &self,
        host: &'static str,
        path: &str,
    ) -> AppResult<Option<T>> {
        let key = self.current_key().ok_or(AppError::MissingApiKey)?;
        let url = format!("https://{host}.api.riotgames.com{path}");

        let mut attempt = 1;
        loop {
            self.limits.wait(host).await;

            let started = Instant::now();
            let response = self
                .http
                .get(&url)
                .header("X-Riot-Token", key.0.clone())
                .send()
                .await?;
            let status = response.status();
            // В лог — хост, путь и статус. Ключ в URL не входит, а заголовок помечен sensitive.
            debug!(
                host,
                path,
                status = status.as_u16(),
                ms = started.elapsed().as_millis() as u64,
                "Riot API"
            );

            match status {
                s if s.is_success() => return Ok(Some(response.json::<T>().await?)),

                StatusCode::NOT_FOUND => return Ok(None),

                StatusCode::TOO_MANY_REQUESTS => {
                    let wait = retry_after(&response).unwrap_or(DEFAULT_RETRY_WAIT);
                    if attempt >= MAX_ATTEMPTS || wait > MAX_AUTO_WAIT {
                        return Err(AppError::RateLimited {
                            retry_after_secs: wait.as_secs().max(1),
                        });
                    }
                    // X-Rate-Limit-Type: application / method / service (может отсутствовать).
                    let limit_type = response
                        .headers()
                        .get("X-Rate-Limit-Type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("service");
                    warn!(host, path, limit_type, wait_s = wait.as_secs(), attempt,
                        "429 от Riot: ждём Retry-After и повторяем");
                    if let Some(notify) = &self.on_wait {
                        notify(wait);
                    }
                    tokio::time::sleep(wait).await;
                }

                s if s.is_server_error() && attempt < MAX_ATTEMPTS => {
                    // 1, 2, 4 секунды — «экспоненциальная» пауза между повторами.
                    let wait = Duration::from_secs(1 << (attempt - 1));
                    warn!(host, path, status = s.as_u16(), wait_s = wait.as_secs(),
                        "ошибка сервера Riot, повторяем");
                    tokio::time::sleep(wait).await;
                }

                s => return Err(AppError::from_status(s.as_u16())),
            }
            attempt += 1;
        }
    }
}

/// Значение заголовка Retry-After (в секундах).
/// Внутри — цепочка `?` на Option: если на любом шаге None, функция вернёт None.
fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    let secs: u64 = response
        .headers()
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(secs))
}

/// Кодирование сегмента пути URL: всё, кроме `A–Z a–z 0–9 - . _ ~`, превращается в `%XX`
/// (UTF-8 байты). Пробел → `%20`, «Ё» → `%D0%81`, `#` → `%23`, `/` → `%2F`.
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// `pub(crate)` — видно во всём нашем крейте (нужно и клиенту LCU), но не снаружи.
pub(crate) fn encode_segment(s: &str) -> String {
    utf8_percent_encode(s, PATH_SEGMENT).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_path_segments() {
        assert_eq!(encode_segment("Hide on bush"), "Hide%20on%20bush");
        assert_eq!(encode_segment("Ёж"), "%D0%81%D0%B6");
        assert_eq!(encode_segment("a/b#c?d%"), "a%2Fb%23c%3Fd%25");
        assert_eq!(encode_segment("KR1"), "KR1");
        assert_eq!(encode_segment("x-y_z.~"), "x-y_z.~");
    }

    #[test]
    fn api_key_is_never_printed() {
        let key = ApiKey::new("  RGAPI-secret-value  ").unwrap();
        assert_eq!(format!("{key:?}"), "ApiKey(***)");
        assert!(ApiKey::new("   ").is_err());
    }

    #[test]
    fn profile_icon_url_format() {
        assert_eq!(
            RiotClient::profile_icon_url("16.21.1", 588),
            "https://ddragon.leagueoflegends.com/cdn/16.21.1/img/profileicon/588.png"
        );
    }

    /// Сетевой тест, по умолчанию пропускается. Запуск: `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "нужен интернет"]
    async fn ddragon_version_live() {
        let version = RiotClient::new().unwrap().ddragon_latest_version().await.unwrap();
        assert!(version.split('.').count() >= 3, "странная версия: {version}");
    }

    /// Без ключа запросы к Riot API даже не отправляются.
    #[tokio::test]
    async fn missing_key_is_reported_without_network() {
        let client = RiotClient::new().unwrap();
        let id = RiotId::parse("Name#TAG").unwrap();
        let err = client.account_by_riot_id(&id, Region::Euw).await.unwrap_err();
        assert_eq!(err, AppError::MissingApiKey);
    }
}
