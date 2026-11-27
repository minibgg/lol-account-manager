//! Главная структура приложения: состояние окна и связь с фоновым потоком.
//!
//! Окно ничего не ждёт: кнопки отправляют [`Command`] в фоновый поток, а результаты
//! приходят как [`Event`] и забираются в начале каждого кадра (метод `logic`).
//!
//! Части интерфейса живут в `ui/`: карточка аккаунта, форма, поиск и сортировка,
//! картинки, тема. Здесь — состояние окна, страницы и обработка событий.

use std::collections::{HashMap, HashSet};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Margin, RichText};

use crate::autostart;
use crate::decay;
use crate::notify::{self, Message, Notifier};
use crate::backend::{Backend, is_same_account};
use crate::clash::{ClashHistory, ClashState, HistoryRequest};
use crate::error::AppError;
use crate::hello;
use crate::hotkey::{self, GlobalHotkey};
use crate::history::{self, Queue, Session};
use crate::images::ImageKey;
use crate::lcu::LcuStatus;
use crate::lobby::{self, LiveGame, LobbyPhase, LobbyState};
use crate::messages::{Command, CredentialKind, Event, LeagueDirInfo, Secret};
use crate::riot::models::RECENT_MATCHES;
use crate::riot::{Division, Inventory, MatchDetails, MatchSummary, RankedQueues, Region, RiotId};
use crate::storage::{Account, AccountId, AccountInput, SecretChange, unix_now};
use crate::single_instance;
use crate::transfer;
use crate::tray::{self, Tray, TrayAction};
use crate::update::{self, UpdateInfo, Version};
use crate::vault;
use crate::ui::account_card::CardAction;
use crate::ui::account_form::{AccountForm, FormAction};
use crate::ui::account_list::{self, ListView, SortOrder};
use crate::ui::collection_view::CollectionView;
use crate::ui::image_store::ImageStore;
use crate::ui::theme::{self, Weight};
use crate::ui::widgets::{self, Button, Icon, Tone};
use crate::ui::match_view::{DialogAction, MatchDialog, Ranks};
use crate::ui::overview_view::{OverviewAction, OverviewView};
use crate::ui::post_game::PostGame;
use crate::ui::profile_view::ProfileView;
use crate::ui::rank_history::{self, HistoryDialog};
use crate::ui::command::{self, Invocation, Suggestion, Target};
use crate::ui::games_view::{GamesAction, GamesView, HistoryLoad};
use crate::ui::keymap::{self, Action, Screen};
use crate::ui::terminal::{self, CommandEvent, CommandLine, Hint};
use crate::ui::clash_history_view::{self, HistoryAction};
use crate::ui::{clash_view, lobby_view, wallpapers_view, window_frame};

/// Загрузка истории: открытые окна перечитывают архив после каждых стольких новых игр.
const HISTORY_RELOAD_EVERY: usize = 20;

/// Сколько сообщение видно в нижней строке.
const NOTICE_DURATION: Duration = Duration::from_secs(8);

/// Сколько ждём второе нажатие («Пароль») после «Войти».
const LOGIN_FLOW_TIMEOUT: Duration = Duration::from_secs(120);

/// Ключ сортировки в window.ron.
const SORT_KEY: &str = "sort";
/// «Сворачивать в трей при закрытии» в window.ron: «1» — включено. Хранится там, а не
/// в базе, потому что нужно сразу при создании окна, до ответа фонового потока.
const TRAY_KEY: &str = "close_to_tray";
/// Ключ в том же файле: показывать ли уведомления Windows («0» — нет).
const NOTIFY_KEY: &str = "notifications";
/// Ключ в window.ron: глобальная клавиша Ctrl+Shift+L («0» — выключена).
const HOTKEY_KEY: &str = "global_hotkey";
/// Сколько ждём после игры, прежде чем показать уведомление Windows: клиент League
/// отдаёт новые LP и саму игру не одновременно — собираем всё в одно уведомление.
const TOAST_DELAY: Duration = Duration::from_secs(8);
/// Через сколько после запуска проверяем, не сгорает ли у какого-то аккаунта ранг.
const DECAY_CHECK_DELAY: Duration = Duration::from_secs(4);

/// Что `main` передаёт приложению при запуске.
pub struct Startup {
    /// Порт, по которому вторая копия программы просит показать окно
    /// (см. single_instance.rs). `None` — проверки второй копии нет.
    pub listener: Option<TcpListener>,
    /// Программу запустила Windows при входе в систему (а не пользователь).
    pub by_system: bool,
}

/// Трей и автозапуск.
struct DesktopState {
    /// Значок в трее. `Some` — «при закрытии сворачивать в трей» включено.
    tray: Option<Tray>,
    /// Глобальная клавиша Ctrl+Shift+L. `Some` — включена и зарегистрирована.
    hotkey: Option<GlobalHotkey>,
    /// Запуск вместе с Windows (как записано в реестре).
    autostart: bool,
    /// Спрятать окно в первом же кадре (запуск вместе с Windows сразу в трей).
    hide_on_start: bool,
    /// Выходим по-настоящему: закрытие окна больше не перехватываем.
    quitting: bool,
    /// Вторая копия программы (или щелчок по уведомлению) просит показать окно.
    show_requests: Receiver<()>,
    /// Уведомления Windows. `None` — на этой системе их нет.
    notifier: Option<Notifier>,
    /// Галочка «уведомления Windows» в настройках.
    notifications: bool,
    /// Окно спрятано в трей.
    hidden: bool,
    /// Окно сейчас активно (с ним работают) — тогда уведомления Windows не нужны.
    focused: bool,
    /// Когда показать уведомление о результате игры (см. [`TOAST_DELAY`]).
    toast_at: Option<Instant>,
    /// Когда проверить сгорание рангов (один раз после запуска).
    decay_check_at: Option<Instant>,
    decay_checked: bool,
    /// Только для отладки (LOLAM_DEBUG_QUIT_AFTER=секунды): когда выйти тем же путём,
    /// что и пункт «Выход» в трее.
    debug_quit_at: Option<Instant>,
    result: Option<Result<&'static str, AppError>>,
}

/// Какой экран открыт: аккаунты, лобби, настройки (см. ui/keymap.rs).
use crate::ui::keymap::Screen as Page;

/// Состояние фонового потока, как его видит окно.
enum BackendState {
    Starting,
    Ready,
    Failed(AppError),
}

/// Короткое сообщение внизу окна («Пароль скопирован…»).
struct Notice {
    text: String,
    is_error: bool,
    shown_at: Instant,
}

/// Окно «Удалить аккаунт?».
struct ConfirmDelete {
    /// Кого удаляем: один аккаунт или все отмеченные.
    ids: Vec<AccountId>,
    /// «Name#TAG» или список имён.
    names: String,
    has_credentials: bool,
}

/// Окно ввода кода подтверждения (двухфакторная защита) при входе.
struct LoginCodePrompt {
    /// Куда Riot отправил код (пусто — неизвестно).
    email: String,
    code_length: u8,
    input: String,
}

/// Нажата «Войти»: логин скопирован, ждём нажатия «Пароль» на этой карточке.
struct LoginFlow {
    id: AccountId,
    started: Instant,
}

#[derive(Default)]
struct SettingsState {
    /// Введённый ключ. Поле ввода замаскировано, после сохранения очищается.
    key_input: String,
    /// Ждём ответа на проверку, сохранение или удаление ключа.
    busy: bool,
    /// Результат последнего действия: `Ok(текст)` или ошибка.
    result: Option<Result<&'static str, AppError>>,
}

/// Настройка «спрашивать Windows Hello перед копированием пароля».
#[derive(Default)]
struct HelloState {
    enabled: bool,
    /// Ждём ответа человека в окне Windows Hello.
    busy: bool,
    result: Option<Result<&'static str, AppError>>,
}

#[derive(Default)]
struct TransferState {
    /// Ждём, пока фоновый поток запишет или прочитает файл.
    busy: bool,
    /// Пароль зашифрованной копии (поле замаскировано, после отправки очищается).
    vault_password: String,
    result: Option<Result<String, AppError>>,
}

/// Обновление приложения (настройки и метка в шапке).
#[derive(Default)]
struct UpdateState {
    /// Проверять при запуске (галочка в настройках).
    auto: bool,
    /// Ждём ответа на «Проверить сейчас».
    checking: bool,
    /// Найденная новая версия.
    available: Option<UpdateInfo>,
    /// Установщик скачивается: (получено, всего) байт.
    progress: Option<(u64, u64)>,
    /// Установщик запущен — окно закрывается.
    installing: bool,
    /// Можно ли обновиться кнопкой (программа запущена из папки установки).
    can_install: bool,
    result: Option<Result<String, AppError>>,
}

#[derive(Default)]
struct LeagueDirState {
    info: LeagueDirInfo,
    input: String,
    busy: bool,
    result: Option<Result<&'static str, AppError>>,
}

pub struct App {
    page: Page,
    backend: Backend,
    state: BackendState,
    has_api_key: bool,
    /// Клиент League: не запущен / загружается / подключён (и к какому аккаунту).
    lcu: LcuStatus,
    /// Группа, выбор чемпионов, игра — для вкладки «Лобби».
    lobby: LobbyState,
    /// Турнир Clash: команда и соперник. Пока турнир идёт, вкладка «Лобби» показывает его.
    clash: ClashState,
    /// Каталог образов и поиск — для вкладки «Обои».
    wallpapers: wallpapers_view::State,
    /// Аккаунты, у которых прямо сейчас идёт игра (кроме того, что в клиенте).
    live: HashMap<AccountId, LiveGame>,
    accounts: Vec<Account>,
    /// Аккаунты, которые сейчас обновляются (показываем индикатор).
    refreshing: HashSet<AccountId>,
    /// Последняя ошибка обновления каждого аккаунта.
    errors: HashMap<AccountId, AppError>,
    refresh_all_running: bool,
    /// До какого момента Riot просил подождать (ответ 429).
    rate_limited_until: Option<Instant>,
    notice: Option<Notice>,
    settings: SettingsState,
    league_dir: LeagueDirState,
    /// Экспорт / импорт списка (настройки).
    transfer: TransferState,
    hello: HelloState,
    update: UpdateState,
    desktop: DesktopState,
    /// Поиск, фильтр по тегу и сортировка над списком.
    view: ListView,
    images: ImageStore,
    /// Открытая форма добавления или редактирования (`None` — закрыта).
    form: Option<AccountForm>,
    confirm_delete: Option<ConfirmDelete>,
    /// Окно ввода кода подтверждения при входе (`None` — закрыто).
    login_code_prompt: Option<LoginCodePrompt>,
    /// Окно «Подробности игры» (щелчок по игре на строке аккаунта).
    match_dialog: Option<MatchDialog>,
    /// Уже загруженные подробности: повторное открытие той же игры — без запроса.
    match_cache: HashMap<String, MatchDetails>,
    /// Нынешние ранги игроков из открытых игр (на время работы программы).
    player_ranks: HashMap<RiotId, RankedQueues>,
    /// Игры, для которых ранги игроков сейчас загружаются.
    ranks_loading: HashSet<String>,
    /// Окно «прошлые Clash» (щелчок по нику на вкладке Clash): чьё.
    clash_history: Option<HistoryRequest>,
    /// Загруженные прошлые Clash (на время работы программы); `None` — ещё загружаются.
    clash_histories: HashMap<RiotId, Option<Result<ClashHistory, AppError>>>,
    /// Окно «История ранга» (щелчок по рангу на строке аккаунта).
    history_dialog: Option<HistoryDialog>,
    /// Карточка «после игры» в правом нижнем углу.
    post_game: Option<PostGame>,
    /// Только для отладки (LOLAM_DEBUG_PAGE=match|history|postgame): что открыть, когда
    /// придёт список аккаунтов.
    debug_page: Option<String>,
    login_flow: Option<LoginFlow>,
    /// Аккаунт под курсором (янтарная строка таблицы).
    cursor: Option<AccountId>,
    /// Отмеченные аккаунты (пробел): r, Delete и «метка» действуют на них все.
    marked: HashSet<AccountId>,
    /// Курсор сдвинули с клавиатуры — прокрутить таблицу к нему.
    scroll_to_cursor: bool,
    /// Открыто поле поиска над таблицей.
    search_open: bool,
    /// Поставить фокус в поле поиска, как только оно появится.
    search_focus: bool,
    /// Открытая командная строка (`:` или Ctrl+K).
    command: Option<CommandLine>,
    /// Открыта справка по клавишам (`?` или F1).
    help_open: bool,
    /// Загрузка всей истории игр (h во «Всех играх»): идёт или чем кончилась — по аккаунтам.
    history_loads: HashMap<AccountId, HistoryLoad>,
    /// Окно «Все игры» (архив игр аккаунта, клавиша m).
    games_view: Option<GamesView>,
    /// Окно «Сводка» (все аккаунты за период, клавиша d).
    overview: Option<OverviewView>,
    /// Окно «Профиль» (статистика по архиву игр, клавиша i).
    profile_view: Option<ProfileView>,
    /// Окно «Коллекция» (на каком аккаунте есть чемпион или образ, клавиша b).
    collection: Option<CollectionView>,
    /// Сохранённые инвентари всех аккаунтов (для «Коллекции»); `None` — ещё не загружены.
    inventories: Option<HashMap<AccountId, Inventory>>,
    /// Только для отладки (LOLAM_DEBUG_PAGE=lobby-demo): лобби из тестовых аккаунтов,
    /// клиент League его не перезаписывает.
    debug_lobby: bool,
}

