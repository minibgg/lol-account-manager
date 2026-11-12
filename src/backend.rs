//! Фоновый поток: tokio-runtime, база, хранилище паролей, Riot API, буфер обмена.
//!
//! Зачем отдельный поток. eframe рисует окно в главном потоке и вызывает `App::ui`
//! десятки раз в секунду. Если там ждать сеть (сотни миллисекунд) или хранилище
//! паролей, окно «замёрзнет». Поэтому всё медленное живёт здесь, а окно только
//! отправляет [`Command`] и забирает [`Event`] — ни того, ни другого оно не ждёт.
//!
//! Внутри потока работает tokio-runtime. Он выполняет задачи (tasks) — async-функции,
//! запущенные через `tokio::spawn`. Пока задача ждёт ответа сети на `.await`, поток
//! свободен и выполняет другие задачи. Похоже на Promise в JS, только задачи могут
//! идти и по-настоящему параллельно, на нескольких потоках.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;
use tokio::sync::Semaphore;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};

use crate::clipboard::{self, SecretClipboard};
use crate::error::{AppError, AppResult};
use crate::history::Queue;
use crate::images::{self, ImageCache};
use crate::launcher;
use crate::lcu::models::ChatParticipant;
use crate::lcu::riot_client::{self, RiotClientAuth};
use crate::lcu::{LcuClient, LcuStatus, lockfile};
use crate::lobby::{self, LobbyPhase, LobbyPlayer, LobbyState, PlayerProfile};
use crate::messages::{Command, CredentialKind, Event, ImportReport, LeagueDirInfo, Secret};
use crate::riot::{
    AccountSnapshot, ApiKey, DataSource, MatchDetails, MatchSummary, PlayerStats, RankedQueues, Region, RiotClient,
    RiotId,
};
use crate::storage::{Account, AccountId, AccountInput, Credentials, SecretChange, Storage, unix_now};
use crate::transfer;

/// Сколько аккаунтов обновляются одновременно. Остальные ждут в очереди:
/// так карточки обновляются по одной сверху вниз, а не все разом в самом конце.
const PARALLEL_REFRESHES: usize = 3;

/// На каком сервере проверять ключ. Ключ общий для всех регионов, подойдёт любой.
const KEY_CHECK_REGION: Region = Region::Euw;

/// Как часто проверять, запущен ли клиент League и в какой аккаунт выполнен вход.
/// Проверка дешёвая: чтение маленького файла и пара запросов к 127.0.0.1.
const LCU_POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Как часто смотреть, что с лобби (выбор чемпионов идёт быстро — чаще, чем LCU_POLL_INTERVAL).
const LOBBY_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Сколько последних игр архива сверять с ответом Riot API (чтобы не загружать их снова).
const KNOWN_MATCHES: usize = 100;

/// Сколько подробностей игр догружать в архив из клиента League за одно обновление.
const DETAILS_PER_REFRESH: usize = 20;

/// Через сколько после конца игры пробуем обновить аккаунт (каждая следующая попытка —
/// только если предыдущая ничего нового не принесла).
const AFTER_GAME_RETRIES: [Duration; 3] = [
    Duration::from_secs(3),
    Duration::from_secs(12),
    Duration::from_secs(40),
];

/// Настройка (в SQLite) с папкой игры, если она не находится сама.
pub const LEAGUE_DIR_SETTING: &str = "league_dir";

/// Сколько окно ждёт фоновый поток при выходе.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

/// Как открыть хранилище. `Box<dyn FnOnce...>` — «какая-то функция, которую можно
/// вызвать один раз». В приложении это папка данных и системное хранилище паролей,
/// в тестах — временная папка и отдельное имя сервиса.
pub type OpenStorage = Box<dyn FnOnce() -> AppResult<Storage> + Send>;

pub struct BackendConfig {
    pub open_storage: OpenStorage,
    pub clipboard_clear_after: Duration,
    /// Куда складывать скачанные иконки и эмблемы.
    pub image_cache_dir: PathBuf,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            open_storage: Box::new(Storage::open_default),
            clipboard_clear_after: clipboard::CLEAR_AFTER,
            image_cache_dir: images::default_cache_dir(),
        }
    }
}

// ---------------------------------------------------------------------------
// Ручка для окна
// ---------------------------------------------------------------------------

/// То, что держит окно: конец канала команд, конец канала событий и сам поток.
pub struct Backend {
    /// `Option` — чтобы при выходе сначала закрыть канал, а потом ждать поток.
    commands: Option<UnboundedSender<Command>>,
    events: UnboundedReceiver<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Backend {
    /// Запуск с настоящими базой и хранилищем паролей.
    pub fn start(ctx: egui::Context) -> Self {
        Self::start_with(ctx, BackendConfig::default())
    }

    pub fn start_with(ctx: egui::Context, config: BackendConfig) -> Self {
        // Два канала: команды (окно → фон) и события (фон → окно).
        // unbounded — очередь без ограничения длины, поэтому `send` никогда не ждёт.
        // Именно это нужно окну: оно не должно блокироваться ни на чём.
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let events = EventSink { tx: event_tx, ctx };

        // `move` переносит command_rx, events и config во владение нового потока.
        // Поток не запустится, только если у системы кончились ресурсы — тогда
        // работать всё равно невозможно, поэтому expect (паника с понятным текстом).
        let thread = std::thread::Builder::new()
            .name("backend".into())
            .spawn(move || run_thread(command_rx, events, config))
            .expect("не удалось запустить фоновый поток");

        Self {
            commands: Some(command_tx),
            events: event_rx,
            thread: Some(thread),
        }
    }

    /// Отправить команду. Возвращается сразу.
    pub fn send(&self, command: Command) {
        let Some(tx) = &self.commands else { return };
        // Ошибка — только если фоновый поток уже завершился (например, не открылась база).
        if tx.send(command).is_err() {
            warn!("фоновый поток остановлен, команда не выполнена");
        }
    }

    /// Следующее событие, если оно уже пришло. `try_recv` не ждёт:
    /// при пустой очереди сразу возвращает ошибку, а мы превращаем её в `None`.
    pub fn try_recv(&mut self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

/// `Drop` — код, который Rust вызывает сам, когда значение уничтожается.
/// `Backend` живёт внутри `App`, поэтому это происходит при закрытии окна.
impl Drop for Backend {
    fn drop(&mut self) {
        // 1. Закрываем канал команд: цикл в фоне получит `None` и завершится.
        drop(self.commands.take());
        // 2. Ждём поток (при выходе он стирает пароль из буфера обмена, если тот
        //    ещё там), но недолго: зависший запрос не должен держать окно.
        let Some(thread) = self.thread.take() else { return };
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if thread.is_finished() {
            let _ = thread.join();
        } else {
            warn!("фоновый поток не завершился за {} с", SHUTDOWN_WAIT.as_secs());
        }
    }
}

// ---------------------------------------------------------------------------
// Фоновый поток
// ---------------------------------------------------------------------------

/// Отправка событий в окно.
///
/// egui перерисовывает окно, только когда что-то происходит (мышь, клавиатура).
/// Событие из другого потока само его не разбудит, поэтому после отправки зовём
/// `request_repaint()` — окно перерисуется и заберёт событие.
#[derive(Clone)]
struct EventSink {
    tx: UnboundedSender<Event>,
    ctx: egui::Context,
}

impl EventSink {
    fn send(&self, event: Event) {
        // Ошибка — окно уже закрыто, показывать событие некому.
        if self.tx.send(event).is_ok() {
            self.ctx.request_repaint();
        }
    }
}

fn run_thread(commands: UnboundedReceiver<Command>, events: EventSink, config: BackendConfig) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("backend-worker")
        .enable_all() // таймеры и сеть
        .build();
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(e) => {
            events.send(Event::StartupFailed(AppError::Internal(format!("tokio: {e}"))));
            return;
        }
    };

    let clipboard = SecretClipboard::new();
    // block_on — выполнить async-функцию до конца, блокируя ЭТОТ поток (не окно).
    runtime.block_on(run(commands, events, config, clipboard.clone()));

    // Окно закрылось. Незаконченные задачи прерываются, а операциям с базой
    // и хранилищем паролей, которые уже выполняются, даём до 2 секунд.
    runtime.shutdown_timeout(Duration::from_secs(2));
    // Таймеры очистки буфера прерваны вместе с задачами — чистим сами.
    if clipboard.clear_if_ours(None) {
        info!("буфер обмена очищен при выходе");
    }
    info!("фоновый поток остановлен");
}

async fn run(
    mut commands: UnboundedReceiver<Command>,
    events: EventSink,
    config: BackendConfig,
    clipboard: SecretClipboard,
) {
    let worker = match Worker::start(events.clone(), config, clipboard).await {
        Ok(worker) => worker,
        Err(e) => {
            error!(error = %e, "фоновый поток не запустился");
            events.send(Event::StartupFailed(e));
            return;
        }
    };

    // recv() ждёт следующую команду (не занимая поток). None — канал закрыт:
    // окно закрылось и уничтожило свой конец канала.
    while let Some(command) = commands.recv().await {
        // Секреты в Debug замаскированы (см. messages.rs), поэтому логировать безопасно.
        debug!(?command, "команда");
        worker.handle(command);
    }
}

/// Выполнить блокирующую функцию (SQLite, хранилище паролей, буфер обмена)
/// в отдельном пуле потоков tokio и дождаться результата, не блокируя остальные задачи.
async fn blocking<T, F>(f: F) -> AppResult<T>
where
    F: FnOnce() -> AppResult<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| AppError::Internal("фоновая задача аварийно завершилась".into()))?
}