impl App {
    /// Вызывается один раз при старте (из main.rs).
    /// `&eframe::CreationContext<'_>` — ссылка (заимствование): мы только читаем `cc`,
    /// владеет им eframe. `'_` — время жизни ссылки, компилятор выводит его сам.
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        theme::apply(&cc.egui_ctx);
        let desktop = Self::desktop_state(cc, startup);
        // Декодеры картинок (PNG) из egui_extras: превращают байты в текстуры.
        egui_extras::install_image_loaders(&cc.egui_ctx);
        // Сортировка с прошлого запуска (см. `save` ниже). `cc.storage` — None,
        // если сохранять некуда; тогда просто сортировка по умолчанию.
        let mut view = ListView::default();
        if let Some(sort) = cc
            .storage
            .and_then(|s| s.get_string(SORT_KEY))
            .and_then(|key| SortOrder::from_key(&key))
        {
            view.sort = sort;
        }
        // Только в debug-сборке: LOLAM_DEBUG_PAGE=lobby|settings|add|match открывает страницу
        // или форму сразу — чтобы проверять внешний вид по скриншотам без кликов.
        let (page, form) = match std::env::var("LOLAM_DEBUG_PAGE").as_deref() {
            Ok("lobby") if cfg!(debug_assertions) => (Page::Lobby, None),
            // «wallpapers-selected» — то же, и сразу выбрать третий образ (см. Event::SkinsLoaded).
            Ok("wallpapers" | "wallpapers-selected") if cfg!(debug_assertions) => (Page::Wallpapers, None),
            Ok("settings" | "update") if cfg!(debug_assertions) => (Page::Settings, None),
            Ok("add") if cfg!(debug_assertions) => (Page::Accounts, Some(AccountForm::add(Region::Ru))),
            // «match» — открыть подробности первой игры первого аккаунта (см. Event::Accounts).
            Ok("match") if cfg!(debug_assertions) => (Page::Accounts, None),
            _ => (Page::Accounts, None),
        };
        Self {
            page,
            // Клон Context — дешёвый указатель на общий контекст egui. Через него
            // фоновый поток будит окно (request_repaint), когда приходит событие.
            backend: Backend::start(cc.egui_ctx.clone()),
            state: BackendState::Starting,
            has_api_key: false,
            lcu: LcuStatus::NotRunning,
            lobby: LobbyState::default(),
            clash: ClashState::default(),
            wallpapers: wallpapers_view::State::default(),
            live: HashMap::new(),
            accounts: Vec::new(),
            refreshing: HashSet::new(),
            errors: HashMap::new(),
            refresh_all_running: false,
            rate_limited_until: None,
            notice: None,
            settings: SettingsState::default(),
            league_dir: LeagueDirState::default(),
            transfer: TransferState::default(),
            hello: HelloState::default(),
            desktop,
            update: UpdateState {
                auto: true,
                can_install: update::can_self_install(),
                ..UpdateState::default()
            },
            view,
            images: ImageStore::default(),
            form,
            confirm_delete: None,
            login_code_prompt: None,
            match_dialog: None,
            match_cache: HashMap::new(),
            player_ranks: HashMap::new(),
            ranks_loading: HashSet::new(),
            clash_history: None,
            clash_histories: HashMap::new(),
            history_dialog: None,
            post_game: None,
            debug_page: std::env::var("LOLAM_DEBUG_PAGE")
                .ok()
                .filter(|_| cfg!(debug_assertions)),
            login_flow: None,
            cursor: None,
            marked: HashSet::new(),
            scroll_to_cursor: false,
            search_open: false,
            search_focus: false,
            command: None,
            help_open: false,
            games_view: None,
            history_loads: HashMap::new(),
            profile_view: None,
            overview: None,
            collection: None,
            inventories: None,
            debug_lobby: false,
        }
    }

    // ----- Трей, автозапуск, вторая копия -----------------------------------

    fn desktop_state(cc: &eframe::CreationContext<'_>, startup: Startup) -> DesktopState {
        // Канал: поток, слушающий порт, сообщает окну «вторая копия просит показаться».
        let (show_tx, show_requests) = mpsc::channel();
        // Щелчок по уведомлению Windows — тоже «покажи окно».
        let notifier = notify::is_supported().then(|| {
            let (tx, ctx) = (show_tx.clone(), cc.egui_ctx.clone());
            Notifier::new(move || {
                let _ = tx.send(());
                ctx.request_repaint();
            })
        });
        let notifications = cc.storage.and_then(|s| s.get_string(NOTIFY_KEY)).as_deref() != Some("0");
        if let Some(listener) = startup.listener {
            let ctx = cc.egui_ctx.clone();
            single_instance::listen(listener, move || {
                let _ = show_tx.send(());
                ctx.request_repaint(); // разбудить окно, даже если оно спрятано
            });
        }
        let wants_tray = cc.storage.and_then(|s| s.get_string(TRAY_KEY)).as_deref() == Some("1");
        let tray = if wants_tray {
            Tray::new(cc.egui_ctx.clone())
                .inspect_err(|error| tracing::warn!(%error, "значок в трее не создан"))
                .ok()
        } else {
            None
        };
        // Клавиша включена по умолчанию; не удалось (занята другой программой) — без неё.
        let wants_hotkey = cc.storage.and_then(|s| s.get_string(HOTKEY_KEY)).as_deref() != Some("0");
        let hotkey = if wants_hotkey && hotkey::is_supported() {
            GlobalHotkey::new(cc.egui_ctx.clone())
                .inspect_err(|error| tracing::warn!(%error, "глобальная клавиша не зарегистрирована"))
                .ok()
        } else {
            None
        };
        let debug_quit_at = std::env::var("LOLAM_DEBUG_QUIT_AFTER")
            .ok()
            .filter(|_| cfg!(debug_assertions))
            .and_then(|secs| secs.parse().ok())
            .map(|secs| Instant::now() + Duration::from_secs(secs));
        DesktopState {
            // Windows запустила нас при входе в систему, и трей включён — сразу в трей.
            hide_on_start: startup.by_system && tray.is_some(),
            tray,
            hotkey,
            autostart: autostart::is_enabled(),
            quitting: false,
            show_requests,
            notifier,
            notifications,
            hidden: false,
            focused: false,
            toast_at: None,
            decay_check_at: None,
            decay_checked: false,
            debug_quit_at,
            result: None,
        }
    }

    /// Нажатия в трее и просьбы второй копии. Вызывается из `logic`, которая работает
    /// и тогда, когда окно спрятано.
    fn desktop_events(&mut self, ctx: &egui::Context) {
        self.desktop.focused = ctx.input(|i| i.viewport().focused) == Some(true);
        self.pending_notifications(ctx);
        let mut show = self.desktop.show_requests.try_recv().is_ok();
        let mut quit = false;
        if let Some(tray) = &self.desktop.tray {
            while let Some(action) = tray.poll() {
                match action {
                    TrayAction::Show => show = true,
                    TrayAction::Quit => quit = true,
                }
            }
        }
        // Ctrl+Shift+L: окно не перед глазами — показать; перед глазами — спрятать.
        let mut hide = false;
        if self.desktop.hotkey.as_ref().is_some_and(GlobalHotkey::pressed) {
            if self.desktop.hidden || !self.desktop.focused {
                show = true;
            } else {
                hide = true;
            }
        }
        if let Some(at) = self.desktop.debug_quit_at {
            let left = at.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.desktop.debug_quit_at = None;
                quit = true;
            } else {
                ctx.request_repaint_after(left);
            }
        }
        if quit {
            self.quit(ctx);
        } else if show {
            self.show_window(ctx);
        } else if hide {
            self.hide_window(ctx);
        }
    }

    /// Спрятать окно клавишей: с треем — в трей, без него — свернуть на панель задач.
    fn hide_window(&mut self, ctx: &egui::Context) {
        if self.desktop.tray.is_some() {
            self.desktop.hidden = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }

    /// Показать уведомление Windows — если они включены и окно сейчас не перед глазами
    /// (спрятано в трей, свёрнуто или закрыто другими окнами).
    fn toast(&self, message: Message) {
        let away = self.desktop.hidden || !self.desktop.focused;
        if let Some(notifier) = &self.desktop.notifier
            && self.desktop.notifications
            && away
        {
            notifier.show(message);
        }
    }

    /// Отложенные уведомления: результат игры и сгорание рангов.
    fn pending_notifications(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        // `due` — пришло ли время; если нет, просим разбудить нас к этому моменту
        // (иначе спрятанное окно просто спит и ничего не проверяет).
        let due = |at: Option<Instant>| match at {
            Some(at) if at <= now => true,
            Some(at) => {
                ctx.request_repaint_after(at - now);
                false
            }
            None => false,
        };
        if due(self.desktop.toast_at) {
            self.desktop.toast_at = None;
            if let Some(card) = &self.post_game {
                self.toast(card.notification());
            }
        }
        if due(self.desktop.decay_check_at) {
            self.desktop.decay_check_at = None;
            self.warn_about_decay();
        }
    }

    /// Один раз после запуска: если у какого-то аккаунта ранг вот-вот начнёт сгорать
    /// (или уже сгорает) — говорим об этом внизу окна и уведомлением Windows.
    fn warn_about_decay(&mut self) {
        let now = unix_now();
        let mut urgent: Vec<(String, decay::Decay)> = self
            .accounts
            .iter()
            .filter_map(|account| {
                let (queue, decay) = decay::warning(account, now).filter(|(_, d)| d.is_urgent())?;
                Some((format!("{} ({})", account.riot_id, queue.label()), decay))
            })
            .collect();
        urgent.sort_by_key(|(_, decay)| decay.days_left);
        let Some((first, decay)) = urgent.first() else {
            return;
        };
        let when = if decay.is_active() {
            "уже сгорает".to_string()
        } else {
            format!("начнёт сгорать ≈ через {} дн.", decay.days_left)
        };
        let more = match urgent.len() - 1 {
            0 => String::new(),
            n => format!(" · и ещё аккаунтов: {n}"),
        };
        let text = format!("{first}: ранг {when}{more}");
        self.toast(Message::new("Ранг сгорает без игр", text.clone()).detail("Сыграйте ранговую игру на этом аккаунте"));
        self.notify(text);
    }

    /// Закрыть программу совсем (а не спрятать в трей).
    fn quit(&mut self, ctx: &egui::Context) {
        self.desktop.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.desktop.hide_on_start = false;
        self.desktop.hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    /// Перехват закрытия окна: с включённым треем крестик (и Alt+F4) прячет окно.
    fn close_to_tray(&mut self, ctx: &egui::Context) {
        // mem::take — забрать значение и оставить на его месте false.
        let hide_now = std::mem::take(&mut self.desktop.hide_on_start);
        let closing = ctx.input(|i| i.viewport().close_requested());
        let to_tray = closing && self.desktop.tray.is_some() && !self.desktop.quitting;
        if to_tray {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        if to_tray || hide_now {
            self.desktop.hidden = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    // ----- События от фонового потока ---------------------------------------

    /// `match` обязан покрыть все варианты Event: если добавить новое событие
    /// и забыть его обработать, программа не скомпилируется.
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Ready {
                has_api_key,
                league_dir,
                update_check,
                hello_lock,
            } => {
                self.state = BackendState::Ready;
                self.hello.enabled = hello_lock;
                self.update.auto = update_check;
                self.has_api_key = has_api_key;
                self.league_dir.input = league_dir.custom.clone().unwrap_or_default();
                self.league_dir.info = league_dir;
            }
            Event::StartupFailed(error) => self.state = BackendState::Failed(error),
            Event::Accounts(accounts) => {
                self.accounts = accounts;
                // Первый список после запуска — чуть позже проверим сгорание рангов.
                if !std::mem::replace(&mut self.desktop.decay_checked, true) {
                    self.desktop.decay_check_at = Some(Instant::now() + DECAY_CHECK_DELAY);
                }
                if let Some(page) = self.debug_page.take() {
                    self.debug_open(&page);
                }
            }
            Event::LcuStatus(status) => self.lcu = status,
            Event::Lobby(state) if !self.debug_lobby => self.lobby = state,
            Event::Lobby(_) => {}
            Event::Clash(state) => self.clash = state,
            Event::ClashHistoryLoaded { riot_id, result } => {
                self.clash_histories.insert(riot_id, Some(result));
            }
            Event::LiveGames(games) => self.live = games.into_iter().collect(),

            Event::SkinsLoaded(result) => {
                self.wallpapers.loading = false;
                match result {
                    Ok(skins) => {
                        if cfg!(debug_assertions)
                            && std::env::var("LOLAM_DEBUG_PAGE").as_deref() == Ok("wallpapers-selected")
                        {
                            self.wallpapers.selected = skins.get(2).map(|s| s.id);
                        }
                        self.wallpapers.skins = skins;
                        self.wallpapers.load_error = None;
                    }
                    Err(error) => self.wallpapers.load_error = Some(error.to_string()),
                }
            }
            Event::BackgroundSet(result) => {
                self.wallpapers.setting = false;
                self.wallpapers.status = Some(match result {
                    Ok(()) => "Готово. Откройте профиль в клиенте заново, чтобы увидеть фон.".into(),
                    Err(error) => format!("Не удалось: {error}"),
                });
            }

            Event::AccountAdded {
                account,
                fetch_error,
            } => {
                // `is_some_and` — Some и условие выполняется.
                if self.form.as_ref().is_some_and(AccountForm::is_add) {
                    self.form = None;
                }
                self.notify(format!("Аккаунт {} добавлен", account.riot_id));
                if let Some(error) = fetch_error {
                    self.errors.insert(account.id, error);
                }
                self.accounts.push(account);
            }
            // Ошибку показываем в форме (там же остаются введённые данные),
            // а если форма уже закрыта — внизу окна.
            Event::AddFailed(error) => match &mut self.form {
                Some(form) if form.is_add() => form.failed(&error),
                _ => self.notify_error(&error),
            },
            Event::AccountEdited(account) => {
                if self.form.as_ref().and_then(AccountForm::editing) == Some(account.id) {
                    self.form = None;
                }
                self.notify(format!("Аккаунт {} сохранён", account.riot_id));
                self.replace_account(account);
            }
            Event::EditFailed { id, error } => match &mut self.form {
                Some(form) if form.editing() == Some(id) => form.failed(&error),
                _ => self.notify_error(&error),
            },
            Event::AccountDeleted(id) => {
                self.marked.remove(&id);
                if self.form.as_ref().and_then(AccountForm::editing) == Some(id) {
                    self.form = None;
                }
                if self.login_flow.as_ref().is_some_and(|f| f.id == id) {
                    self.login_flow = None;
                }
                if self.history_dialog.as_ref().is_some_and(|d| d.account == id) {
                    self.history_dialog = None;
                }
                if self.post_game.as_ref().is_some_and(|c| c.account == id) {
                    self.post_game = None;
                }
                if self.games_view.as_ref().is_some_and(|v| v.account == id) {
                    self.games_view = None;
                }
                // retain — оставить только элементы, для которых замыкание вернуло true.
                self.accounts.retain(|a| a.id != id);
                self.errors.remove(&id);
                self.notify("Аккаунт удалён");
            }
            Event::DeleteFailed { error, .. } => self.notify_error(&error),

            Event::RefreshQueued(id) => {
                self.refreshing.insert(id);
                self.errors.remove(&id);
            }
            Event::AccountRefreshed { account, .. } => {
                self.refreshing.remove(&account.id);
                // Аккаунт из клиента League: новая игра или новые LP — карточка «после игры».
                if self.is_in_client(&account)
                    && let Some(old) = self.accounts.iter().find(|a| a.id == account.id)
                    && let Some(card) = PostGame::detect(old, &account)
                {
                    match &mut self.post_game {
                        Some(current) if current.account == account.id && !current.expired() => current.merge(card),
                        _ => self.post_game = Some(card),
                    }
                    // Окно спрятано или не активно — расскажем уведомлением Windows.
                    self.desktop.toast_at = Some(Instant::now() + TOAST_DELAY);
                }
                // Открыто «Все игры» этого аккаунта — в архиве могли появиться новые игры.
                if self.games_view.as_ref().is_some_and(|v| v.account == account.id)
                    || self.profile_view.as_ref().is_some_and(|v| v.account == account.id)
                {
                    self.backend.send(Command::LoadGames(account.id));
                    // И игроки этих игр (напарники, поиск): у новых игр могли появиться подробности.
                    self.backend.send(Command::LoadPlayers(account.id));
                }
                self.replace_account(account);
            }
            Event::RefreshFailed { id, error } => {
                self.refreshing.remove(&id);
                if let AppError::RateLimited { retry_after_secs } = error {
                    self.rate_limited(retry_after_secs);
                }
                self.errors.insert(id, error);
            }
            Event::RefreshAllDone { ok, failed } => {
                self.refresh_all_running = false;
                // Все неудачи — из-за связи: скорее всего, нет интернета.
                let offline = failed > 0 && self.errors.values().all(AppError::is_offline);
                match (ok, failed) {
                    (0, 0) => {}
                    (0, _) if offline => self.notify_error_text(
                        "Нет связи с интернетом — показаны сохранённые данные".to_string(),
                    ),
                    (ok, 0) => self.notify(format!("Обновлено аккаунтов: {ok}")),
                    (ok, failed) => self.notify_error_text(format!(
                        "Обновлено: {ok}, с ошибками: {failed}"
                    )),
                }
            }
            Event::RateLimitWait { secs } => self.rate_limited(secs),

            Event::CredentialCopied {
                kind,
                clear_after_secs,
            } => self.notify(format!(
                "{} скопирован. Буфер обмена очистится через {clear_after_secs} сек",
                kind.label()
            )),
            Event::CopyFailed(error) => {
                self.login_flow = None; // «Войти» не удалась — второй шаг не ждём
                self.notify_error(&error);
            }
            Event::ClipboardCleared => self.notify("Буфер обмена очищен"),
            Event::AutoLoginProgress(text) => self.notify(text),
            Event::AutoLoginDone(Ok(text)) => self.notify(text),
            Event::AutoLoginDone(Err(error)) => self.notify_error(&error),
            Event::AutoLoginNeedCode { email, code_length } => {
                self.login_code_prompt = Some(LoginCodePrompt { email, code_length, input: String::new() });
            }
            Event::RiotClientLaunched => self.notify(if self.login_flow.is_some() {
                "Riot Client открывается. Вставьте логин (Ctrl+V), затем нажмите «Пароль» на карточке"
            } else {
                "Riot Client открывается. Вставьте логин (Ctrl+V)"
            }),

            Event::ImageLoaded { key, bytes } => self.images.loaded(key, bytes),
            Event::ImageFailed(key) => self.images.failed(key),

            Event::LeagueDirSaved(result) => {
                self.league_dir.busy = false;
                self.league_dir.result = Some(match result {
                    Ok(info) => {
                        self.league_dir.input = info.custom.clone().unwrap_or_default();
                        let text = if info.custom.is_some() {
                            "Папка сохранена"
                        } else {
                            "Папка снова определяется автоматически"
                        };
                        self.league_dir.info = info;
                        Ok(text)
                    }
                    Err(error) => Err(error),
                });
            }

            Event::ApiKeyChecked(result) => {
                self.settings.busy = false;
                self.settings.result = Some(result.map(|()| "Ключ действителен"));
            }
            Event::ApiKeySaved(result) => {
                self.settings.busy = false;
                if result.is_ok() {
                    self.has_api_key = true;
                    self.settings.key_input.clear();
                }
                self.settings.result =
                    Some(result.map(|()| "Ключ проверен и сохранён в системном хранилище"));
            }
            Event::ApiKeyDeleted(result) => {
                self.settings.busy = false;
                if result.is_ok() {
                    self.has_api_key = false;
                }
                self.settings.result = Some(result.map(|()| "Ключ удалён"));
            }
            Event::Exported(result) => {
                self.transfer.busy = false;
                self.transfer.result = Some(result.map(|count| format!("Сохранено аккаунтов: {count}")));
            }
            Event::Imported(result) => {
                self.transfer.busy = false;
                let result = result.map(|report| {
                    let mut text = format!("Добавлено аккаунтов: {}", report.added);
                    if report.duplicates > 0 {
                        text.push_str(&format!(", уже были в списке: {}", report.duplicates));
                    }
                    if report.invalid > 0 {
                        text.push_str(&format!(", с ошибкой в файле: {}", report.invalid));
                    }
                    text
                });
                match &result {
                    Ok(text) => self.notify(text.clone()),
                    Err(error) => self.notify_error(error),
                }
                self.transfer.result = Some(result);
            }
            Event::MatchLoaded { match_id, result } => {
                if let Ok(details) = &result {
                    self.match_cache.insert(match_id.clone(), details.clone());
                    // Игра пришла — теперь можно спросить ранги её игроков.
                    if let Some(account) = self.match_dialog.as_ref().filter(|d| d.summary.id == match_id).map(|d| d.account) {
                        self.request_ranks(account, details);
                    }
                }
                // Окно могли закрыть или открыть другую игру — тогда ответ только в кэш.
                if let Some(dialog) = &mut self.match_dialog
                    && dialog.summary.id == match_id
                {
                    dialog.details = Some(result);
                }
            }
            Event::RanksLoaded { match_id, result } => {
                self.ranks_loading.remove(&match_id);
                // Ошибка — не беда: в столбце «Ранг» останутся прочерки.
                if let Ok(found) = result {
                    self.player_ranks.extend(found);
                }
            }
            Event::GamesLoaded { account, result } => {
                // Тот же архив нужен и окну «Профиль».
                if let Some(view) = &mut self.profile_view
                    && view.account == account
                {
                    view.games = Some(result.clone());
                }
                if let Some(view) = &mut self.games_view
                    && view.account == account
                {
                    view.games = Some(result);
                }
            }
            Event::PlayersLoaded { account, result } => {
                if let Some(view) = &mut self.profile_view
                    && view.account == account
                {
                    view.players_loaded(&result);
                }
                if let Some(view) = &mut self.games_view
                    && view.account == account
                {
                    view.players = Some(result);
                }
            }
            Event::HistoryProgress { account, progress } => {
                let before = self.history_loads.get(&account).map_or(0, |l| l.progress.saved);
                self.history_loads.insert(account, HistoryLoad { progress, running: true, error: None });
                // Открытые окна подхватывают новые игры не каждую игру, а пачками.
                if progress.saved / HISTORY_RELOAD_EVERY != before / HISTORY_RELOAD_EVERY {
                    self.reload_archive(account);
                }
            }
            Event::HistoryDone { account, progress, error } => {
                let name = self.accounts.iter().find(|a| a.id == account).map(|a| a.riot_id.to_string());
                let name = name.unwrap_or_default();
                match &error {
                    Some(e) => self.notify_error_text(format!("История {name}: {e} (загружено +{} игр)", progress.saved)),
                    None if progress.stopped => self.notify(format!("История {name}: остановлено, +{} игр", progress.saved)),
                    None => self.notify(format!("История {name} загружена: +{} новых игр", progress.saved)),
                }
                self.history_loads.insert(account, HistoryLoad { progress, running: false, error });
                self.reload_archive(account);
            }
            Event::InventoryLoaded { account, inventory } => {
                // Список инвентарей уже загружен — обновим в нём этот аккаунт.
                if let (Some(map), Some(inventory)) = (&mut self.inventories, &inventory) {
                    map.insert(account, inventory.clone());
                }
                if let Some(view) = &mut self.profile_view
                    && view.account == account
                {
                    view.inventory = Some(inventory);
                }
            }
            Event::Inventories(list) => self.inventories = Some(list.into_iter().collect()),
            Event::UpdateChecked { result, manual } => self.update_checked(result, manual),
            Event::UpdateProgress { received, total } => self.update.progress = Some((received, total)),
            Event::UpdateInstalling => {
                self.update.progress = None;
                self.update.installing = true; // окно закроется в `ui`
            }
            Event::HelloLockSaved(result) => {
                self.hello.busy = false;
                self.hello.result = Some(result.map(|enabled| {
                    self.hello.enabled = enabled;
                    if enabled { "Защита включена" } else { "Защита выключена" }
                }));
            }
            Event::UpdateFailed(error) => {
                self.update.progress = None;
                self.update.result = Some(Err(error));
            }

            Event::Warning(error) => self.notify_error(&error),
        }
    }

    /// Ответ на проверку обновлений.
    fn update_checked(&mut self, result: Result<Option<UpdateInfo>, AppError>, manual: bool) {
        self.update.checking = false;
        match result {
            Ok(Some(info)) => {
                // О каждой новой версии говорим один раз, а не каждые 6 часов.
                if self.update.available.as_ref() != Some(&info) {
                    self.toast(
                        Message::new(format!("Доступна версия {}", info.version), "Откройте настройки → обновление")
                            .detail("LoL Account Manager"),
                    );
                    self.notify(format!(
                        "Доступна версия {} — настройки → обновление (или команда «обновление»)",
                        info.version
                    ));
                }
                self.update.available = Some(info);
                self.update.result = None;
            }
            Ok(None) => {
                self.update.available = None;
                if manual {
                    self.update.result = Some(Ok("Установлена последняя версия".into()));
                }
            }
            Err(error) => self.update.result = Some(Err(error)),
        }
    }

    /// «Проверить сейчас» (кнопка в настройках или команда «обновление»).
    fn check_update(&mut self) {
        if self.update.checking || self.update.progress.is_some() {
            return;
        }
        self.update.checking = true;
        self.update.result = None;
        self.backend.send(Command::CheckUpdate);
    }

    fn replace_account(&mut self, account: Account) {
        // iter_mut().find(...) — изменяемая ссылка на первый подходящий элемент.
        if let Some(slot) = self.accounts.iter_mut().find(|a| a.id == account.id) {
            *slot = account;
        }
    }

    fn lcu_connected(&self) -> bool {
        matches!(self.lcu, LcuStatus::Connected { .. })
    }

    /// Этот аккаунт сейчас открыт в клиенте League?
    fn is_in_client(&self, account: &Account) -> bool {
        match &self.lcu {
            LcuStatus::Connected {
                region,
                riot_id,
                puuid,
            } => is_same_account(account, *region, riot_id, puuid),
            _ => false,
        }
    }

    /// Аккаунт, в который выполнен вход в клиенте League (для кнопки «Из клиента»).
    fn client_account(&self) -> Option<(Region, RiotId)> {
        match &self.lcu {
            LcuStatus::Connected {
                region, riot_id, ..
            } => Some((*region, riot_id.clone())),
            _ => None,
        }
    }

    /// Регион нового аккаунта по умолчанию: сервер клиента League, иначе первого аккаунта.
    fn default_region(&self) -> Region {
        match &self.lcu {
            LcuStatus::Connected { region, .. } => *region,
            // map_or(по умолчанию, функция) — если список пуст, взять EUW.
            _ => self.accounts.first().map_or(Region::Euw, |a| a.region),
        }
    }

    fn open_add_form(&mut self) {
        self.form = Some(AccountForm::add(self.default_region()));
    }

    /// Нажатие кнопки на карточке.
    fn card_action(&mut self, action: CardAction, id: AccountId) {
        // Поля структуры заимствуются по отдельности: читаем self.accounts
        // и одновременно меняем self.form — компилятор это разрешает.
        let Some(account) = self.accounts.iter().find(|a| a.id == id) else {
            return;
        };
        match action {
            CardAction::Refresh => self.backend.send(Command::RefreshAccount(id)),
            CardAction::Copy(kind) => self.backend.send(Command::CopyCredential { id, kind }),
            CardAction::StartLogin => {
                // Без пароля второго шага нет: только логин и запуск клиента.
                self.login_flow = account.has_password.then(|| LoginFlow {
                    id,
                    started: Instant::now(),
                });
                self.backend.send(Command::StartLogin(id));
            }
            CardAction::FinishLogin => {
                self.login_flow = None;
                self.backend.send(Command::CopyCredential {
                    id,
                    kind: CredentialKind::Password,
                });
            }
            CardAction::Edit => self.form = Some(AccountForm::edit(account)),
            CardAction::Delete => {
                self.confirm_delete = Some(ConfirmDelete {
                    ids: vec![id],
                    names: account.riot_id.to_string(),
                    has_credentials: account.has_login || account.has_password,
                });
            }
            CardAction::OpenMatch(match_id) => {
                let summary = account
                    .cache
                    .as_ref()
                    .and_then(|c| c.matches.as_ref())
                    .and_then(|matches| matches.iter().find(|m| m.id == match_id))
                    .cloned();
                if let Some(summary) = summary {
                    let owner = account.riot_id.clone();
                    self.open_match(id, owner, summary);
                }
            }
            CardAction::OpenHistory(queue) => self.history_dialog = Some(HistoryDialog::new(id, queue)),
        }
    }

    /// Окно «Подробности игры»: уже открывали — сразу, иначе просим фоновый поток
    /// (он возьмёт из архива, клиента League или Riot API).
    fn open_match(&mut self, id: AccountId, owner: RiotId, summary: MatchSummary) {
        let cached = self.match_cache.get(&summary.id).cloned();
        if let Some(details) = &cached {
            self.request_ranks(id, details);
        }
        let details = cached.map(Ok);
        if details.is_none() {
            self.backend.send(Command::LoadMatch {
                account: id,
                match_id: summary.id.clone(),
            });
        }
        self.match_dialog = Some(MatchDialog {
            account: id,
            owner,
            summary,
            details,
        });
    }

    /// Просит нынешние ранги игроков игры — тех, чьи ещё не знаем.
    fn request_ranks(&mut self, account: AccountId, details: &MatchDetails) {
        let players: Vec<RiotId> = details
            .teams
            .iter()
            .flat_map(|team| &team.players)
            .filter_map(|player| player.riot_id.clone())
            .filter(|id| !self.player_ranks.contains_key(id))
            .collect();
        // `insert` возвращает false, если такой запрос уже идёт.
        if players.is_empty() || !self.ranks_loading.insert(details.id.clone()) {
            return;
        }
        self.backend.send(Command::LoadRanks {
            account,
            match_id: details.id.clone(),
            players,
        });
    }

    /// Только в debug-сборке (LOLAM_DEBUG_PAGE): открыть окно или карточку для скриншота
    /// без щелчков. match — первая игра; history — история первого аккаунта с историей;
    /// postgame — карточка «после игры» по последней игре (будто она дала +19 LP);
    /// help — справка по клавишам; command — командная строка с «войти dr4»;
    /// search — поиск «dr».
    fn debug_open(&mut self, page: &str) {
        match page {
            "match" => {
                let first = self.accounts.iter().find_map(|a| {
                    let game = a.cache.as_ref()?.matches.as_ref()?.first()?;
                    Some((a.id, game.id.clone()))
                });
                if let Some((id, match_id)) = first {
                    self.card_action(CardAction::OpenMatch(match_id), id);
                }
            }
            "history" => {
                if let Some(account) = self.accounts.iter().find(|a| a.history.len() >= 2) {
                    self.history_dialog = Some(HistoryDialog::new(account.id, account.history[0].queue));
                }
            }
            "help" => self.help_open = true,
            // Настройки с «найденной» новой версией — посмотреть панель обновления.
            "update" => {
                let current = Version::current();
                self.update_checked(
                    Ok(Some(UpdateInfo {
                        version: Version { minor: current.minor + 1, patch: 0, ..current },
                        page_url: update::RELEASES_PAGE.to_string(),
                        installer: None,
                    })),
                    false,
                );
            }
            "lobby-demo" => {
                self.page = Page::Lobby;
                self.debug_lobby = true;
                self.lobby = debug_lobby(&self.accounts);
            }
            "overview" => self.overview = Some(OverviewView::default()),
            // «collection» или «collection:ари» — сразу с поиском.
            p if p.starts_with("collection") => {
                let query = p.strip_prefix("collection:").unwrap_or_default().to_string();
                self.open_collection(query);
            }
            "profile" => {
                if let Some(account) = self.accounts.iter().find(|a| a.archived_games > 0) {
                    self.profile_view = Some(ProfileView::new(account.id));
                    self.backend.send(Command::LoadGames(account.id));
                    self.backend.send(Command::LoadPlayers(account.id));
                    self.backend.send(Command::LoadInventory(account.id));
                }
            }
            "games" => {
                if let Some(account) = self.accounts.iter().find(|a| a.archived_games > 0) {
                    self.games_view = Some(GamesView::new(account.id, account.riot_id.clone()));
                    self.backend.send(Command::LoadGames(account.id));
                    self.backend.send(Command::LoadPlayers(account.id));
                }
            }
            "command" => {
                self.command = Some(CommandLine {
                    input: "войти fr0".into(),
                    selected: 0,
                });
            }
            "search" => {
                self.search_open = true;
                self.search_focus = true;
                self.view.search = "dr".into();
            }
            "postgame" => {
                let found = self.accounts.iter().find(|a| {
                    a.cache.as_ref().is_some_and(|c| c.ranked.solo.is_some() && c.matches.as_ref().is_some_and(|m| !m.is_empty()))
                });
                if let Some(account) = found {
                    // «Как было до игры»: на 19 LP и одну победу меньше, без последней игры.
                    let mut old = account.clone();
                    if let Some(cache) = old.cache.as_mut() {
                        if let Some(solo) = cache.ranked.solo.as_mut() {
                            solo.lp -= 19;
                            solo.wins = solo.wins.saturating_sub(1);
                            if solo.lp < 0 && !solo.tier.is_apex() {
                                solo.lp += 100;
                                solo.division = match solo.division {
                                    Division::One => Division::Two,
                                    Division::Two => Division::Three,
                                    _ => Division::Four,
                                };
                            }
                        }
                        if let Some(matches) = cache.matches.as_mut() {
                            matches.remove(0);
                        }
                    }
                    self.post_game = PostGame::detect(&old, account);
                }
            }
            _ => {}
        }
    }

    /// Картинки для окон (в таблицах — только текст): эмблемы рангов — окно истории
    /// и карточка «после игры», иконки чемпионов последних игр — карточка и заголовок
    /// подробностей, чемпионы и предметы обеих команд — окно подробностей. Недостающие
    /// просим у фонового потока (каждую один раз — см. `ImageStore::take_missing`).
    fn request_images(&mut self) {
        let needed = self.accounts.iter().flat_map(|a| match &a.cache {
            None => vec![ImageKey::Emblem(None)],
            Some(cache) => vec![
                ImageKey::Emblem(cache.ranked.solo.as_ref().map(|s| s.tier)),
                ImageKey::Emblem(cache.ranked.flex.as_ref().map(|s| s.tier)),
            ],
        });
        // Чемпионы пяти последних игр каждого аккаунта.
        let champions = self
            .accounts
            .iter()
            .filter_map(|a| a.cache.as_ref())
            .flat_map(|c| c.matches.iter().flatten().take(RECENT_MATCHES))
            .map(|m| ImageKey::Champion(m.me.champion_id));
        // Открытые подробности игры: её чемпион (заголовок) и все игроки с предметами.
        let header = self.match_dialog.iter().map(|d| ImageKey::Champion(d.summary.me.champion_id));
        let dialog = self
            .match_dialog
            .iter()
            .filter_map(|d| d.details.as_ref()?.as_ref().ok())
            .flat_map(|d| d.teams.iter().flat_map(|t| &t.players))
            .flat_map(|p| {
                // Чемпион и предметы (0 — пустой слот).
                std::iter::once(ImageKey::Champion(p.stats.champion_id))
                    .chain(p.stats.items.iter().filter(|&&id| id > 0).map(|&id| ImageKey::Item(id)))
            });
        for key in self.images.take_missing(needed.chain(champions).chain(header).chain(dialog)) {
            self.backend.send(Command::LoadImage(key));
        }
    }

    /// «Войти» ждёт второго нажатия не дольше LOGIN_FLOW_TIMEOUT.
    fn expire_login_flow(&mut self, ctx: &egui::Context) {
        if let Some(flow) = &self.login_flow {
            let elapsed = flow.started.elapsed();
            if elapsed >= LOGIN_FLOW_TIMEOUT {
                self.login_flow = None;
            } else {
                ctx.request_repaint_after(LOGIN_FLOW_TIMEOUT - elapsed);
            }
        }
    }

    fn rate_limited(&mut self, secs: u64) {
        self.rate_limited_until = Some(Instant::now() + Duration::from_secs(secs));
    }

    /// `impl Into<String>` — принимает и `&str`, и `String`.
    /// Окно «прошлые Clash» игрока: загруженное — сразу, иначе (или после ошибки) — с запросом.
    fn open_clash_history(&mut self, request: HistoryRequest) {
        let loaded = matches!(self.clash_histories.get(&request.riot_id), Some(None | Some(Ok(_))));
        if !loaded {
            self.load_clash_history(request.clone());
        }
        self.clash_history = Some(request);
    }

    fn load_clash_history(&mut self, request: HistoryRequest) {
        self.clash_histories.insert(request.riot_id.clone(), None);
        self.backend.send(Command::LoadClashHistory(request));
    }

    /// В архиве аккаунта появились игры — перечитать их в открытые «Все игры» и «Профиль».
    fn reload_archive(&mut self, id: AccountId) {
        if self.games_view.as_ref().is_some_and(|v| v.account == id)
            || self.profile_view.as_ref().is_some_and(|v| v.account == id)
        {
            self.backend.send(Command::LoadGames(id));
            self.backend.send(Command::LoadPlayers(id));
        }
    }

    /// Загрузить всю историю игр аккаунта или, если она уже идёт, остановить.
    fn toggle_history(&mut self, id: AccountId, start: bool) {
        if start {
            let progress = Default::default();
            self.history_loads.insert(id, HistoryLoad { progress, running: true, error: None });
            self.backend.send(Command::LoadHistory(id));
        } else {
            self.backend.send(Command::StopHistory(id));
        }
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            is_error: false,
            shown_at: Instant::now(),
        });
    }

    fn notify_error(&mut self, error: &AppError) {
        self.notify_error_text(error.to_string());
    }

    fn notify_error_text(&mut self, text: String) {
        self.notice = Some(Notice {
            text,
            is_error: true,
            shown_at: Instant::now(),
        });
    }

    // ----- Части окна --------------------------------------------------------

    /// Шапка (38 px) — первая строка «терминала»: название, экраны 1 2 3, клиент League,
    /// сессия и кнопки окна. Она же заголовок окна: за неё окно перетаскивают.
    fn header(&mut self, ui: &mut egui::Ui) {
        let session = self.client_session();
        let lobby_active = self.lobby.phase.is_active();
        let lobby_active = lobby_active || self.clash.is_shown();
        let (status, status_color, status_hint) = terminal_status(&self.lcu, self.lobby.phase);
        let new_version = self.update.available.as_ref().map(|info| info.version);
        let mut go = None;
        egui::Panel::top("header")
            .frame(egui::Frame::new().inner_margin(Margin {
                left: 16,
                right: 6,
                top: 0,
                bottom: 0,
            }))
            .exact_size(38.0)
            .show(ui, |ui| {
                // Область перетаскивания — вся ширина шапки (вместе с отступами).
                let viewport = ui.ctx().viewport_rect();
                let bar = egui::Rect::from_x_y_ranges(viewport.x_range(), ui.max_rect().y_range());
                window_frame::drag_area(ui, bar);
                ui.painter()
                    .hline(viewport.x_range(), bar.bottom() - 0.5, egui::Stroke::new(1.0, theme::BORDER));

                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    ui.label(RichText::new("lol-accounts").font(theme::mono(13.0)).color(theme::TEXT).semibold());
                    for screen in Screen::ALL {
                        let dot = screen == Screen::Lobby && lobby_active;
                        if screen_tab(ui, screen, self.page == screen, dot, bar.bottom()) {
                            go = Some(screen);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        window_frame::window_controls(ui);
                        ui.add_space(12.0);
                        ui.spacing_mut().item_spacing.x = 16.0;
                        if let Some(session) = &session {
                            session_label(ui, session);
                        }
                        ui.label(RichText::new(status).font(theme::mono(12.0)).color(status_color))
                            .on_hover_text(status_hint);
                        if let Some(version) = new_version
                            && link(
                                ui,
                                &format!("↑ {version}"),
                                theme::ACCENT_TEXT,
                                "Доступна новая версия приложения — открыть настройки",
                            )
                        {
                            go = Some(Screen::Settings);
                        }
                    });
                });
            });
        if let Some(screen) = go {
            self.go_to(screen);
        }
    }

    /// Игровая сессия аккаунта, в который выполнен вход в клиенте.
    fn client_session(&self) -> Option<Session> {
        self.accounts
            .iter()
            .find(|a| self.is_in_client(a))
            .and_then(|a| history::session(&a.history, unix_now()))
    }

    /// Подвал (60 px): строка подсказок клавиш и под ней — командная строка или
    /// последнее сообщение, как в vim. Возвращает действие, если нажали подсказку
    /// или выполнили команду.
    fn footer(&mut self, ui: &mut egui::Ui) -> Option<Invocation> {
        let hints = self.hints();
        let suggestions = match &self.command {
            Some(command) => self.command_suggestions(&command.input),
            None => Vec::new(),
        };
        let rate_limit_left = self
            .rate_limited_until
            .map(|until| until.saturating_duration_since(Instant::now()))
            .filter(|left| !left.is_zero());
        if rate_limit_left.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(250)); // обратный отсчёт
        }

        let mut result = None;
        let mut close_command = false;
        egui::Panel::bottom("footer")
            .frame(egui::Frame::new().inner_margin(Margin::symmetric(16, 0)))
            .exact_size(60.0)
            .show(ui, |ui| {
                let viewport = ui.ctx().viewport_rect();
                let area = ui.max_rect();
                let middle = area.top() + 30.0;
                let painter = ui.painter();
                painter.rect_filled(
                    egui::Rect::from_x_y_ranges(viewport.x_range(), middle..=area.bottom()),
                    0,
                    theme::CARD,
                );
                painter.hline(viewport.x_range(), area.top() + 0.5, egui::Stroke::new(1.0, theme::BORDER));
                painter.hline(viewport.x_range(), middle, egui::Stroke::new(1.0, theme::BORDER));

                // Две строки по 30 px вплотную — без промежутка между ними.
                ui.spacing_mut().item_spacing.y = 0.0;
                let width = ui.available_width();
                let row = egui::Layout::left_to_right(egui::Align::Center);
                ui.allocate_ui_with_layout(egui::vec2(width, 30.0), row, |ui| {
                    if let Some(action) = terminal::hints_bar(ui, &hints) {
                        result = Some(Invocation::new(action));
                    }
                });
                ui.allocate_ui_with_layout(egui::vec2(width, 30.0), row, |ui| match &mut self.command {
                    Some(command) => match command.show(ui, &suggestions) {
                        CommandEvent::Run(run) => {
                            result = Some(run);
                            close_command = true;
                        }
                        CommandEvent::Close => close_command = true,
                        CommandEvent::None => {}
                    },
                    None => message_line(ui, self.notice.as_ref(), rate_limit_left, self.page == Page::Accounts),
                });
            });
        if close_command {
            self.command = None;
        }
        result
    }

    /// Подсказки клавиш для текущего экрана.
    fn hints(&self) -> Vec<Hint> {
        use Action as A;
        match self.page {
            Page::Lobby => {
                return vec![
                    Hint::new("c", "скопировать ники", A::CopyRiotId),
                    Hint::new("o", "мультипоиск op.gg", A::OpGg),
                    Hint::new("ctrl+1", "аккаунты", A::Screen(Screen::Accounts)),
                    Hint::new(":", "команда", A::CommandLine),
                    Hint::new("?", "помощь", A::Help),
                ];
            }
            Page::Wallpapers => {
                return vec![
                    Hint::new("ctrl+1", "аккаунты", A::Screen(Screen::Accounts)),
                    Hint::new("ctrl+2", "лобби", A::Screen(Screen::Lobby)),
                    Hint::new(":", "команда", A::CommandLine),
                    Hint::new("?", "помощь", A::Help),
                ];
            }
            Page::Settings => {
                return vec![
                    Hint::new("ctrl+1", "аккаунты", A::Screen(Screen::Accounts)),
                    Hint::new("ctrl+2", "лобби", A::Screen(Screen::Lobby)),
                    Hint::new(":", "команда", A::CommandLine),
                    Hint::new("?", "помощь", A::Help),
                ];
            }
            Page::Accounts => {}
        }
        // После «Войти» Enter копирует пароль — подсвечиваем это.
        let pending = self
            .cursor
            .is_some_and(|id| self.login_flow.as_ref().is_some_and(|f| f.id == id));
        let mut enter = Hint::new("enter", if pending { "пароль" } else { "войти" }, A::Login);
        enter.accent = pending;
        vec![
            enter,
            Hint::new("l", "логин", A::CopyLogin),
            Hint::new("p", "пароль", A::CopyPassword),
            Hint::new("r", "обновить", A::Refresh),
            Hint::new("h", "история", A::History),
            Hint::new("m", "все игры", A::Games),
            Hint::new("i", "профиль", A::Profile),
            Hint::new("/", "поиск", A::Search),
            Hint::new(":", "команда", A::CommandLine),
            Hint::new("?", "помощь", A::Help),
        ]
    }

    /// Подсказки командной строки: все аккаунты (без фильтра) в порядке сортировки.
    fn command_suggestions(&self, input: &str) -> Vec<Suggestion> {
        let all = ListView {
            search: String::new(),
            tag: None,
            sort: self.view.sort,
        };
        let targets: Vec<Target> = all
            .apply(&self.accounts)
            .iter()
            .map(|a| Target {
                id: a.id,
                name: a.riot_id.to_string(),
            })
            .collect();
        command::suggest(input, &targets, &account_list::all_tags(&self.accounts), self.cursor)
    }

    /// Сообщение живёт NOTICE_DURATION; пока живёт — перерисуемся, когда пора его убрать.
    fn expire_notice(&mut self, ctx: &egui::Context) {
        if let Some(notice) = &self.notice {
            let shown_for = notice.shown_at.elapsed();
            if shown_for >= NOTICE_DURATION {
                self.notice = None;
            } else {
                ctx.request_repaint_after(NOTICE_DURATION - shown_for);
            }
        }
    }

    /// Экран «Аккаунты»: строка состояния списка, таблица и панель выбранного аккаунта.
    /// Возвращает, что нажали мышью (выполнит `run`).
    fn accounts_page(&mut self, ui: &mut egui::Ui) -> Vec<Invocation> {
        let mut out = Vec::new();
        let font = theme::mono(terminal::FONT);
        match &self.state {
            BackendState::Starting => {
                ui.label(RichText::new("запуск…").font(font).color(theme::TEXT_MUTED));
                return out;
            }
            BackendState::Failed(error) => {
                ui.label(RichText::new("! не удалось запустить приложение").font(font.clone()).color(theme::RED_TEXT));
                ui.add(egui::Label::new(RichText::new(error.to_string()).font(font).color(theme::TEXT_MUTED)).wrap());
                return out;
            }
            BackendState::Ready => {}
        }
        self.expire_login_flow(ui.ctx());
        // Время «обновлено N мин назад» меняется само по себе — перерисуем через 30 с.
        ui.ctx().request_repaint_after(Duration::from_secs(30));

        self.list_status(ui, &mut out);
        ui.add_space(8.0);

        if self.accounts.is_empty() {
            ui.add_space(16.0);
            ui.label(RichText::new("аккаунтов пока нет").font(font.clone()).color(theme::TEXT));
            ui.horizontal(|ui| {
                if terminal::key_button(ui, "n", "добавить", true).clicked() {
                    out.push(Invocation::new(Action::Add));
                }
                ui.label(
                    RichText::new("Riot ID, регион и, если нужно, логин с паролем")
                        .font(font.clone())
                        .color(theme::TEXT_HINT),
                );
            });
            return out;
        }

        let visible = self.view.apply(&self.accounts);
        // Курсор всегда на видимой строке: после фильтра или удаления — на первой.
        if !visible.iter().any(|a| Some(a.id) == self.cursor) {
            self.cursor = visible.first().map(|a| a.id);
        }
        if visible.is_empty() {
            ui.add_space(16.0);
            ui.label(RichText::new("ничего не найдено").font(font.clone()).color(theme::TEXT));
            if terminal::key_button(ui, "esc", "сбросить поиск и тег", false).clicked() {
                out.push(Invocation::new(Action::Escape));
            }
            return out;
        }

        let in_client = visible.iter().find(|a| self.is_in_client(a)).map(|a| a.id);
        // Кто сейчас в игре: по данным Riot API и — для аккаунта из клиента — самого клиента.
        let mut live = self.live.clone();
        if let (Some(id), Some(game)) = (in_client, &self.lobby.game) {
            live.insert(id, game.clone());
        }
        let scroll_to_cursor = std::mem::take(&mut self.scroll_to_cursor);
        // Панели аккаунта нужно ~190 px, остальное — таблице.
        const DETAILS_HEIGHT: f32 = 196.0;
        let table = terminal::Table {
            accounts: &visible,
            cursor: self.cursor,
            in_client,
            marked: &self.marked,
            live: &live,
            refreshing: &self.refreshing,
            errors: &self.errors,
            scroll_to_cursor,
            max_height: (ui.available_height() - DETAILS_HEIGHT).max(terminal::ROW_HEIGHT * 3.0),
        };
        if let Some(id) = table.show(ui) {
            out.push(Invocation::on(Action::Select, id));
        }
        ui.add_space(12.0);
        if let Some(account) = visible.iter().find(|a| Some(a.id) == self.cursor) {
            let details = terminal::Details {
                account,
                in_client: in_client == Some(account.id),
                live: live.get(&account.id),
                refreshing: self.refreshing.contains(&account.id),
                error: self.errors.get(&account.id),
                login_pending: self.login_flow.as_ref().is_some_and(|f| f.id == account.id),
            };
            if let Some(run) = details.show(ui) {
                out.push(run);
            }
        }
        out
    }

    /// Строка над таблицей: «аккаунты 4 · поиск · тег · сортировка» и кнопки справа.
    /// Тег и сортировка — тоже кнопки (как клавиши t и s).
    fn list_status(&mut self, ui: &mut egui::Ui, out: &mut Vec<Invocation>) {
        let font = theme::mono(12.5);
        let shown = self.view.apply(&self.accounts).len();
        let total = self.accounts.len();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.label(RichText::new("аккаунты").font(font.clone()).color(theme::TEXT));
            let count = if shown == total { total.to_string() } else { format!("{shown} из {total}") };
            ui.label(RichText::new(count).font(font.clone()).color(theme::TEXT_MUTED));
            if self.search_open {
                ui.label(RichText::new("/").font(font.clone()).color(theme::ACCENT_TEXT));
                self.search_field(ui);
            } else if !self.view.search.is_empty()
                && link(ui, &format!("поиск «{}»", self.view.search), theme::ACCENT_TEXT, "/ — изменить, Esc — сбросить")
            {
                out.push(Invocation::new(Action::Search));
            }
            if !self.marked.is_empty() {
                ui.label(RichText::new(format!("отмечено {}", self.marked.len())).font(font.clone()).color(theme::ACCENT_TEXT));
                ui.label(
                    RichText::new("r обновить · del удалить · :метка тег · esc снять")
                        .font(font.clone())
                        .color(theme::TEXT_HINT),
                );
            }
            let tag = self.view.tag.clone().unwrap_or_else(|| "все".into());
            let tag_color = if self.view.tag.is_some() { theme::ACCENT_TEXT } else { theme::TEXT_MUTED };
            if link(ui, &format!("тег {tag}"), tag_color, "t / Shift+T — следующий / предыдущий тег") {
                out.push(Invocation::new(Action::NextTag));
            }
            let sort = self.view.sort.label().to_lowercase();
            if link(ui, &sort, theme::TEXT_MUTED, "s — другая сортировка") {
                out.push(Invocation::new(Action::NextSort));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if terminal::key_button(ui, "n", "добавить", false).clicked() {
                    out.push(Invocation::new(Action::Add));
                }
                let label = if self.refresh_all_running { "обновляю…" } else { "обновить все" };
                if terminal::key_button(ui, "shift+r", label, false).clicked() {
                    out.push(Invocation::new(Action::RefreshAll));
                }
            });
        });
    }

    /// Поле поиска. Пока в нём фокус: Esc — сбросить и закрыть, Enter — закрыть
    /// (фильтр остаётся), ↑ ↓ — двигать курсор по найденному.
    fn search_field(&mut self, ui: &mut egui::Ui) {
        let id = egui::Id::unique("search_field");
        if ui.ctx().memory(|m| m.has_focus(id)) {
            let none = egui::Modifiers::NONE;
            let (escape, enter, up, down) = ui.input_mut(|i| {
                (
                    i.consume_key(none, egui::Key::Escape),
                    i.consume_key(none, egui::Key::Enter),
                    i.consume_key(none, egui::Key::ArrowUp),
                    i.consume_key(none, egui::Key::ArrowDown),
                )
            });
            if escape {
                self.view.search.clear();
            }
            if escape || enter {
                self.search_open = false;
                ui.ctx().memory_mut(|m| m.surrender_focus(id));
                return;
            }
            if up {
                self.move_cursor(-1);
            }
            if down {
                self.move_cursor(1);
            }
        }
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.view.search)
                .id(id)
                .font(theme::mono(12.5))
                .text_color(theme::TEXT)
                .frame(egui::Frame::NONE)
                .hint_text(RichText::new("ник, заметка или регион").color(theme::TEXT_HINT))
                .desired_width(240.0),
        );
        if std::mem::take(&mut self.search_focus) {
            response.request_focus();
        }
        terminal::focus_outline(ui, response.rect.expand2(egui::vec2(6.0, 3.0)));
        // Щёлкнули мимо пустого поля — закрываем его.
        if response.lost_focus() && self.view.search.is_empty() {
            self.search_open = false;
        }
    }

    /// Горячие клавиши (ui/keymap.rs). Пока открыто окно или идёт ввод текста,
    /// клавиши не перехватываем: окно или поле ввода само разбирается со своими.
    fn hotkeys(&self, ctx: &egui::Context) -> Vec<Invocation> {
        let busy = self.form.is_some()
            || self.match_dialog.is_some()
            || self.clash_history.is_some()
            || self.history_dialog.is_some()
            || self.confirm_delete.is_some()
            || self.login_code_prompt.is_some()
            || self.help_open
            || self.games_view.is_some()
            || self.profile_view.is_some()
            || self.overview.is_some()
            || self.collection.is_some()
            || self.command.is_some()
            || ctx.text_edit_focused();
        if busy {
            return Vec::new();
        }
        keymap::take_actions(ctx, self.page)
            .into_iter()
            .map(Invocation::new)
            .collect()
    }

    /// Окно «Коллекция» (сразу с поиском, если `query` не пустой).
    fn open_collection(&mut self, query: String) {
        self.collection = Some(CollectionView::new(query));
        // Каждый раз заново: инвентари меняются, когда вы заходите в аккаунты.
        self.backend.send(Command::LoadInventories);
        // Русские названия образов — из каталога «Обоев» (грузится один раз).
        if !self.wallpapers.requested {
            self.wallpapers.requested = true;
            self.wallpapers.loading = true;
            self.backend.send(Command::LoadSkins);
        }
    }

    /// Выполняет действие — с клавиатуры, из командной строки или щелчком.
    fn run(&mut self, invocation: Invocation, ctx: &egui::Context) {
        use Action as A;
        let Invocation { action, account } = invocation;
        // На «Лобби» во время турнира Clash c и o — про соперника (или свою команду,
        // пока соперника нет).
        if self.page == Page::Lobby
            && self.clash.is_shown()
            && account.is_none()
            && matches!(action, A::CopyRiotId | A::OpGg)
        {
            let result = if action == A::CopyRiotId {
                clash_view::copy_names(ctx, &self.clash)
            } else {
                clash_view::open_multisearch(ctx, &self.clash, self.client_region())
            };
            match result {
                Ok(text) => self.notify(text),
                Err(text) => self.notify_error_text(text.to_string()),
            }
            return;
        }
        // На «Лобби» c и o — про игроков лобби: ники и мультипоиск op.gg.
        if self.page == Page::Lobby && account.is_none() && matches!(action, A::CopyRiotId | A::OpGg) {
            let result = if action == A::CopyRiotId {
                lobby_view::copy_names(ctx, &self.lobby)
            } else {
                lobby_view::open_multisearch(ctx, &self.lobby, self.client_region())
            };
            match result {
                Ok(text) => self.notify(text),
                Err(text) => self.notify_error_text(text.to_string()),
            }
            return;
        }
        // Есть отмеченные аккаунты и действие не про конкретный аккаунт из командной
        // строки — «обновить» и «удалить» относятся ко всем отмеченным.
        if account.is_none() && !self.marked.is_empty() && matches!(action, A::Refresh | A::Delete) {
            self.run_on_marked(&action);
            return;
        }
        if action.needs_account() {
            match account.or(self.cursor) {
                Some(id) => self.run_on(action, id, ctx),
                None => self.notify_error_text("Сначала выберите аккаунт: ↑ ↓ или щелчок по строке".into()),
            }
            return;
        }
        match action {
            A::Up => self.move_cursor(-1),
            A::Down => self.move_cursor(1),
            A::PageUp => self.move_cursor(-5),
            A::PageDown => self.move_cursor(5),
            A::First => self.move_cursor(-100_000),
            A::Last => self.move_cursor(100_000),
            A::RefreshAll => self.refresh_all(),
            A::Add => self.open_add_form(),
            A::MarkAll => {
                let visible: Vec<AccountId> = self.view.apply(&self.accounts).iter().map(|a| a.id).collect();
                // Все уже отмечены — снимаем отметки, иначе отмечаем все видимые.
                if !visible.is_empty() && visible.iter().all(|id| self.marked.contains(id)) {
                    self.marked.clear();
                } else {
                    self.marked.extend(visible);
                }
            }
            A::ToggleTag(tag) => self.toggle_tag(&tag),
            A::NextTag => self.cycle_tag(1),
            A::PrevTag => self.cycle_tag(-1),
            A::NextSort => {
                let index = SortOrder::ALL.iter().position(|s| *s == self.view.sort).unwrap_or(0);
                self.view.sort = SortOrder::ALL[(index + 1) % SortOrder::ALL.len()];
            }
            A::Tag(tag) => self.view.tag = tag,
            A::Sort(order) => self.view.sort = order,
            A::Search => {
                self.go_to(Page::Accounts);
                self.search_open = true;
                self.search_focus = true;
            }
            A::CommandLine => self.command = Some(CommandLine::default()),
            A::Help => self.help_open = true,
            A::Overview => {
                self.go_to(Page::Accounts);
                self.overview = Some(OverviewView::default());
            }
            A::Collection(query) => self.open_collection(query),
            A::Quit => self.quit(ctx),
            A::Update => {
                self.go_to(Page::Settings);
                self.check_update();
            }
            A::Screen(screen) => self.go_to(screen),
            // Первый Esc снимает отметки, второй — сбрасывает поиск и тег.
            A::Escape if !self.marked.is_empty() => self.marked.clear(),
            A::Escape => {
                self.view.search.clear();
                self.view.tag = None;
                self.search_open = false;
            }
            // С аккаунтом — в `run_on` выше.
            A::Select
            | A::Mark
            | A::Login
            | A::AutoLogin
            | A::CopyLogin
            | A::CopyPassword
            | A::CopyRiotId
            | A::Refresh
            | A::History
            | A::Game(_)
            | A::Games
            | A::LoadHistory
            | A::Profile
            | A::OpGg
            | A::Edit
            | A::Delete => {}
        }
    }

    /// Действие с конкретным аккаунтом. Курсор переезжает на него (важно для команд
    /// вроде «войти quiet»: сразу видно, с каким аккаунтом работаем).
    fn run_on(&mut self, action: Action, id: AccountId, ctx: &egui::Context) {
        use Action as A;
        let Some(account) = self.accounts.iter().find(|a| a.id == id) else {
            return;
        };
        // Всё нужное достаём сразу: дальше будем менять состояние окна.
        let pending = self.login_flow.as_ref().is_some_and(|f| f.id == id);
        let in_client = self.is_in_client(account);
        let (has_login, has_password) = (account.has_login, account.has_password);
        let (riot_id, region) = (account.riot_id.clone(), account.region);
        let game = match action {
            A::Game(n) => account
                .cache
                .as_ref()
                .and_then(|c| c.matches.as_ref())
                .and_then(|m| m.get(n))
                .map(|g| g.id.clone()),
            _ => None,
        };

        self.cursor = Some(id);
        self.scroll_to_cursor = true;
        match action {
            A::Select => {
                // Аккаунт скрыт фильтром — сбрасываем фильтр, чтобы курсор был виден.
                if !self.view.apply(&self.accounts).iter().any(|a| a.id == id) {
                    self.view.search.clear();
                    self.view.tag = None;
                }
            }
            A::Mark => {
                // Отметить (или снять отметку) и перейти к следующей строке — так удобно
                // отмечать несколько подряд.
                if !self.marked.remove(&id) {
                    self.marked.insert(id);
                }
                self.move_cursor(1);
            }
            A::AutoLogin if in_client => self.notify("Вы уже вошли в этот аккаунт в клиенте League"),
            A::AutoLogin if !(has_login && has_password) => {
                self.notify_error_text(format!(
                    "Для входа одним нажатием у {riot_id} должны быть сохранены логин и пароль — нажмите e, чтобы добавить"
                ));
            }
            A::AutoLogin => {
                self.login_flow = None;
                self.backend.send(Command::AutoLogin(id));
                self.notify(format!("Вхожу в {riot_id}…"));
            }
            A::Login if pending => self.card_action(CardAction::FinishLogin, id),
            A::Login if in_client => self.notify("Вы уже вошли в этот аккаунт в клиенте League"),
            A::Login if !has_login => {
                self.notify_error_text(format!("У {riot_id} нет сохранённого логина — нажмите e, чтобы добавить"));
            }
            A::Login => self.card_action(CardAction::StartLogin, id),
            A::CopyLogin if has_login => self.card_action(CardAction::Copy(CredentialKind::Login), id),
            A::CopyPassword if pending => self.card_action(CardAction::FinishLogin, id),
            A::CopyPassword if has_password => self.card_action(CardAction::Copy(CredentialKind::Password), id),
            A::CopyLogin | A::CopyPassword => {
                let what = if action == A::CopyLogin { "логина" } else { "пароля" };
                self.notify_error_text(format!("У {riot_id} нет сохранённого {what} — нажмите e, чтобы добавить"));
            }
            A::CopyRiotId => {
                ctx.copy_text(riot_id.to_string());
                self.notify(format!("Riot ID скопирован: {riot_id}"));
            }
            A::Refresh => self.card_action(CardAction::Refresh, id),
            A::History => self.card_action(CardAction::OpenHistory(Queue::Solo), id),
            A::Games => {
                self.games_view = Some(GamesView::new(id, riot_id.clone()));
                self.backend.send(Command::LoadGames(id));
                self.backend.send(Command::LoadPlayers(id));
            }
            A::LoadHistory => {
                let running = self.history_loads.get(&id).is_some_and(|l| l.running);
                self.toggle_history(id, !running);
                if running {
                    self.notify(format!("История {riot_id}: останавливаю загрузку"));
                } else {
                    self.notify(format!("История {riot_id}: загружаю в фоне — прогресс во «Всех играх» (m)"));
                }
            }
            A::Profile => {
                self.profile_view = Some(ProfileView::new(id));
                self.backend.send(Command::LoadGames(id));
                self.backend.send(Command::LoadPlayers(id));
                self.backend.send(Command::LoadInventory(id));
            }
            A::Game(n) => match game {
                Some(match_id) => self.card_action(CardAction::OpenMatch(match_id), id),
                None => self.notify_error_text(format!("У {riot_id} нет игры №{}", n + 1)),
            },
            A::OpGg => {
                ctx.open_url(egui::OpenUrl::new_tab(lobby::opgg_profile_url(region, &riot_id)));
                self.notify(format!("Профиль {riot_id} на op.gg открыт в браузере"));
            }
            A::Edit => self.card_action(CardAction::Edit, id),
            A::Delete => self.card_action(CardAction::Delete, id),
            _ => {}
        }
    }

    /// Отмеченные аккаунты в порядке списка.
    fn marked_accounts(&self) -> Vec<&Account> {
        self.accounts.iter().filter(|a| self.marked.contains(&a.id)).collect()
    }

    /// «Обновить» или «удалить» для всех отмеченных аккаунтов.
    fn run_on_marked(&mut self, action: &Action) {
        let marked = self.marked_accounts();
        let ids: Vec<AccountId> = marked.iter().map(|a| a.id).collect();
        match action {
            Action::Refresh => {
                for id in &ids {
                    self.backend.send(Command::RefreshAccount(*id));
                }
                self.notify(format!("Обновляю отмеченные аккаунты: {}", ids.len()));
            }
            Action::Delete => {
                let names: Vec<String> = marked.iter().map(|a| a.riot_id.to_string()).collect();
                let has_credentials = marked.iter().any(|a| a.has_login || a.has_password);
                self.confirm_delete = Some(ConfirmDelete {
                    ids,
                    names: names.join(", "),
                    has_credentials,
                });
            }
            _ => {}
        }
    }

    /// Команда «метка тег»: добавить тег отмеченным аккаунтам (без отметок — аккаунту
    /// под курсором); если он уже есть у всех — убрать. Логины и пароли не трогаем.
    fn toggle_tag(&mut self, tag: &str) {
        let tag = tag.trim().to_lowercase();
        let targets: Vec<&Account> = if self.marked.is_empty() {
            self.accounts.iter().filter(|a| Some(a.id) == self.cursor).collect()
        } else {
            self.marked_accounts()
        };
        if tag.is_empty() || targets.is_empty() {
            self.notify_error_text("Сначала выберите аккаунт или отметьте несколько пробелом".into());
            return;
        }
        let remove = targets.iter().all(|a| a.tags.iter().any(|t| t.to_lowercase() == tag));
        let commands: Vec<Command> = targets
            .iter()
            .filter(|a| remove || !a.tags.iter().any(|t| t.to_lowercase() == tag))
            .map(|a| {
                let mut tags: Vec<String> = a.tags.iter().filter(|t| t.to_lowercase() != tag).cloned().collect();
                if !remove {
                    tags.push(tag.clone());
                }
                Command::UpdateAccount {
                    id: a.id,
                    input: AccountInput {
                        riot_id: a.riot_id.clone(),
                        region: a.region,
                        note: a.note.clone(),
                        tags,
                    },
                    login: SecretChange::Keep,
                    password: SecretChange::Keep,
                }
            })
            .collect();
        let count = commands.len();
        for command in commands {
            self.backend.send(command);
        }
        let what = if remove { "убран у" } else { "добавлен" };
        self.notify(format!("Тег «{tag}» {what} аккаунтов: {count}"));
    }

    /// Сдвинуть курсор на `delta` строк (в пределах списка).
    fn move_cursor(&mut self, delta: isize) {
        let ids: Vec<AccountId> = self.view.apply(&self.accounts).iter().map(|a| a.id).collect();
        if ids.is_empty() {
            return;
        }
        let current = self.cursor.and_then(|id| ids.iter().position(|&x| x == id));
        let next = match current {
            None => 0,
            Some(index) => (index as isize).saturating_add(delta).clamp(0, ids.len() as isize - 1) as usize,
        };
        self.cursor = Some(ids[next]);
        self.scroll_to_cursor = true;
    }

    /// Следующий (`step = 1`) или предыдущий (`-1`) тег фильтра; «все» — тоже вариант.
    fn cycle_tag(&mut self, step: isize) {
        let mut options: Vec<Option<String>> = vec![None];
        options.extend(account_list::all_tags(&self.accounts).into_iter().map(Some));
        let current = options
            .iter()
            .position(|option| match (option, &self.view.tag) {
                (None, None) => true,
                (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                _ => false,
            })
            .unwrap_or(0);
        let next = (current as isize + step).rem_euclid(options.len() as isize) as usize;
        self.view.tag = options[next].clone();
    }

    /// «Обновить все» — с проверками, есть ли откуда обновлять.
    fn refresh_all(&mut self) {
        if !matches!(self.state, BackendState::Ready) || self.accounts.is_empty() {
            return;
        }
        if !(self.has_api_key || self.lcu_connected()) {
            self.notify_error_text("Неоткуда обновить: запустите клиент League или укажите API-ключ в настройках".into());
        } else if self.refresh_all_running {
            self.notify("Обновление уже идёт");
        } else {
            self.refresh_all_running = true;
            self.backend.send(Command::RefreshAll);
            self.notify("Обновляю все аккаунты…");
        }
    }

    fn go_to(&mut self, screen: Page) {
        self.page = screen;
        if screen != Page::Accounts {
            self.search_open = false;
        }
    }

    // ----- Окна поверх списка ------------------------------------------------

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.help_open && terminal::help(ctx) {
            self.help_open = false;
        }
        let client = self.client_account();
        if let Some(form) = &mut self.form {
            match form.show(ctx, client) {
                FormAction::None => {}
                FormAction::Close => self.form = None,
                FormAction::Submit(command) => self.backend.send(command),
            }
        }
        self.delete_dialog(ctx);
        self.login_code_dialog(ctx);
        if let Some(dialog) = &mut self.history_dialog {
            // Аккаунт могли удалить — тогда и окно закрываем.
            let close = match self.accounts.iter().find(|a| a.id == dialog.account) {
                Some(account) => dialog.show(ctx, account, &self.images),
                None => true,
            };
            if close {
                self.history_dialog = None;
            }
        }
        if let Some(view) = &mut self.overview {
            match view.show(ctx, &self.accounts) {
                OverviewAction::None => {}
                OverviewAction::Close => self.overview = None,
                OverviewAction::Select(id) => {
                    self.overview = None;
                    self.run_on(Action::Select, id, ctx);
                }
            }
        }
        if let Some(view) = &mut self.collection
            && view.show(ctx, &self.accounts, self.inventories.as_ref(), &self.wallpapers.skins)
        {
            self.collection = None;
        }
        if let Some(view) = &mut self.profile_view {
            let close = match self.accounts.iter().find(|a| a.id == view.account) {
                Some(account) => view.show(ctx, account),
                None => true,
            };
            if close {
                self.profile_view = None;
            }
        }
        // «Все игры» — под подробностями игры: клавиши ему, только когда подробности закрыты.
        let keys = self.match_dialog.is_none();
        // История рангов аккаунта — для столбца «ранг после игры».
        let accounts = &self.accounts;
        let loads = &self.history_loads;
        let shown = self.games_view.as_mut().map(|view| {
            let history = accounts.iter().find(|a| a.id == view.account).map_or(&[][..], |a| &a.history);
            (view.account, view.show(ctx, keys, history, loads.get(&view.account)))
        });
        if let Some((id, action)) = shown {
            match action {
                GamesAction::None => {}
                GamesAction::LoadHistory => self.toggle_history(id, true),
                GamesAction::StopHistory => self.toggle_history(id, false),
                GamesAction::Close => self.games_view = None,
                GamesAction::Open(summary) => {
                    if let Some(owner) = self.accounts.iter().find(|a| a.id == id).map(|a| a.riot_id.clone()) {
                        self.open_match(id, owner, summary);
                    }
                }
            }
        }
        if let Some(dialog) = &mut self.match_dialog {
            let ranks = Ranks {
                known: &self.player_ranks,
                loading: self.ranks_loading.contains(&dialog.summary.id),
            };
            match dialog.show(ctx, &self.images, ranks) {
                DialogAction::None => {}
                DialogAction::Close => self.match_dialog = None,
                DialogAction::Retry => {
                    dialog.details = None;
                    self.backend.send(Command::LoadMatch {
                        account: dialog.account,
                        match_id: dialog.summary.id.clone(),
                    });
                }
            }
        }
        if let Some(request) = self.clash_history.clone() {
            let state = self.clash_histories.get(&request.riot_id).and_then(Option::as_ref);
            match clash_history_view::show(ctx, &request.riot_id, state, self.client_region()) {
                HistoryAction::None => {}
                HistoryAction::Close => self.clash_history = None,
                // Повторный запрос, только если прошлый уже закончился.
                HistoryAction::Retry if state.is_some() => self.load_clash_history(request),
                HistoryAction::Retry => {}
            }
        }
    }

    /// Окно ввода кода подтверждения при входе (двухфакторная защита).
    fn login_code_dialog(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &mut self.login_code_prompt else {
            return;
        };
        let none = egui::Modifiers::NONE;
        let (submit, cancel) = ctx.input_mut(|i| {
            (i.consume_key(none, egui::Key::Enter), i.consume_key(none, egui::Key::Escape))
        });
        // Some(код) — что отправить фоновому потоку; пустая строка — отмена.
        let mut done: Option<String> = None;
        let modal = egui::Modal::new(egui::Id::unique("login_code"))
            .frame(theme::dialog_frame())
            .show(ctx, |ui| {
                ui.set_width(360.0);
                ui.label(RichText::new("Код подтверждения").size(15.0).semibold().color(theme::TEXT));
                ui.add_space(4.0);
                let where_sent = if prompt.email.is_empty() {
                    "Riot отправил код подтверждения.".to_string()
                } else {
                    format!("Riot отправил код на {}.", prompt.email)
                };
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{where_sent} Введите {} цифр, чтобы войти.", prompt.code_length))
                            .size(12.0)
                            .color(theme::TEXT_MUTED),
                    )
                    .wrap(),
                );
                ui.add_space(10.0);
                let edit = egui::TextEdit::singleline(&mut prompt.input)
                    .font(theme::mono(16.0))
                    .desired_width(160.0)
                    .hint_text("******");
                ui.add(edit).request_focus();
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(Button::primary("Войти")).clicked() {
                        done = Some(prompt.input.clone());
                    }
                    if ui.add(Button::outline("Отмена")).clicked() {
                        done = Some(String::new());
                    }
                });
            });
        if submit {
            done = Some(prompt.input.clone());
        }
        if cancel || modal.should_close() {
            done = Some(String::new());
        }
        if let Some(code) = done {
            self.backend.send(Command::SubmitLoginCode(code));
            self.login_code_prompt = None;
        }
    }

    fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm_delete else {
            return;
        };
        // Enter или Y — удалить, N — отмена (Esc закрывает окно и так).
        let none = egui::Modifiers::NONE;
        let (mut confirmed, mut cancelled) = ctx.input_mut(|i| {
            (
                i.consume_key(none, egui::Key::Enter) || i.consume_key(none, egui::Key::Y),
                i.consume_key(none, egui::Key::N),
            )
        });
        let modal = egui::Modal::new(egui::Id::unique("confirm_delete"))
            .frame(theme::dialog_frame())
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    widgets::icon_tile(ui, Icon::Trash, Tone::Red, 40.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let title = match confirm.ids.len() {
                            1 => "Удалить аккаунт?".to_string(),
                            n => format!("Удалить аккаунты ({n})?"),
                        };
                        ui.label(RichText::new(title).size(15.0).semibold().color(theme::TEXT));
                        ui.add(egui::Label::new(RichText::new(&confirm.names).size(12.5).color(theme::TEXT_MUTED)).wrap());
                    });
                });
                ui.add_space(12.0);
                let text = if confirm.has_credentials {
                    "Сохранённые логины и пароли тоже будут удалены \
                     из хранилища паролей. Отменить нельзя."
                } else {
                    "Отменить нельзя."
                };
                ui.add(egui::Label::new(RichText::new(text).size(12.0).color(theme::TEXT_MUTED)).wrap());
                ui.add_space(6.0);
                ui.label(RichText::new("enter — удалить · esc — отмена").font(theme::mono(11.5)).color(theme::TEXT_HINT));
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    confirmed |= ui.add(Button::danger("Удалить").icon(Icon::Trash)).clicked();
                    cancelled |= ui.add(Button::outline("Отмена")).clicked();
                });
            });

        if confirmed {
            for id in &confirm.ids {
                self.backend.send(Command::DeleteAccount(*id));
            }
            self.marked.clear();
            self.confirm_delete = None;
        } else if cancelled || modal.should_close() {
            self.confirm_delete = None;
        }
    }

    // ----- Настройки ---------------------------------------------------------

    /// `frame` нужен системному окну «Открыть / Сохранить файл»: так оно открывается
    /// поверх нашего окна и не даёт с ним работать, пока не закроется.
    fn settings_page(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.set_max_width(760.0);
            // Заголовок — как строка над таблицей аккаунтов.
            ui.label(RichText::new("настройки").font(theme::mono(12.5)).color(theme::TEXT));
            ui.label(
                RichText::new("всё секретное хранится только в системном хранилище паролей Windows")
                    .font(theme::mono(11.5))
                    .color(theme::TEXT_HINT),
            );
            ui.add_space(12.0);
            theme::panel().inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.league_settings(ui);
            });
            ui.add_space(12.0);
            theme::panel().inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.api_key_settings(ui);
            });
            ui.add_space(12.0);
            theme::panel().inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.hello_settings(ui);
            });
            ui.add_space(12.0);
            theme::panel().inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.transfer_settings(ui, frame);
            });
            ui.add_space(12.0);
            theme::panel().inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.update_settings(ui);
            });
            ui.add_space(12.0);
            theme::panel().inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.desktop_settings(ui);
            });
        });
    }

    /// Защита паролей проверкой Windows Hello (см. hello.rs).
    fn hello_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(theme::caps("Защита паролей"));
        ui.add_space(6.0);
        let mut enabled = self.hello.enabled;
        let mut changed = false;
        ui.add_enabled_ui(hello::is_supported() && !self.hello.busy, |ui| {
            let label = RichText::new("спрашивать Windows Hello перед копированием пароля").size(12.5).color(theme::TEXT);
            changed = ui.checkbox(&mut enabled, label).changed();
        });
        if changed {
            // Галочка переключится, только когда Windows Hello подтвердит (см. HelloLockSaved).
            self.hello.busy = true;
            self.hello.result = None;
            self.backend.send(Command::SetHelloLock(enabled));
        }
        ui.add_space(4.0);
        let text = if hello::is_supported() {
            "Перед копированием пароля (клавиша p, второй шаг «Войти») и сохранением копии с \
             паролями Windows попросит отпечаток пальца, лицо или PIN-код. После подтверждения \
             две минуты повторно не спрашивает. Чтобы включить или выключить защиту, тоже нужно \
             пройти проверку. Требуется настроенный вход Windows Hello (хотя бы PIN-код)."
        } else {
            "Windows Hello есть только в Windows."
        };
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(theme::TEXT_MUTED)).wrap());
        if self.hello.busy {
            ui.label(RichText::new("ждём подтверждения в окне Windows Hello…").size(12.0).color(theme::ACCENT_TEXT));
        }
        result_line(ui, self.hello.result.as_ref());
    }

    /// Запуск вместе с Windows и сворачивание в трей.
    fn desktop_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(theme::caps("Запуск и трей"));
        ui.add_space(6.0);
        let supported = tray::is_supported() && autostart::is_supported();
        let label = |text: &str| RichText::new(text).size(12.5).color(theme::TEXT);
        let mut autostart_on = self.desktop.autostart;
        let mut tray_on = self.desktop.tray.is_some();
        let mut hotkey_on = self.desktop.hotkey.is_some();
        let (mut autostart_changed, mut tray_changed, mut hotkey_changed) = (false, false, false);
        ui.add_enabled_ui(self.desktop.notifier.is_some(), |ui| {
            ui.checkbox(&mut self.desktop.notifications, label("уведомления Windows: результат игры, сгорание ранга, новая версия"));
        });
        ui.add_enabled_ui(supported, |ui| {
            autostart_changed = ui.checkbox(&mut autostart_on, label("запускать вместе с Windows")).changed();
            tray_changed = ui.checkbox(&mut tray_on, label("при закрытии сворачивать в трей")).changed();
        });
        ui.add_enabled_ui(hotkey::is_supported(), |ui| {
            let text = format!("{} из любой программы — показать окно (ещё раз — спрятать)", hotkey::LABEL);
            hotkey_changed = ui.checkbox(&mut hotkey_on, label(&text)).changed();
        });
        if hotkey_changed {
            self.desktop.result = None;
            if hotkey_on {
                match GlobalHotkey::new(ui.ctx().clone()) {
                    Ok(hotkey) => self.desktop.hotkey = Some(hotkey),
                    Err(error) => self.desktop.result = Some(Err(error)),
                }
            } else {
                self.desktop.hotkey = None; // сочетание освобождается вместе со значением
            }
        }
        if autostart_changed {
            // Запись в реестр — мгновенная, фоновый поток для неё не нужен.
            self.desktop.result = Some(autostart::set_enabled(autostart_on).map(|()| {
                if autostart_on { "Добавлено в автозапуск Windows" } else { "Убрано из автозапуска Windows" }
            }));
            self.desktop.autostart = autostart::is_enabled();
        }
        if tray_changed {
            self.desktop.result = None;
            if tray_on {
                match Tray::new(ui.ctx().clone()) {
                    Ok(tray) => self.desktop.tray = Some(tray),
                    Err(error) => self.desktop.result = Some(Err(error)),
                }
            } else {
                self.desktop.tray = None; // значок исчезает вместе со значением
            }
        }
        ui.add_space(4.0);
        let text = if supported {
            "С треем крестик не закрывает программу, а прячет окно: она продолжает следить за \
             клиентом League и обновлять аккаунты после игр. Вернуть окно — щелчок по значку \
             рядом с часами или повторный запуск с ярлыка. Выйти совсем — правая кнопка по \
             значку → «Выход» или команда «выход». Если включены автозапуск и трей, при входе в \
             Windows программа запускается сразу в трей, без окна. Уведомления появляются, \
             только когда окно спрятано или не активно; щелчок по уведомлению открывает окно. \
             Ctrl+Shift+L показывает окно из любой программы (и из трея), а если окно уже \
             перед глазами — прячет его."
        } else {
            "Автозапуск и трей есть только в Windows."
        };
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(theme::TEXT_MUTED)).wrap());
        result_line(ui, self.desktop.result.as_ref());
    }

    /// Версия приложения и обновление из GitHub Releases (см. update.rs).
    fn update_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(theme::caps("Обновление"));
        ui.add_space(6.0);
        let current = Version::current();
        // Клон: ниже меняем self.update, пока смотрим на найденную версию.
        let available = self.update.available.clone();
        match &available {
            Some(info) => status_line(
                ui,
                Icon::Download,
                theme::ACCENT_TEXT,
                &format!("Доступна версия {} (установлена {current})", info.version),
            ),
            None => {
                ui.label(RichText::new(format!("Установлена версия {current}")).size(13.0).medium().color(theme::TEXT));
            }
        }
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Новые версии публикуются на GitHub. Установщик скачивается только по вашей \
                     команде, сверяется по контрольной сумме SHA-256 и ставит новую версию поверх \
                     старой, после чего приложение откроется снова. Аккаунты, пароли и настройки \
                     при этом не затрагиваются.",
                )
                .size(12.0)
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );
        ui.add_space(10.0);

        let downloading = self.update.progress.is_some() || self.update.installing;
        let busy = self.update.checking || downloading;
        let mut install = None;
        let mut check = false;
        ui.horizontal(|ui| {
            if let Some(info) = &available {
                if self.update.can_install && info.installer.is_some() {
                    let button = Button::primary(format!("Установить {}", info.version)).icon(Icon::Download);
                    if ui.add_enabled(!busy, button).clicked() {
                        install = Some(info.clone());
                    }
                    if ui.add(Button::outline("Что нового")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&info.page_url));
                    }
                } else if ui.add(Button::primary("Скачать на GitHub").icon(Icon::Download)).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&info.page_url));
                }
            }
            if ui.add_enabled(!busy, Button::outline("Проверить сейчас")).clicked() {
                check = true;
            }
            if busy {
                widgets::spinner(ui, 14.0);
            }
        });
        if let Some(info) = install {
            self.update.progress = Some((0, info.installer.as_ref().map_or(0, |i| i.size)));
            self.update.result = None;
            self.backend.send(Command::InstallUpdate(info));
        }
        if check {
            self.check_update();
        }

        if self.update.installing {
            ui.add_space(6.0);
            status_line(ui, Icon::Check, theme::GREEN_TEXT, "Установщик запущен — приложение сейчас перезапустится");
        } else if let Some((received, total)) = self.update.progress {
            ui.add_space(8.0);
            download_progress(ui, received, total);
        }
        if available.is_some() && !self.update.can_install {
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Эта копия запущена не из папки установки (переносная версия или сборка из \
                         исходников), поэтому обновить её одной кнопкой нельзя — скачайте новую \
                         версию со страницы выпуска.",
                    )
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
                )
                .wrap(),
            );
        }
        result_line(ui, self.update.result.as_ref());

        ui.add_space(8.0);
        let mut auto = self.update.auto;
        let label = RichText::new("проверять обновления при запуске").size(12.0).color(theme::TEXT_MUTED);
        if ui.checkbox(&mut auto, label).changed() {
            self.update.auto = auto;
            self.backend.send(Command::SetUpdateCheck(auto));
        }
    }

    /// Экспорт и импорт списка аккаунтов.
    fn transfer_settings(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        ui.label(theme::caps("Резервная копия списка"));
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Экспорт сохраняет в файл JSON Riot ID, регионы, теги и заметки — например, \
                     чтобы перенести список на другой компьютер. Логины и пароли в файл НЕ попадают: \
                     они хранятся только в системном хранилище паролей, после импорта их нужно \
                     ввести заново (кнопка «Изменить» на строке аккаунта). Импорт добавляет \
                     только те аккаунты, которых ещё нет в списке.",
                )
                .size(12.0)
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );
        ui.add_space(10.0);

        let busy = self.transfer.busy;
        let mut command = None;
        ui.horizontal(|ui| {
            let export = ui
                .add_enabled(!busy && !self.accounts.is_empty(), Button::outline("Экспорт в файл…").icon(Icon::Download))
                .on_disabled_hover_text("Список пуст — экспортировать нечего");
            if export.clicked() {
                // Окно «Сохранить» — системное. Пока оно открыто, наше окно ждёт:
                // так и задумано, это обычный модальный диалог Windows.
                let picked = rfd::FileDialog::new()
                    .set_parent(frame)
                    .set_title("Экспорт списка аккаунтов")
                    .add_filter("Список аккаунтов (JSON)", &["json"])
                    .set_file_name(transfer::default_file_name(unix_now()))
                    .save_file();
                if let Some(path) = picked {
                    // Имя без расширения — добавим .json, чтобы файл легко было найти.
                    let path = if path.extension().is_none() { path.with_extension("json") } else { path };
                    command = Some(Command::ExportAccounts(path));
                }
            }
            let import = ui.add_enabled(!busy, Button::outline("Импорт из файла…").icon(Icon::Upload));
            if import.clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_parent(frame)
                    .set_title("Импорт списка аккаунтов")
                    .add_filter("Список аккаунтов (JSON)", &["json"])
                    .pick_file()
            {
                command = Some(Command::ImportAccounts(path));
            }
            if busy {
                widgets::spinner(ui, 14.0);
            }
        });
        ui.add_space(14.0);
        self.vault_settings(ui, frame, &mut command);
        if let Some(command) = command {
            self.transfer.busy = true;
            self.transfer.result = None;
            self.backend.send(command);
        }
        result_line(ui, self.transfer.result.as_ref());
    }

    /// Полная копия с логинами и паролями, зашифрованная паролем (см. vault.rs).
    fn vault_settings(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame, command: &mut Option<Command>) {
        ui.label(theme::caps("Полная копия с паролями"));
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Сохраняет список вместе с логинами и паролями в один файл, зашифрованный \
                     паролем, который вы придумаете здесь (Argon2id + XChaCha20-Poly1305). Нужна, \
                     чтобы переехать на другой компьютер целиком. Без пароля копии файл бесполезен, \
                     а забытый пароль восстановить нельзя. Восстановление добавляет только те \
                     аккаунты, которых ещё нет в списке.",
                )
                .size(12.0)
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );
        ui.add_space(8.0);
        let busy = self.transfer.busy;
        ui.add_enabled(
            !busy,
            egui::TextEdit::singleline(&mut self.transfer.vault_password)
                .password(true)
                .hint_text(format!("Пароль копии — не меньше {} символов", vault::MIN_PASSWORD_CHARS))
                .desired_width(360.0),
        );
        ui.add_space(8.0);
        let strong = vault::check_password(&self.transfer.vault_password).is_ok();
        let mut picked = None;
        ui.horizontal(|ui| {
            let save = ui
                .add_enabled(!busy && strong && !self.accounts.is_empty(), Button::outline("Сохранить копию…").icon(Icon::Download))
                .on_disabled_hover_text("Введите пароль копии (и в списке должен быть хотя бы один аккаунт)");
            if save.clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_parent(frame)
                    .set_title("Зашифрованная копия аккаунтов")
                    .add_filter("Зашифрованная копия", &[vault::EXTENSION])
                    .set_file_name(vault::default_file_name(unix_now()))
                    .save_file()
            {
                let path = if path.extension().is_none() { path.with_extension(vault::EXTENSION) } else { path };
                picked = Some((path, true));
            }
            let restore = ui
                .add_enabled(!busy && !self.transfer.vault_password.is_empty(), Button::outline("Восстановить из копии…").icon(Icon::Upload))
                .on_disabled_hover_text("Введите пароль, которым зашифрована копия");
            if restore.clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_parent(frame)
                    .set_title("Восстановить аккаунты из копии")
                    .add_filter("Зашифрованная копия", &[vault::EXTENSION])
                    .pick_file()
            {
                picked = Some((path, false));
            }
        });
        if let Some((path, save)) = picked {
            // Пароль уходит в фоновый поток, а поле ввода затираем и очищаем.
            let password = Secret::new(self.transfer.vault_password.clone());
            zeroize::Zeroize::zeroize(&mut self.transfer.vault_password);
            *command = Some(if save {
                Command::ExportVault { path, password }
            } else {
                Command::ImportVault { path, password }
            });
        }
    }

    fn league_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(theme::caps("Клиент League of Legends"));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let status = client_status(&self.lcu, self.lobby.phase);
            widgets::status_dot(ui, status.color, status.glow);
            ui.label(RichText::new(status.text).size(13.0).medium().color(theme::TEXT));
        });
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Основной источник данных. Пока клиент запущен, уровни и ранги аккаунтов \
                     его сервера обновляются через него — без ключа и без лимитов. Приложение \
                     только читает данные: ничего не вводит в клиент и ничего в нём не нажимает.",
                )
                .size(12.0)
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );

        ui.add_space(14.0);
        ui.label(theme::caps("Папка игры"));
        ui.add_space(2.0);
        match &self.league_dir.info.found {
            Some(dir) => status_line(ui, Icon::Check, theme::GREEN_TEXT, &format!("Игра найдена: {}", dir.display())),
            None => status_line(
                ui,
                Icon::Warning,
                theme::AMBER_TEXT,
                "Игра не найдена в стандартных местах — укажите папку, где лежит LeagueClient.exe",
            ),
        }
        ui.add_space(4.0);
        let field = ui.add(
            egui::TextEdit::singleline(&mut self.league_dir.input)
                .hint_text(r"Определяется автоматически. Например: D:\Games\League of Legends")
                .desired_width(f32::INFINITY),
        );
        let enter_pressed = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        let busy = self.league_dir.busy;
        let typed = self.league_dir.input.trim();
        let changed = !typed.is_empty() && Some(typed) != self.league_dir.info.custom.as_deref();
        let mut command = None;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let save = ui.add_enabled(!busy && changed, Button::primary("Сохранить"));
            if save.clicked() || (enter_pressed && changed && !busy) {
                command = Some(Command::SetLeagueDir(Some(typed.to_string())));
            }
            let auto = ui
                .add_enabled(
                    !busy && self.league_dir.info.custom.is_some(),
                    Button::outline("Искать автоматически"),
                )
                .on_hover_text("Забыть указанную папку и искать игру в стандартных местах");
            if auto.clicked() {
                command = Some(Command::SetLeagueDir(None));
            }
            if busy {
                widgets::spinner(ui, 14.0);
            }
        });
        if let Some(command) = command {
            self.league_dir.busy = true;
            self.league_dir.result = None;
            self.backend.send(command);
        }
        result_line(ui, self.league_dir.result.as_ref());
    }

    fn api_key_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(theme::caps("Riot API key"));
            widgets::tag(ui, "необязательно", Tone::Neutral);
        });
        ui.add_space(6.0);
        if self.has_api_key {
            status_line(ui, Icon::Check, theme::GREEN_TEXT, "Ключ сохранён в системном хранилище паролей");
        } else {
            ui.label(RichText::new("Ключ не задан").size(13.0).medium().color(theme::TEXT));
        }
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Нужен для аккаунтов других серверов и чтобы обновлять данные при закрытом клиенте.",
                )
                .size(12.0)
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );
        ui.add_space(10.0);

        // TextEdit::password — точки вместо символов. Само значение хранится в
        // key_input (память окна), в файлы и логи не попадает.
        let field = ui.add(
            egui::TextEdit::singleline(&mut self.settings.key_input)
                .password(true)
                .hint_text("RGAPI-xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx")
                .desired_width(f32::INFINITY),
        );
        let typed = !self.settings.key_input.trim().is_empty();
        let enter_pressed = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        ui.add_space(4.0);
        let busy = self.settings.busy;
        let mut command = None;
        ui.horizontal(|ui| {
            let save = ui.add_enabled(!busy && typed, Button::primary("Сохранить"));
            if save.clicked() || (enter_pressed && typed && !busy) {
                command = Some(Command::SaveApiKey(Secret::new(self.settings.key_input.clone())));
            }
            let check = ui
                .add_enabled(!busy, Button::outline("Проверить ключ"))
                .on_hover_text("Проверяет введённый ключ, а если поле пустое — сохранённый");
            if check.clicked() {
                // bool::then — Some(...) если true, иначе None (проверить сохранённый).
                let candidate = typed.then(|| Secret::new(self.settings.key_input.clone()));
                command = Some(Command::CheckApiKey(candidate));
            }
            if ui
                .add_enabled(!busy && self.has_api_key, Button::outline("Удалить ключ").icon(Icon::Trash))
                .clicked()
            {
                command = Some(Command::DeleteApiKey);
            }
            if busy {
                widgets::spinner(ui, 14.0);
            }
        });
        // Команду отправляем после замыкания: внутри него `self` уже занят
        // (ui.horizontal держит изменяемую ссылку), а здесь он снова свободен.
        if let Some(command) = command {
            self.settings.busy = true;
            self.settings.result = None;
            self.backend.send(command);
        }
        result_line(ui, self.settings.result.as_ref());

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
        ui.label(theme::caps("Где взять ключ"));
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let text = |t: &str| RichText::new(t).size(12.0).color(theme::TEXT_MUTED);
            ui.label(text("Войдите на"));
            ui.hyperlink_to(RichText::new("developer.riotgames.com").size(12.0), "https://developer.riotgames.com");
            ui.label(text("своим Riot-аккаунтом и нажмите Regenerate API Key."));
        });
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Ключ разработчика действует 24 часа. Лимиты: 20 запросов/с и 100 за 2 минуты — \
                     приложение соблюдает их само.",
                )
                .size(12.0)
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );
    }

    /// Сервер клиента League (для ссылок OP.GG на вкладке «Лобби»).
    fn client_region(&self) -> Option<Region> {
        self.client_account().map(|(region, _)| region)
    }
}

/// Значок и цветной текст в одну строку: «✓ Игра найдена: …».
fn status_line(ui: &mut egui::Ui, icon: Icon, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(13.0, 13.0), egui::Sense::hover());
        widgets::paint_icon(ui.painter(), rect, icon, color);
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).wrap());
    });
}

/// Ход скачивания установщика: полоска и «3.2 из 9.8 МБ».
fn download_progress(ui: &mut egui::Ui, received: u64, total: u64) {
    const MB: f32 = 1024.0 * 1024.0;
    // `as f32` — явное преобразование числа: в Rust оно не происходит само.
    let share = if total == 0 { 0.0 } else { (received as f32 / total as f32).clamp(0.0, 1.0) };
    let (bar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(320.0), 4.0), egui::Sense::hover());
    ui.painter().rect_filled(bar, 0, theme::BORDER);
    let done = egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * share, bar.height()));
    ui.painter().rect_filled(done, 0, theme::ACCENT);
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!(
            "скачивание: {:.1} из {:.1} МБ",
            received as f32 / MB,
            total as f32 / MB
        ))
        .font(theme::mono(11.5))
        .color(theme::TEXT_MUTED),
    );
}

/// Строка «✓ Готово» или текст ошибки под кнопками.
/// `T: AsRef<str>` — подходит и `&'static str`, и `String`.
fn result_line<T: AsRef<str>>(ui: &mut egui::Ui, result: Option<&Result<T, AppError>>) {
    match result {
        Some(Ok(text)) => status_line(ui, Icon::Check, theme::GREEN_TEXT, text.as_ref()),
        Some(Err(error)) => status_line(ui, Icon::Warning, theme::RED_TEXT, &error.to_string()),
        None => {}
    }
}