/// Всё, что нужно задачам. `Clone` дешёвый: внутри только `Arc` и клиенты,
/// которые сами по себе — указатели на общие данные.
#[derive(Clone)]
struct Worker {
    /// `Mutex`: соединение SQLite нельзя использовать из двух потоков одновременно.
    storage: Arc<Mutex<Storage>>,
    /// Основной источник данных: запущенный клиент League (без ключа и лимитов).
    lcu: LcuClient,
    /// Запасной источник: Riot API (нужен ключ). Для других серверов и закрытого клиента.
    riot: RiotClient,
    /// Иконки профиля и эмблемы рангов.
    images: ImageCache,
    events: EventSink,
    clipboard: SecretClipboard,
    clipboard_clear_after: Duration,
    /// Аккаунты, которые сейчас обновляются или ждут очереди.
    refreshing: Arc<Mutex<HashSet<AccountId>>>,
    /// Семафор — «N пропусков»: задача берёт пропуск перед обновлением и
    /// возвращает после. Когда пропуска кончились, следующие ждут.
    refresh_slots: Arc<Semaphore>,
}

impl Worker {
    async fn start(
        events: EventSink,
        config: BackendConfig,
        clipboard: SecretClipboard,
    ) -> AppResult<Self> {
        // Деструктуризация: разбираем структуру на поля-переменные.
        let BackendConfig {
            open_storage,
            clipboard_clear_after,
            image_cache_dir,
        } = config;
        // Открыть базу и найти папку клиента League (папка из настроек + стандартные
        // места) — всё это блокирующие операции с диском, поэтому в `blocking`.
        let (storage, (league_dir, dirs)) = blocking(move || {
            let storage = open_storage()?;
            let custom = storage.setting(LEAGUE_DIR_SETTING)?;
            Ok((storage, league_dirs(custom)))
        })
        .await?;
        debug!(?dirs, "где искать клиент League");
        let lcu = LcuClient::new(dirs)?;

        let sink = events.clone();
        let riot = RiotClient::new()?.on_rate_limit_wait(move |wait| {
            sink.send(Event::RateLimitWait {
                secs: wait.as_secs().max(1),
            });
        });
        let images = ImageCache::new(image_cache_dir, riot.clone())?;

        let worker = Self {
            storage: Arc::new(Mutex::new(storage)),
            lcu,
            riot,
            images,
            events,
            clipboard,
            clipboard_clear_after,
            refreshing: Arc::default(),
            refresh_slots: Arc::new(Semaphore::new(PARALLEL_REFRESHES)),
        };

        // Хранилище паролей может быть недоступно (например, на Linux не запущен
        // Secret Service). Список из базы всё равно покажем, а ошибку — предупреждением.
        let has_api_key = match worker.with_storage(|s| s.api_key()).await {
            Ok(key) => {
                let has_key = key.is_some();
                worker.riot.set_api_key(key);
                has_key
            }
            Err(e) => {
                warn!(error = %e, "не удалось прочитать API-ключ");
                worker.events.send(Event::Warning(e));
                false
            }
        };
        info!(has_api_key, "фоновый поток запущен");
        worker.events.send(Event::Ready {
            has_api_key,
            league_dir,
        });
        worker.reload_accounts().await;

        // Отдельные задачи следят за клиентом League и за лобби всё время работы программы.
        tokio::spawn(worker.clone().watch_lcu());
        tokio::spawn(worker.clone().watch_lobby());
        Ok(worker)
    }