/// Только для отладки: «выбор чемпионов» из тестовых аккаунтов (первый — вы) и одного
/// скрытого игрока — чтобы проверить вкладку «Лобби» по скриншоту без настоящей игры.
fn debug_lobby(accounts: &[Account]) -> LobbyState {
    use crate::lobby::{LobbyPlayer, PlayerProfile, Position};
    let positions = [Position::Top, Position::Jungle, Position::Middle, Position::Bottom];
    let mut players: Vec<LobbyPlayer> = accounts
        .iter()
        .take(4)
        .enumerate()
        .map(|(index, account)| LobbyPlayer {
            puuid: format!("demo-{index}"),
            riot_id: Some(account.riot_id.clone()),
            is_self: index == 0,
            cell_id: Some(index as i64),
            position: positions.get(index).copied(),
            champion: account
                .cache
                .as_ref()
                .and_then(|c| c.matches.as_ref()?.first())
                .map(|g| g.me.champion.clone()),
            profile: account.cache.as_ref().map(|c| PlayerProfile {
                profile_icon_id: c.profile_icon_id,
                summoner_level: c.summoner_level,
                ranked: c.ranked.clone(),
            }),
            met: None,
        })
        .collect();
    players.push(LobbyPlayer {
        cell_id: Some(4),
        position: Some(Position::Utility),
        ..LobbyPlayer::default()
    });
    // Противники — те же аккаунты, с выдуманной формой (только для скриншотов).
    let enemies = players
        .iter()
        .enumerate()
        .map(|(index, p)| crate::lobby::EnemyPlayer {
            puuid: p.puuid.clone(),
            riot_id: p.riot_id.clone(),
            champion: p.champion.clone(),
            position: p.position,
            profile: p.profile.clone(),
            met: (index == 1).then(|| crate::lobby::Encounters {
                with: 1,
                with_wins: 1,
                against: 2,
                against_wins: 1,
                last_at: unix_now() - 3 * 86_400,
                last_account: "Main#RU1".into(),
                last_champion: "Ари".into(),
            }),
            form: (index < 4).then(|| crate::lobby::RecentForm {
                games: 20,
                wins: [13, 8, 11, 6][index],
                kills: 120,
                deaths: 80,
                assists: 140,
                champion_games: [9, 0, 2, 1][index],
                champion_wins: [7, 0, 1, 0][index],
                streak: [4, -3, 1, -1][index],
            }),
            ..crate::lobby::EnemyPlayer::default()
        })
        .collect();
    LobbyState {
        game: None,
        phase: LobbyPhase::InGame,
        players,
        problem: None,
        enemies,
    }
}