    /// Раз в 2 секунды смотрит, что с лобби, и сообщает окну, если что-то изменилось.
    async fn watch_lobby(self) {
        let mut cache = LobbyCache::default();
        let mut last: Option<LobbyState> = None;
        let mut ticker = tokio::time::interval(LOBBY_POLL_INTERVAL);
        // Если опрос затянулся, не «догоняем» пропущенные тики пачкой.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let state = self.poll_lobby(&mut cache).await;
            // mem::take — забрать значение и оставить на его месте false.
            if std::mem::take(&mut cache.game_ended) {
                info!("игра закончилась — обновляю аккаунт из клиента");
                tokio::spawn(self.clone().refresh_after_game());
            }
            if last.as_ref() != Some(&state) {
                debug!(phase = ?state.phase, players = state.players.len(), "лобби");
                self.events.send(Event::Lobby(state.clone()));
                last = Some(state);
            }
        }
    }

    async fn poll_lobby(&self, cache: &mut LobbyCache) -> LobbyState {
        let raw_phase = match self.lcu.gameflow_phase().await {
            Ok(phase) => Some(phase),
            Err(AppError::LcuNotRunning) => {
                *cache = LobbyCache::default(); // клиент закрыли — всё забываем
                return LobbyState::default();
            }
            Err(_) => None, // клиент ещё загружается
        };
        if let Some(raw) = &raw_phase {
            let running = LobbyPhase::game_running(raw);
            // Шла игра, а теперь нет — она закончилась (см. watch_lobby).
            cache.game_ended |= cache.game_running && !running;
            cache.game_running = running;
        }
        // map_or(по умолчанию, функция): пока клиент загружается — «ничего не происходит».
        let phase = raw_phase.as_deref().map_or(LobbyPhase::Idle, LobbyPhase::from_gameflow);
        if cache.my_puuid.is_none() && phase != LobbyPhase::Idle {
            cache.my_puuid = self.lcu.current_summoner().await.ok().map(|s| s.puuid);
        }

        let mut problem = None;
        let mut players = match phase {
            LobbyPhase::Party | LobbyPhase::Queue => self
                .lcu
                .lobby_member_puuids()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(LobbyPlayer::new)
                .collect(),
            LobbyPhase::ChampSelect => {
                let session = self.lcu.champ_select_session().await.ok().flatten();
                let chat = match self.chat_participants(cache).await {
                    Ok(chat) => chat,
                    Err(e) => {
                        problem = Some(e);
                        Vec::new()
                    }
                };
                if cache.champions.is_empty()
                    && let Ok(names) = self.lcu.champion_names().await
                {
                    cache.champions = names;
                }
                lobby::champ_select_players(&chat, session.as_ref(), &cache.champions, cache.my_puuid.as_deref())
            }
            // В игре показываем команду из выбора чемпионов (если он был при нас).
            LobbyPhase::InGame => cache.last_team.clone(),
            LobbyPhase::Idle | LobbyPhase::NoClient => {
                // Лобби закончилось: в следующий раз загрузим свежие ранги.
                cache.profiles.clear();
                cache.last_team.clear();
                Vec::new()
            }
        };

        for player in &mut players {
            player.is_self |= cache.my_puuid.as_deref() == Some(player.puuid.as_str());
        }
        self.fill_profiles(&mut players, cache).await;
        if phase == LobbyPhase::ChampSelect {
            cache.last_team = players.clone();
        }
        LobbyState {
            phase,
            players,
            problem,
        }
    }

    /// Участники чатов Riot Client. Порт и токен читаем из аргументов LeagueClientUx.exe
    /// один раз и запоминаем; при любой ошибке забываем — в следующий раз прочитаем заново
    /// (клиент могли перезапустить).
    async fn chat_participants(&self, cache: &mut LobbyCache) -> AppResult<Vec<ChatParticipant>> {
        if cache.riot_client.is_none() {
            cache.riot_client = Some(blocking(riot_client::read_auth).await?);
        }
        let Some(auth) = &cache.riot_client else {
            return Err(AppError::LcuNotRunning);
        };
        let result = self.lcu.chat_participants(auth).await;
        if result.is_err() {
            cache.riot_client = None;
        }
        result
    }

    /// Уровень, иконка и ранги игроков (и Riot ID тех, у кого его ещё нет).
    /// Загружаются один раз за лобби — дальше из `cache.profiles`.
    async fn fill_profiles(&self, players: &mut [LobbyPlayer], cache: &mut LobbyCache) {
        for player in players.iter_mut().filter(|p| !p.puuid.is_empty()) {
            if !cache.profiles.contains_key(&player.puuid) {
                match self.player_profile(&player.puuid).await {
                    Ok(found) => {
                        cache.profiles.insert(player.puuid.clone(), Some(found));
                    }
                    // Такого игрока нет — больше не спрашиваем.
                    Err(AppError::AccountNotFound) => {
                        cache.profiles.insert(player.puuid.clone(), None);
                    }
                    // Временная ошибка — попробуем на следующем опросе.
                    Err(e) => debug!(error = %e, "профиль игрока лобби не загрузился"),
                }
            }
            if let Some(Some((riot_id, profile))) = cache.profiles.get(&player.puuid) {
                player.profile = Some(profile.clone());
                if player.riot_id.is_none() {
                    player.riot_id = riot_id.clone();
                }
            }
        }
    }

    async fn player_profile(&self, puuid: &str) -> AppResult<(Option<RiotId>, PlayerProfile)> {
        let summoner = self.lcu.summoner_by_puuid(puuid).await?;
        let ranked = self.lcu.ranked_by_puuid(puuid).await?;
        Ok((
            lobby::riot_id(&summoner.game_name, &summoner.tag_line),
            PlayerProfile {
                profile_icon_id: summoner.profile_icon_id,
                summoner_level: summoner.summoner_level,
                ranked,
            },
        ))
    }

    /// Раз в несколько секунд проверяет клиент League и сообщает окну об изменениях.
    /// Когда в клиенте появляется аккаунт из списка — сразу обновляет его данные.
    async fn watch_lcu(self) {
        let mut last = None;
        // interval — «тикает» каждые N секунд; первый тик сразу.
        let mut ticker = tokio::time::interval(LCU_POLL_INTERVAL);
        loop {
            ticker.tick().await;
            let status = self.lcu.status().await;
            if last.as_ref() == Some(&status) {
                continue; // ничего не изменилось — окно не беспокоим
            }
            match &status {
                LcuStatus::Connected { region, riot_id, .. } => {
                    info!(%region, %riot_id, "клиент League: выполнен вход");
                }
                LcuStatus::Loading => info!("клиент League загружается"),
                LcuStatus::NotRunning => info!("клиент League не запущен"),
            }
            self.events.send(Event::LcuStatus(status.clone()));
            if let LcuStatus::Connected { region, riot_id, puuid } = &status {
                self.refresh_current_account(*region, riot_id, puuid).await;
            }
            last = Some(status);
        }
    }

    /// Аккаунт, в который вошли в клиенте, есть в списке? Тогда обновляем его.
    async fn refresh_current_account(&self, region: Region, riot_id: &RiotId, puuid: &str) {
        let Ok(accounts) = self.with_storage(|s| s.list_accounts()).await else {
            return;
        };
        if let Some(account) = accounts
            .iter()
            .find(|a| is_same_account(a, region, riot_id, puuid))
        {
            self.refresh(vec![account.id]).await;
        }
    }

    /// После игры обновляет аккаунт, в который выполнен вход: окно покажет новые LP и игру.
    /// Клиенту нужно несколько секунд, чтобы получить LP и записать игру в историю, поэтому
    /// пробуем несколько раз, пока что-нибудь не изменится.
    async fn refresh_after_game(self) {
        let LcuStatus::Connected { region, riot_id, puuid } = self.lcu.status().await else {
            return;
        };
        let Ok(accounts) = self.with_storage(|s| s.list_accounts()).await else {
            return;
        };
        let Some(id) = accounts
            .iter()
            .find(|a| is_same_account(a, region, &riot_id, &puuid))
            .map(|a| a.id)
        else {
            return; // этого аккаунта нет в списке
        };
        let Ok(initial) = self.with_storage(move |s| s.get_account(id)).await else {
            return;
        };
        for delay in AFTER_GAME_RETRIES {
            tokio::time::sleep(delay).await;
            self.refresh(vec![id]).await;
            let Ok(after) = self.with_storage(move |s| s.get_account(id)).await else {
                return;
            };
            if game_recorded(&initial, &after) {
                return;
            }
            debug!(id, "клиент ещё не записал игру или новые LP — попробую ещё раз");
        }
    }

    /// Работа с базой и хранилищем паролей (блокирующая — поэтому через `blocking`).
    async fn with_storage<T, F>(&self, f: F) -> AppResult<T>
    where
        F: FnOnce(&mut Storage) -> AppResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let storage = Arc::clone(&self.storage);
        blocking(move || {
            // lock() вернёт Err, только если другой поток запаниковал, держа замок.
            // Данные при этом целы (SQLite сам откатывает незавершённые транзакции),
            // поэтому не падаем, а берём их (into_inner).
            let mut storage = storage.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut storage)
        })
        .await
    }

    /// Каждая команда выполняется отдельной задачей: долгое «Обновить все»
    /// не задерживает, например, копирование пароля.
    fn handle(&self, command: Command) {
        // Задача может жить дольше этого вызова, поэтому ей нужна СВОЯ копия
        // worker (`move`), а не ссылка на self — иначе компилятор не пропустит.
        let worker = self.clone();
        tokio::spawn(async move {
            match command {
                Command::ReloadAccounts => worker.reload_accounts().await,
                Command::AddAccount { input, credentials } => {
                    worker.add_account(input, credentials).await
                }
                Command::UpdateAccount {
                    id,
                    input,
                    login,
                    password,
                } => worker.update_account(id, input, login, password).await,
                Command::DeleteAccount(id) => worker.delete_account(id).await,
                Command::RefreshAccount(id) => {
                    worker.refresh(vec![id]).await;
                }
                Command::RefreshAll => worker.refresh_all().await,
                Command::CopyCredential { id, kind } => worker.copy_credential(id, kind).await,
                Command::StartLogin(id) => worker.start_login(id).await,
                Command::LoadImage(key) => match worker.images.load(key).await {
                    Ok(bytes) => worker.events.send(Event::ImageLoaded { key, bytes }),
                    Err(error) => {
                        warn!(?key, %error, "картинка не загрузилась");
                        worker.events.send(Event::ImageFailed(key));
                    }
                },
                Command::SetLeagueDir(dir) => {
                    let result = worker.set_league_dir(dir).await;
                    worker.events.send(Event::LeagueDirSaved(result));
                }
                Command::CheckApiKey(candidate) => {
                    let result = worker.check_api_key(candidate).await;
                    worker.events.send(Event::ApiKeyChecked(result));
                }
                Command::SaveApiKey(raw) => {
                    let result = worker.save_api_key(raw).await;
                    worker.events.send(Event::ApiKeySaved(result));
                }
                Command::DeleteApiKey => {
                    let result = worker.delete_api_key().await;
                    worker.events.send(Event::ApiKeyDeleted(result));
                }
                Command::ExportAccounts(path) => {
                    let result = worker.export_accounts(path).await;
                    worker.events.send(Event::Exported(result));
                }
                Command::ImportAccounts(path) => worker.import_accounts(path).await,
                Command::LoadMatch { account, match_id } => {
                    let result = worker.load_match(account, &match_id).await;
                    if let Err(error) = &result {
                        warn!(%match_id, %error, "подробности игры не загрузились");
                    }
                    worker.events.send(Event::MatchLoaded { match_id, result });
                }
                Command::LoadGames(account) => {
                    let result = worker.with_storage(move |s| s.archived_matches(account, None)).await;
                    worker.events.send(Event::GamesLoaded { account, result });
                }
            }
        });
    }

    // ----- Экспорт и импорт списка ------------------------------------------

    /// Список из базы → файл JSON (только несекретное, см. transfer.rs).
    async fn export_accounts(&self, path: PathBuf) -> AppResult<usize> {
        let accounts = self.with_storage(|s| s.list_accounts()).await?;
        let json = transfer::export_json(&accounts, unix_now());
        tokio::fs::write(&path, json).await?;
        info!(count = accounts.len(), "список аккаунтов экспортирован");
        Ok(accounts.len())
    }

    /// Файл → новые аккаунты (без логинов и паролей). Уже существующие пропускаются.
    /// Добавленные сразу ставим на обновление: ранги и игры загрузятся сами.
    async fn import_accounts(&self, path: PathBuf) {
        let result: AppResult<(ImportReport, Vec<AccountId>)> = async {
            if tokio::fs::metadata(&path).await?.len() > transfer::MAX_IMPORT_BYTES {
                return Err(AppError::ImportTooLarge);
            }
            let bytes = tokio::fs::read(&path).await?;
            let plan = transfer::parse_import(&bytes)?;
            let invalid = plan.invalid;
            let (added, duplicates) = self
                .with_storage(move |s| {
                    let (mut added, mut duplicates) = (Vec::new(), 0);
                    for input in plan.accounts {
                        match s.add_account(&input, Credentials::default()) {
                            Ok(account) => added.push(account.id),
                            Err(AppError::DuplicateAccount) => duplicates += 1,
                            Err(e) => return Err(e),
                        }
                    }
                    Ok((added, duplicates))
                })
                .await?;
            let report = ImportReport {
                added: added.len(),
                duplicates,
                invalid,
            };
            Ok((report, added))
        }
        .await;

        match result {
            Ok((report, added)) => {
                info!(?report, "список аккаунтов импортирован");
                self.reload_accounts().await;
                self.events.send(Event::Imported(Ok(report)));
                if !added.is_empty() {
                    self.refresh(added).await;
                }
            }
            Err(error) => {
                // Импорт мог прерваться на середине — показываем то, что успело добавиться.
                self.reload_accounts().await;
                self.events.send(Event::Imported(Err(error)));
            }
        }
    }

    // ----- Аккаунты ---------------------------------------------------------

    async fn reload_accounts(&self) {
        match self.with_storage(|s| s.list_accounts()).await {
            Ok(accounts) => self.events.send(Event::Accounts(accounts)),
            Err(e) => self.events.send(Event::Warning(e)),
        }
    }

    async fn add_account(&self, input: AccountInput, credentials: Credentials) {
        match self.try_add(input, credentials).await {
            Ok((account, fetch_error)) => {
                info!(id = account.id, "аккаунт добавлен");
                self.events.send(Event::AccountAdded {
                    account,
                    fetch_error,
                });
            }
            Err(e) => self.events.send(Event::AddFailed(e)),
        }
    }

    async fn try_add(
        &self,
        input: AccountInput,
        credentials: Credentials,
    ) -> AppResult<(Account, Option<AppError>)> {
        // 1. Сначала проверяем, есть ли такой аккаунт (через клиент League или Riot API):
        //    опечатка в Riot ID не должна превратиться в пустую карточку.
        let target = Target {
            riot_id: &input.riot_id,
            region: input.region,
            puuid: None,
            lcu_puuid: None,
            known_matches: &[],
        };
        let (snapshot, fetch_error) = match self.fetch_snapshot(&target, None).await {
            Ok(mut snapshot) => {
                self.name_champions(&mut snapshot).await;
                (Some(snapshot), None)
            }
            // `e @ (...)` — проверить, что ошибка одна из перечисленных, и назвать её `e`.
            Err(e @ (AppError::AccountNotFound | AppError::SummonerNotFound(_) | AppError::BadRequest)) => {
                return Err(e);
            }
            // Нет клиента и ключа, нет сети, лимит — аккаунт всё равно сохраняем, обновим позже.
            Err(e) => (None, Some(e)),
        };

        // 2. Сохраняем. Все шаги под одним замком: никто не вклинится между ними.
        let account = self
            .with_storage(move |s| {
                let account = s.add_account(&input, credentials)?;
                if let Some(snapshot) = snapshot {
                    // Например, этот же аккаунт уже есть под старым Riot ID (тот же PUUID).
                    // Тогда откатываем добавление вместе с логином и паролем.
                    if let Err(e) = s.save_snapshot(account.id, &snapshot) {
                        let _ = s.delete_account(account.id);
                        return Err(e);
                    }
                }
                s.get_account(account.id)
            })
            .await?;
        Ok((account, fetch_error))
    }

    async fn update_account(
        &self,
        id: AccountId,
        input: AccountInput,
        login: SecretChange,
        password: SecretChange,
    ) {
        let result = self
            .with_storage(move |s| s.update_account(id, &input, login, password))
            .await;
        match result {
            Ok(account) => {
                // Сменили Riot ID или регион — старые данные стёрты, загружаем новые.
                let needs_refresh = account.cache.is_none();
                self.events.send(Event::AccountEdited(account));
                if needs_refresh {
                    self.refresh(vec![id]).await;
                }
            }
            Err(error) => self.events.send(Event::EditFailed { id, error }),
        }
    }

    async fn delete_account(&self, id: AccountId) {
        match self.with_storage(move |s| s.delete_account(id)).await {
            Ok(()) => {
                info!(id, "аккаунт удалён");
                self.events.send(Event::AccountDeleted(id));
            }
            Err(error) => self.events.send(Event::DeleteFailed { id, error }),
        }
    }

    // ----- Обновление из Riot API -------------------------------------------

    async fn refresh_all(&self) {
        // Ни клиента, ни ключа — каждый аккаунт вернул бы одну и ту же ошибку.
        if !self.riot.has_api_key() && self.lcu.region().await.is_err() {
            self.events.send(Event::Warning(AppError::NoDataSource));
            self.events.send(Event::RefreshAllDone { ok: 0, failed: 0 });
            return;
        }
        let ids = match self.with_storage(|s| s.list_accounts()).await {
            Ok(accounts) => accounts.iter().map(|a| a.id).collect(),
            Err(e) => {
                self.events.send(Event::Warning(e));
                self.events.send(Event::RefreshAllDone { ok: 0, failed: 0 });
                return;
            }
        };
        let (ok, failed) = self.refresh(ids).await;
        info!(ok, failed, "обновление всех аккаунтов завершено");
        self.events.send(Event::RefreshAllDone { ok, failed });
    }

    /// Обновляет аккаунты. Возвращает (успешно, с ошибкой).
    async fn refresh(&self, ids: Vec<AccountId>) -> (usize, usize) {
        // Если API-ключ оказался неверным, остальные запросы к Riot API не отправляем:
        // каждый вернул бы ту же ошибку (а через клиент League обновление продолжится).
        // OnceLock — ячейка, в которую можно записать только один раз.
        let api_failed: Arc<OnceLock<AppError>> = Arc::default();
        let mut tasks = JoinSet::new();

        for id in ids {
            // let-else: аккаунт уже обновляется — guard не выдадут, пропускаем.
            let Some(guard) = RefreshGuard::acquire(&self.refreshing, id) else {
                continue;
            };
            self.events.send(Event::RefreshQueued(id));

            let worker = self.clone();
            let api_failed = Arc::clone(&api_failed);
            tasks.spawn(async move {
                let _guard = guard; // снимет отметку «обновляется», когда задача закончится
                // Ждём свободный пропуск. Семафор никогда не закрывается, поэтому Ok всегда.
                let _slot = worker.refresh_slots.acquire().await.ok();
                match worker.refresh_one(id, api_failed.get()).await {
                    Ok((account, source)) => {
                        worker.events.send(Event::AccountRefreshed { account, source });
                        true
                    }
                    Err(error) => {
                        if error == AppError::InvalidApiKey {
                            let _ = api_failed.set(error.clone());
                        }
                        warn!(id, error = %error, "не удалось обновить аккаунт");
                        worker.events.send(Event::RefreshFailed { id, error });
                        false
                    }
                }
            });
        }

        // join_next() отдаёт задачи по мере завершения.
        let (mut ok, mut failed) = (0, 0);
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(true) => ok += 1,
                _ => failed += 1,
            }
        }
        (ok, failed)
    }

    /// Один аккаунт: запросы к клиенту League или Riot API и запись в кэш.
    /// Возвращает обновлённый аккаунт и источник данных.
    async fn refresh_one(
        &self,
        id: AccountId,
        api_failed: Option<&AppError>,
    ) -> AppResult<(Account, DataSource)> {
        let before = self.with_storage(move |s| s.get_account(id)).await?;
        // Игры, которые уже есть в архиве, через Riot API заново не загружаем.
        let known = self
            .with_storage(move |s| s.archived_matches(id, Some(KNOWN_MATCHES)))
            .await
            .unwrap_or_default();
        let target = Target {
            riot_id: &before.riot_id,
            region: before.region,
            puuid: before.puuid.as_deref(),
            lcu_puuid: before.lcu_puuid.as_deref(),
            known_matches: &known,
        };
        let mut snapshot = self.fetch_snapshot(&target, api_failed).await?;
        self.name_champions(&mut snapshot).await;
        let source = snapshot.source;

        let account = self
            .with_storage(move |s| {
                // Пока шли запросы, аккаунт могли изменить. Если сменились Riot ID или регион,
                // ответ уже про другой аккаунт — не сохраняем его поверх правки.
                let current = s.get_account(id)?;
                if current.riot_id != before.riot_id || current.region != before.region {
                    return Ok(current);
                }
                s.save_snapshot(id, &snapshot)?;
                s.get_account(id)
            })
            .await?;
        // Клиент League отдаёт обе команды отдельным запросом на игру — догружаем их
        // в архив в фоне, чтобы не задерживать обновление (запросы локальные, быстрые).
        if source == DataSource::Lcu {
            tokio::spawn(self.clone().archive_details(id));
        }
        Ok((account, source))
    }

    /// Подробности (обе команды) игр архива, у которых их ещё нет, — из клиента League.
    /// Не больше [`DETAILS_PER_REFRESH`] за раз: остальные — при следующих обновлениях.
    async fn archive_details(self, id: AccountId) {
        let Ok(account) = self.with_storage(move |s| s.get_account(id)).await else {
            return;
        };
        let Ok(missing) = self.with_storage(move |s| s.matches_without_details(id, DETAILS_PER_REFRESH)).await else {
            return;
        };
        let mut saved = 0;
        for match_id in missing {
            match self.match_via_lcu(&account, &match_id).await {
                Ok(mut details) => {
                    self.fill_champion_names(details.players_mut().map(|p| &mut p.stats)).await;
                    if self.with_storage(move |s| s.save_details(id, &details)).await.is_ok() {
                        saved += 1;
                    }
                }
                // Клиент закрыли или сменили сервер — продолжим в другой раз.
                Err(AppError::LcuNotRunning | AppError::WrongClientRegion { .. }) => break,
                Err(e) => debug!(%match_id, error = %e, "подробности игры для архива не загрузились"),
            }
        }
        if saved > 0 {
            debug!(id, saved, "подробности игр сохранены в архив");
        }
    }

    /// Получить данные аккаунта из лучшего доступного источника:
    /// 1. клиент League — если он запущен на том же сервере, что и аккаунт;
    /// 2. иначе Riot API — если задан ключ;
    /// 3. иначе — понятная ошибка: что запустить или что настроить.
    async fn fetch_snapshot(
        &self,
        target: &Target<'_>,
        api_failed: Option<&AppError>,
    ) -> AppResult<AccountSnapshot> {
        let lcu_error = match self.lcu.region().await {
            Ok(client_region) if client_region == target.region => {
                match self.fetch_via_lcu(target).await {
                    Ok(snapshot) => return Ok(snapshot),
                    // Клиент ответил «нет такого игрока» — Riot API скажет то же самое.
                    Err(e @ AppError::AccountNotFound) => return Err(e),
                    // Клиент закрылся посреди запроса и т.п. — попробуем Riot API.
                    Err(e) => e,
                }
            }
            Ok(client_region) => AppError::WrongClientRegion {
                client: client_region.label(),
                account: target.region.label(),
            },
            Err(e) => e,
        };

        if !self.riot.has_api_key() {
            // Клиент просто не запущен — подсказываем оба варианта.
            return Err(match lcu_error {
                AppError::LcuNotRunning => AppError::NoDataSource,
                other => other,
            });
        }
        if let Some(error) = api_failed {
            return Err(error.clone());
        }
        debug!(region = %target.region, reason = %lcu_error, "клиент League не подходит, идём в Riot API");
        self.fetch_via_api(target).await
    }

    async fn fetch_via_lcu(&self, target: &Target<'_>) -> AppResult<AccountSnapshot> {
        let mut snapshot = self.lcu_snapshot(target).await?;
        // Клиент не показывает поражения аккаунтов, в которые в нём не выполнен вход.
        // С API-ключом берём настоящие победы и поражения из Riot API (1–2 запроса);
        // не получилось — остаёмся с данными клиента (поражения «скрыты»).
        if snapshot.ranked.has_hidden_losses() && self.riot.has_api_key() {
            match self.api_ranked(target).await {
                Ok(ranked) => snapshot.ranked = ranked,
                Err(e) => debug!(error = %e, "поражения из Riot API не загрузились"),
            }
        }
        Ok(snapshot)
    }

    async fn lcu_snapshot(&self, target: &Target<'_>) -> AppResult<AccountSnapshot> {
        if let Some(puuid) = target.lcu_puuid {
            // По PUUID находим игрока, даже если он сменил Riot ID.
            match self.lcu.fetch_by_puuid(puuid, target.riot_id, target.region).await {
                Err(AppError::AccountNotFound) => {} // ниже попробуем по Riot ID
                other => return other,
            }
        }
        self.lcu.fetch_by_riot_id(target.riot_id, target.region).await
    }

    /// Только ранги — через Riot API (PUUID API известен — 1 запрос, иначе 2).
    async fn api_ranked(&self, target: &Target<'_>) -> AppResult<RankedQueues> {
        if let Some(puuid) = target.puuid {
            match self.riot.ranked_by_puuid(puuid, target.region).await {
                // PUUID «чужой» (сменили ключ) — найдём заново по Riot ID.
                Err(AppError::BadRequest | AppError::AccountNotFound) => {}
                other => return other,
            }
        }
        let account = self.riot.account_by_riot_id(target.riot_id, target.region).await?;
        self.riot.ranked_by_puuid(&account.puuid, target.region).await
    }

    async fn fetch_via_api(&self, target: &Target<'_>) -> AppResult<AccountSnapshot> {
        if let Some(puuid) = target.puuid {
            match self
                .riot
                .fetch_by_puuid(puuid, target.riot_id, target.region, target.known_matches)
                .await
            {
                // PUUID Riot API зашифрован под конкретный проект. Если сменить ключ
                // разработчика на Personal API Key, старый PUUID станет «чужим»
                // и Riot ответит 400. Тогда ищем аккаунт заново по Riot ID.
                Err(AppError::BadRequest | AppError::AccountNotFound) => {}
                other => return other,
            }
        }
        self.riot.fetch_by_riot_id(target.riot_id, target.region).await
    }

    /// Русские названия чемпионов в последних играх (для подсказок на строке аккаунта).
    /// Не получилось с CommunityDragon — берём из клиента League (на его языке);
    /// нет и его — названия останутся пустыми, на строке будет «чемпион #id».
    async fn name_champions(&self, snapshot: &mut AccountSnapshot) {
        let Some(matches) = &mut snapshot.matches else {
            return;
        };
        self.fill_champion_names(matches.iter_mut().map(|m| &mut m.me)).await;
    }

    /// Подставляет названия чемпионов туда, где их ещё нет.
    /// `impl Iterator<Item = &mut PlayerStats>` — любой перебор изменяемых ссылок:
    /// и игры одного аккаунта, и десять игроков из подробностей игры.
    async fn fill_champion_names<'a>(&self, players: impl Iterator<Item = &'a mut PlayerStats>) {
        let mut missing: Vec<&mut PlayerStats> = players.filter(|p| p.champion.is_empty()).collect();
        if missing.is_empty() {
            return;
        }
        let names = match self.images.champion_names().await {
            Ok(names) => names,
            Err(e) => match self.lcu.champion_names().await {
                Ok(names) => Arc::new(names),
                Err(_) => {
                    debug!(error = %e, "названия чемпионов не загрузились");
                    return;
                }
            },
        };
        for player in &mut missing {
            if let Some(name) = names.get(&player.champion_id) {
                player.champion = name.clone();
            }
        }
    }

    // ----- Подробности игры -------------------------------------------------

    /// Обе команды одной игры: из архива, если она там уже есть; иначе из клиента League
    /// (если он на сервере этой игры) или через Riot API — и в архив.
    async fn load_match(&self, account_id: AccountId, match_id: &str) -> AppResult<MatchDetails> {
        let key = match_id.to_string();
        if let Ok(Some(details)) = self.with_storage(move |s| s.archived_details(account_id, &key)).await {
            return Ok(details);
        }
        let account = self.with_storage(move |s| s.get_account(account_id)).await?;
        let mut details = match self.match_via_lcu(&account, match_id).await {
            Ok(details) => details,
            Err(lcu_error) => {
                if !self.riot.has_api_key() {
                    // Клиент просто не запущен — подсказываем оба варианта.
                    return Err(match lcu_error {
                        AppError::LcuNotRunning | AppError::WrongClientRegion { .. } => AppError::NoDataSource,
                        other => other,
                    });
                }
                debug!(reason = %lcu_error, "подробности игры — через Riot API");
                let game = self.riot.match_by_id(match_id, account.region).await?;
                game.details(account.puuid.as_deref())
            }
        };
        self.fill_champion_names(details.players_mut().map(|p| &mut p.stats)).await;
        let saved = details.clone();
        // Не сохранилось — не страшно: окно всё равно покажет подробности.
        if let Err(e) = self.with_storage(move |s| s.save_details(account_id, &saved)).await {
            debug!(error = %e, "подробности игры не сохранились в архив");
        }
        Ok(details)
    }

    async fn match_via_lcu(&self, account: &Account, match_id: &str) -> AppResult<MatchDetails> {
        let client_region = self.lcu.region().await?;
        if client_region != account.region {
            return Err(AppError::WrongClientRegion {
                client: client_region.label(),
                account: account.region.label(),
            });
        }
        let game_id = MatchSummary::game_id(match_id).ok_or(AppError::MatchNotFound)?;
        let game = self.lcu.match_details(game_id).await?;
        Ok(game.details(account.lcu_puuid.as_deref()))
    }

    // ----- Буфер обмена -----------------------------------------------------

    /// Логин/пароль идут из хранилища паролей прямо в буфер обмена, минуя окно.
    async fn copy_credential(&self, id: AccountId, kind: CredentialKind) {
        if let Some(generation) = self.copy_and_notify(id, kind).await {
            self.clear_clipboard_later(generation).await;
        }
    }

    /// Кнопка «Войти»: логин в буфер, затем запуск Riot Client.
    /// В сам клиент ничего не вводится — пользователь вставляет логин и пароль сам.
    async fn start_login(&self, id: AccountId) {
        let Some(generation) = self.copy_and_notify(id, CredentialKind::Login).await else {
            return; // логина нет — клиент запускать незачем, ошибка уже в окне
        };
        match blocking(launcher::launch_riot_client).await {
            Ok(true) => self.events.send(Event::RiotClientLaunched),
            Ok(false) => {} // не Windows: логин скопирован, клиент пользователь откроет сам
            Err(e) => self.events.send(Event::Warning(e)),
        }
        self.clear_clipboard_later(generation).await;
    }

    /// Копирует и сообщает окну. `None` — не получилось (окно уже получило `CopyFailed`).
    async fn copy_and_notify(&self, id: AccountId, kind: CredentialKind) -> Option<u64> {
        match self.try_copy(id, kind).await {
            Ok(generation) => {
                self.events.send(Event::CredentialCopied {
                    kind,
                    clear_after_secs: self.clipboard_clear_after.as_secs(),
                });
                Some(generation)
            }
            Err(e) => {
                self.events.send(Event::CopyFailed(e));
                None
            }
        }
    }

    /// Ждёт и стирает буфер, если там всё ещё наш секрет.
    async fn clear_clipboard_later(&self, generation: u64) {
        // sleep в tokio не занимает поток: задача просто «спит».
        tokio::time::sleep(self.clipboard_clear_after).await;
        let clipboard = self.clipboard.clone();
        let cleared = blocking(move || Ok(clipboard.clear_if_ours(Some(generation)))).await;
        if matches!(cleared, Ok(true)) {
            self.events.send(Event::ClipboardCleared);
        }
    }

    async fn try_copy(&self, id: AccountId, kind: CredentialKind) -> AppResult<u64> {
        let credentials = self.with_storage(move |s| s.credentials(id)).await?;
        // Забираем нужное поле во владение, второе уничтожится вместе с credentials.
        let secret = match kind {
            CredentialKind::Login => credentials.login,
            CredentialKind::Password => credentials.password,
        }
        .ok_or(AppError::CredentialMissing(kind.label()))?;
        let clipboard = self.clipboard.clone();
        blocking(move || clipboard.copy_secret(secret)).await
    }

    // ----- Папка игры -------------------------------------------------------

    /// Сохранить папку игры (`None` — искать автоматически) и сразу искать клиент там.
    async fn set_league_dir(&self, dir: Option<String>) -> AppResult<LeagueDirInfo> {
        // «Копировать как путь» в Проводнике добавляет кавычки — убираем их.
        let dir = dir
            .map(|d| d.trim().trim_matches('"').trim().to_string())
            .filter(|d| !d.is_empty());
        let (info, dirs) = self
            .with_storage(move |s| {
                match &dir {
                    Some(path) => {
                        if !Path::new(path).is_dir() {
                            return Err(AppError::LeagueDirNotFound(path.clone()));
                        }
                        if !lockfile::is_install_dir(Path::new(path)) {
                            return Err(AppError::LeagueDirInvalid);
                        }
                        s.set_setting(LEAGUE_DIR_SETTING, path)?;
                    }
                    None => s.delete_setting(LEAGUE_DIR_SETTING)?,
                }
                Ok(league_dirs(dir))
            })
            .await?;
        // Новые папки сразу видит и наблюдатель за клиентом: у клонов LcuClient общий список.
        self.lcu.set_dirs(dirs);
        info!(custom = ?info.custom, found = ?info.found, "папка игры");
        Ok(info)
    }

    // ----- API-ключ ---------------------------------------------------------

    /// Проверить введённый ключ (`Some`) или сохранённый (`None`).
    async fn check_api_key(&self, candidate: Option<Secret>) -> AppResult<()> {
        let client = match candidate {
            // Введённый ключ проверяем отдельным клиентом: сохранённый не меняется.
            Some(raw) => self.riot.clone().with_api_key(ApiKey::new(raw.expose())?),
            None => self.riot.clone(),
        };
        client.check_api_key(KEY_CHECK_REGION).await
    }

    async fn save_api_key(&self, raw: Secret) -> AppResult<()> {
        let key = ApiKey::new(raw.expose())?;
        // Сначала проверяем в Riot: нерабочий ключ сохранять незачем.
        self.riot
            .clone()
            .with_api_key(key.clone())
            .check_api_key(KEY_CHECK_REGION)
            .await?;
        self.with_storage(move |s| s.set_api_key(raw.expose())).await?;
        self.riot.set_api_key(Some(key));
        info!("API-ключ сохранён");
        Ok(())
    }

    async fn delete_api_key(&self) -> AppResult<()> {
        self.with_storage(|s| s.delete_api_key()).await?;
        self.riot.set_api_key(None);
        info!("API-ключ удалён");
        Ok(())
    }
}