/// «сессия 3W 2L +17 LP» в шапке; подробности — при наведении.
fn session_label(ui: &mut egui::Ui, session: &Session) {
    let font = theme::mono(12.0);
    let format = |color| egui::text::TextFormat::simple(font.clone(), color);
    let mut job = egui::text::LayoutJob::default();
    job.append("сессия ", 0.0, format(theme::TEXT_HINT));
    let record = match session.losses() {
        Some(losses) => format!("{}W {losses}L", session.wins()),
        None => format!("{}W", session.wins()),
    };
    job.append(&record, 0.0, format(theme::TEXT));
    for (queue, progress) in &session.queues {
        // Две очереди — подписываем, какая где; одна — просто число.
        let text = if session.queues.len() > 1 {
            format!(" {} {}", queue.label().to_lowercase(), history::signed_lp(progress.lp))
        } else {
            format!(" {}", history::signed_lp(progress.lp))
        };
        job.append(&text, 0.0, format(rank_history::lp_color(progress.lp)));
    }
    ui.label(job).on_hover_ui(|ui| {
        ui.label(RichText::new("Игровая сессия").size(12.5).semibold().color(theme::TEXT));
        for (queue, progress) in &session.queues {
            ui.label(
                RichText::new(format!("{}: {} · {}", queue.label(), history::signed_lp(progress.lp), progress.record()))
                    .size(11.5)
                    .color(theme::TEXT),
            );
        }
        ui.label(
            RichText::new(format!(
                "Началась {}. Сессия — игры без перерывов дольше 3 часов.",
                crate::ui::format::relative_time(session.started_at)
            ))
            .size(11.0)
            .color(theme::TEXT_MUTED),
        );
    });
}