/// Что наблюдатель за лобби помнит между опросами.
#[derive(Default)]
struct LobbyCache {
    /// Порт и токен Riot Client (из аргументов LeagueClientUx.exe).
    riot_client: Option<RiotClientAuth>,
    my_puuid: Option<String>,
    /// id чемпиона → название (на языке клиента).
    champions: HashMap<i32, String>,
    /// PUUID → (Riot ID, профиль); `None` — игрок не найден.
    profiles: HashMap<String, Option<(Option<RiotId>, PlayerProfile)>>,
    /// Команда из последнего выбора чемпионов — показываем её и во время игры.
    last_team: Vec<LobbyPlayer>,
    /// На прошлом опросе шла игра.
    game_running: bool,
    /// Игра только что закончилась — пора обновить аккаунт (забирает `watch_lobby`).
    game_ended: bool,
}

/// Где искать клиент League с учётом папки из настроек, и где игра нашлась.
/// Блокирующая функция (проверяет диск).
fn league_dirs(custom: Option<String>) -> (LeagueDirInfo, Vec<PathBuf>) {
    let dirs = lockfile::candidate_dirs(custom.as_deref().map(Path::new));
    let found = lockfile::find_install_dir(&dirs);
    (LeagueDirInfo { custom, found }, dirs)
}