/// Состояние клиента для шапки: текст, цвет и подсказка.
fn terminal_status(status: &LcuStatus, phase: LobbyPhase) -> (String, Color32, &'static str) {
    match status {
        LcuStatus::Connected { region, riot_id, .. } if phase == LobbyPhase::InGame => (
            format!("● в игре · {region} · {riot_id}"),
            theme::ACCENT_TEXT,
            "Идёт игра. Когда она закончится, аккаунт обновится сам",
        ),
        LcuStatus::Connected { region, riot_id, .. } => (
            format!("● клиент {region} · {riot_id}"),
            theme::GREEN_TEXT,
            "Данные аккаунтов этого сервера берутся из клиента League",
        ),
        LcuStatus::Loading => (
            "○ клиент загружается".to_string(),
            theme::AMBER_TEXT,
            "Клиент запущен, но вход ещё не выполнен или он загружается",
        ),
        LcuStatus::NotRunning => (
            "○ клиент не запущен".to_string(),
            theme::AMBER_TEXT,
            "Запустите клиент — данные будут обновляться без API-ключа",
        ),
    }
}

/// Вкладка экрана в шапке: «1 аккаунты». Выбранная — янтарная цифра и черта снизу.
/// Возвращает true при щелчке.
fn screen_tab(ui: &mut egui::Ui, screen: Screen, active: bool, dot: bool, bar_bottom: f32) -> bool {
    let font = theme::mono(12.5);
    let format = |color| egui::text::TextFormat::simple(font.clone(), color);
    let mut job = egui::text::LayoutJob::default();
    job.append(&format!("{} ", screen.number()), 0.0, format(if active { theme::ACCENT_TEXT } else { theme::TEXT_HINT }));
    job.append(screen.label(), 0.0, format(if active { theme::TEXT } else { theme::TEXT_MUTED }));
    if dot {
        job.append(" ●", 0.0, format(theme::ACCENT));
    }
    let response = ui
        .add(egui::Label::new(job).sense(egui::Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("Ctrl+{}", screen.number()));
    if active {
        ui.painter().hline(
            response.rect.x_range(),
            bar_bottom - 1.5,
            egui::Stroke::new(2.0, theme::ACCENT),
        );
    }
    response.clicked()
}

/// Текст-кнопка моноширинным шрифтом с подсказкой. Возвращает true при щелчке.
fn link(ui: &mut egui::Ui, text: &str, color: Color32, hint: &str) -> bool {
    ui.add(egui::Label::new(RichText::new(text).font(theme::mono(12.5)).color(color)).sense(egui::Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hint)
        .clicked()
}

/// Нижняя строка, когда командная строка закрыта: последнее сообщение, лимит Riot
/// или подсказка, как открыть команды.
fn message_line(ui: &mut egui::Ui, notice: Option<&Notice>, rate_limit_left: Option<Duration>, accounts: bool) {
    let font = theme::mono(12.0);
    let (text, color) = match (notice, rate_limit_left) {
        (Some(n), _) if n.is_error => (format!("! {}", n.text), theme::RED_TEXT),
        (Some(n), _) => (format!("» {}", n.text), theme::TEXT),
        (None, Some(left)) => (
            format!("лимит запросов Riot — повтор через {} с", left.as_secs() + 1),
            theme::AMBER_TEXT,
        ),
        (None, None) if accounts => (": команда   / поиск   ? все клавиши".to_string(), theme::TEXT_HINT),
        (None, None) => (": команда   ctrl+1 аккаунты   ? все клавиши".to_string(), theme::TEXT_HINT),
    };
    ui.add(egui::Label::new(RichText::new(text).font(font).color(color)).truncate());
}

/// Состояние клиента League для подвала и настроек.
struct ClientStatus {
    color: Color32,
    /// Точка со свечением — подключено.
    glow: bool,
    text: String,
}

/// «● Клиент League подключён · RU · Имя#TAG» и т. п. (цвета как в подвале reveal).
fn client_status(status: &LcuStatus, phase: LobbyPhase) -> ClientStatus {
    match status {
        LcuStatus::Connected { region, riot_id, .. } => {
            let in_game = phase == LobbyPhase::InGame;
            ClientStatus {
                color: if in_game { theme::ACCENT_LIGHT } else { theme::GREEN },
                glow: true,
                text: if in_game {
                    format!("В игре · {region} · {riot_id}")
                } else {
                    format!("Клиент League подключён · {region} · {riot_id}")
                },
            }
        }
        LcuStatus::Loading => ClientStatus {
            color: theme::AMBER,
            glow: false,
            text: "Клиент League загружается".to_string(),
        },
        LcuStatus::NotRunning => ClientStatus {
            color: theme::AMBER,
            glow: false,
            text: "Клиент League не запущен".to_string(),
        },
    }
}

// Реализуем трейт (аналог интерфейса) eframe::App для нашей структуры.
// egui — «immediate mode»: интерфейс описывается заново каждый кадр, как render() в React,
// только без виртуального DOM — функция просто рисует то, что нужно сейчас.
impl eframe::App for App {
    /// Вызывается перед каждым кадром. Забираем все накопившиеся события:
    /// `try_recv` не ждёт, поэтому окно не тормозит, даже если событий нет.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Some(event) = self.backend.try_recv() {
            self.handle_event(event);
        }
        self.desktop_events(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Фон — первым: всё остальное рисуется поверх (панели прозрачные).
        window_frame::background(ui);
        self.request_images();
        // Context — дешёвый клон (указатель).
        let ctx = ui.ctx().clone();
        self.close_to_tray(&ctx);
        self.expire_notice(&ctx);
        // Арты образов, которые давно не рисовались, — из памяти (их тысячи на «Обоях»).
        self.images.evict_unused(&ctx, Instant::now());
        // Клавиши — до виджетов: найденные нажатия забираем, чтобы их не увидели кнопки.
        let mut actions = self.hotkeys(&ctx);
        // Порядок важен: сначала верхняя и нижняя панели, CentralPanel — последней.
        self.header(ui);
        actions.extend(self.footer(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(Margin {
                left: 16,
                right: 16,
                top: 12,
                bottom: 8,
            }))
            .show(ui, |ui| match self.page {
                Page::Accounts => actions.extend(self.accounts_page(ui)),
                Page::Lobby => {
                    // Клиент сообщает об идущем турнире Clash — показываем его вместо лобби.
                    if self.clash.is_shown() {
                        if let Some(request) = clash_view::show(ui, &self.clash) {
                            self.open_clash_history(request);
                        }
                    } else if lobby_view::show(ui, &self.lobby, self.client_region(), &self.clash.schedule) {
                        self.notify("Профиль открыт в браузере");
                    }
                }
                Page::Wallpapers => {
                    // Каталог грузим один раз — при первом открытии вкладки.
                    if !self.wallpapers.requested {
                        self.wallpapers.requested = true;
                        self.wallpapers.loading = true;
                        self.backend.send(Command::LoadSkins);
                    }
                    let connected = self.lcu_connected();
                    let (action, needed) = wallpapers_view::show(ui, &mut self.wallpapers, &self.images, connected);
                    // Арты видимых плиток и превью — каждый просим один раз.
                    for key in self.images.take_missing(needed) {
                        self.backend.send(Command::LoadImage(key));
                    }
                    match action {
                        wallpapers_view::Action::Load => {
                            self.wallpapers.loading = true;
                            self.wallpapers.load_error = None;
                            self.backend.send(Command::LoadSkins);
                        }
                        wallpapers_view::Action::Set(id) => {
                            self.wallpapers.setting = true;
                            self.wallpapers.status = None;
                            self.backend.send(Command::SetBackground { skin_id: id });
                        }
                        wallpapers_view::Action::None => {}
                    }
                }
                Page::Settings => self.settings_page(ui, frame),
            });
        if !actions.is_empty() {
            ctx.request_repaint();
        }
        for action in actions {
            self.run(action, &ctx);
        }
        // Карточка «после игры» и модальные окна — поверх всего.
        if let Some(card) = &mut self.post_game
            && card.show(&ctx, &self.images)
        {
            self.post_game = None;
        }
        self.dialogs(&ctx);
        // Установщик новой версии запущен: закрываемся, чтобы он смог заменить exe-файл.
        if self.update.installing {
            self.desktop.quitting = true; // не в трей, а закрыться по-настоящему
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        window_frame::border(&ctx);
        // Последним: курсор «изменить размер» у краёв окна не должен перебиваться.
        window_frame::resize_edges(&ctx);
    }

    /// Что сохранить между запусками, кроме размера и положения окна (их eframe
    /// сохраняет сам). eframe вызывает это при выходе и раз в 30 секунд; всё
    /// попадает в window.ron в папке данных. Только несекретное: сейчас — сортировка.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(SORT_KEY, self.view.sort.key().to_string());
        let tray = if self.desktop.tray.is_some() { "1" } else { "0" };
        storage.set_string(TRAY_KEY, tray.to_string());
        let notifications = if self.desktop.notifications { "1" } else { "0" };
        storage.set_string(NOTIFY_KEY, notifications.to_string());
        let hotkey = if self.desktop.hotkey.is_some() { "1" } else { "0" };
        storage.set_string(HOTKEY_KEY, hotkey.to_string());
    }

    /// Внутреннее состояние egui (прокрутка, раскрытые блоки и т. п.) не сохраняем:
    /// пользы мало, а в файле на диске лучше держать только то, что мы знаем поимённо.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    /// Окно без рамки красим сами (см. `window_frame::background`); цвет очистки — фон.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }
}