/// Какой аккаунт ищем. Поля — ссылки (`&'a`), а не копии: структура живёт недолго,
/// пока идут запросы, и только читает данные аккаунта. `'a` говорит компилятору:
/// «Target не переживёт данные, на которые ссылается».
struct Target<'a> {
    riot_id: &'a RiotId,
    region: Region,
    /// PUUID Riot API (если уже известен).
    puuid: Option<&'a str>,
    /// PUUID клиента League (если уже известен).
    lcu_puuid: Option<&'a str>,
    /// Последние игры из кэша: через Riot API их заново не загружаем.
    known_matches: &'a [MatchSummary],
}

/// Клиент уже записал игру: в истории появилась новая, а если она рейтинговая —
/// изменился и ранг её очереди (LP и игра в истории приходят не одновременно).
fn game_recorded(before: &Account, after: &Account) -> bool {
    let newest = |a: &Account| a.cache.as_ref()?.matches.as_ref()?.first().cloned();
    let Some(game) = newest(after) else {
        return false;
    };
    if newest(before).is_some_and(|old| old.id == game.id) {
        return false; // новой игры ещё нет
    }
    match Queue::from_queue_id(game.queue_id) {
        Some(queue) => {
            let stats = |a: &Account| a.cache.as_ref().and_then(|c| queue.stats(&c.ranked).cloned());
            stats(before) != stats(after)
        }
        None => true, // не рейтинговая — ранг и не должен меняться
    }
}

/// Это тот же аккаунт, что сейчас в клиенте League?
/// По PUUID клиента — надёжно, по Riot ID — если PUUID ещё не известен.
pub fn is_same_account(account: &Account, region: Region, riot_id: &RiotId, lcu_puuid: &str) -> bool {
    if account.region != region {
        return false;
    }
    match &account.lcu_puuid {
        Some(known) => known == lcu_puuid,
        None => {
            account.riot_id.game_name.to_lowercase() == riot_id.game_name.to_lowercase()
                && account.riot_id.tag_line.to_lowercase() == riot_id.tag_line.to_lowercase()
        }
    }
}

/// Отметка «аккаунт обновляется».
///
/// Пока guard жив, второй для того же аккаунта не выдаётся. Когда guard уничтожается
/// (задача закончилась успешно, с ошибкой или была прервана при выходе), `Drop` сам
/// снимает отметку. Забыть её снять невозможно — это и есть идиома RAII.
struct RefreshGuard {
    set: Arc<Mutex<HashSet<AccountId>>>,
    id: AccountId,
}

impl RefreshGuard {
    fn acquire(set: &Arc<Mutex<HashSet<AccountId>>>, id: AccountId) -> Option<Self> {
        // insert() возвращает false, если такой id уже есть.
        let inserted = set.lock().unwrap_or_else(PoisonError::into_inner).insert(id);
        // bool::then — «если true, то Some(значение), иначе None».
        inserted.then(|| Self {
            set: Arc::clone(set),
            id,
        })
    }
}

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::{Division, RankedStats, Tier};
    use crate::storage::CachedProfile;

    /// Аккаунт с рангом Solo/Duo (LP) и последней игрой (id, очередь).
    fn account_after(lp: i32, game: (&str, i32)) -> Account {
        let summary = MatchSummary {
            id: game.0.into(),
            win: true,
            remake: false,
            queue_id: game.1,
            played_at: 0,
            duration_secs: 1800,
            stats_version: crate::riot::models::MATCH_STATS_VERSION,
            me: PlayerStats::default(),
        };
        Account {
            id: 1,
            riot_id: RiotId::parse("Name#RU1").unwrap(),
            region: Region::Ru,
            puuid: None,
            lcu_puuid: None,
            note: String::new(),
            tags: vec![],
            secret_ref: String::new(),
            has_login: false,
            has_password: false,
            created_at: 0,
            cache: Some(CachedProfile {
                profile_icon_id: 1,
                summoner_level: 30,
                ranked: RankedQueues {
                    solo: Some(RankedStats {
                        tier: Tier::Gold,
                        division: Division::Two,
                        lp,
                        wins: 10,
                        losses: Some(10),
                    }),
                    flex: None,
                },
                matches: Some(vec![summary]),
                fetched_at: 0,
            }),
            history: vec![],
            archived_games: 0,
        }
    }

    #[test]
    fn game_is_recorded_when_both_game_and_lp_arrive() {
        let before = account_after(40, ("RU_1", 420));
        // Клиент уже отдал LP, а игру в истории ещё нет — ждём дальше.
        assert!(!game_recorded(&before, &account_after(59, ("RU_1", 420))));
        // Игра есть, но LP ещё старые — тоже ждём.
        assert!(!game_recorded(&before, &account_after(40, ("RU_2", 420))));
        assert!(game_recorded(&before, &account_after(59, ("RU_2", 420))));
        // ARAM: ранг не меняется — хватает самой игры.
        assert!(game_recorded(&before, &account_after(40, ("RU_2", 450))));
    }

    #[test]
    fn refresh_guard_blocks_duplicates_until_dropped() {
        let set = Arc::default();
        let first = RefreshGuard::acquire(&set, 7);
        assert!(first.is_some());
        assert!(RefreshGuard::acquire(&set, 7).is_none(), "второй guard не выдаётся");
        assert!(RefreshGuard::acquire(&set, 8).is_some(), "другой аккаунт — можно");
        drop(first);
        assert!(RefreshGuard::acquire(&set, 7).is_some(), "после drop снова можно");
    }
}
