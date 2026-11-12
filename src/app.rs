//! Главная структура приложения: состояние окна и связь с фоновым потоком.
//!
//! Окно ничего не ждёт: кнопки отправляют [`Command`] в фоновый поток, а результаты
//! приходят как [`Event`] и забираются в начале каждого кадра (метод `logic`).
//!
//! Части интерфейса живут в `ui/`: карточка аккаунта, форма, поиск и сортировка,
//! картинки, тема. Здесь — состояние окна, страницы и обработка событий.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Margin, RichText};

use crate::backend::{Backend, is_same_account};
use crate::error::AppError;
use crate::history::{self, Queue, Session};
use crate::images::ImageKey;
use crate::lcu::LcuStatus;
use crate::lobby::{self, LobbyPhase, LobbyState};
use crate::messages::{Command, CredentialKind, Event, LeagueDirInfo, Secret};
use crate::riot::models::RECENT_MATCHES;
use crate::riot::{Division, MatchDetails, MatchSummary, Region, RiotId};
use crate::storage::{Account, AccountId, unix_now};
use crate::transfer;
use crate::ui::account_card::CardAction;
use crate::ui::account_form::{AccountForm, FormAction};
use crate::ui::account_list::{self, ListView, SortOrder};
use crate::ui::image_store::ImageStore;
use crate::ui::theme::{self, Weight};
use crate::ui::widgets::{self, Button, Icon, Tone};
use crate::ui::match_view::{DialogAction, MatchDialog};
use crate::ui::post_game::PostGame;
use crate::ui::rank_history::{self, HistoryDialog};
use crate::ui::command::{self, Invocation, Suggestion, Target};
use crate::ui::games_view::{GamesAction, GamesView};
use crate::ui::keymap::{self, Action, Screen};
use crate::ui::terminal::{self, CommandEvent, CommandLine, Hint};
use crate::ui::{lobby_view, window_frame};

/// Сколько сообщение видно в нижней строке.
const NOTICE_DURATION: Duration = Duration::from_secs(8);

/// Сколько ждём второе нажатие («Пароль») после «Войти».
const LOGIN_FLOW_TIMEOUT: Duration = Duration::from_secs(120);

/// Ключ сортировки в window.ron.
const SORT_KEY: &str = "sort";

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
    id: AccountId,
    riot_id: RiotId,
    has_credentials: bool,
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

#[derive(Default)]
struct TransferState {
    /// Ждём, пока фоновый поток запишет или прочитает файл.
    busy: bool,
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
    /// Поиск, фильтр по тегу и сортировка над списком.
    view: ListView,
    images: ImageStore,
    /// Открытая форма добавления или редактирования (`None` — закрыта).
    form: Option<AccountForm>,
    confirm_delete: Option<ConfirmDelete>,
    /// Окно «Подробности игры» (щелчок по игре на строке аккаунта).
    match_dialog: Option<MatchDialog>,
    /// Уже загруженные подробности: повторное открытие той же игры — без запроса.
    match_cache: HashMap<String, MatchDetails>,
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
    /// Окно «Все игры» (архив игр аккаунта, клавиша m).
    games_view: Option<GamesView>,
    /// Только для отладки (LOLAM_DEBUG_PAGE=lobby-demo): лобби из тестовых аккаунтов,
    /// клиент League его не перезаписывает.
    debug_lobby: bool,
}

impl App {
    /// Вызывается один раз при старте (из main.rs).
    /// `&eframe::CreationContext<'_>` — ссылка (заимствование): мы только читаем `cc`,
    /// владеет им eframe. `'_` — время жизни ссылки, компилятор выводит его сам.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
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
            Ok("settings") if cfg!(debug_assertions) => (Page::Settings, None),
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
            accounts: Vec::new(),
            refreshing: HashSet::new(),
            errors: HashMap::new(),
            refresh_all_running: false,
            rate_limited_until: None,
            notice: None,
            settings: SettingsState::default(),
            league_dir: LeagueDirState::default(),
            transfer: TransferState::default(),
            view,
            images: ImageStore::default(),
            form,
            confirm_delete: None,
            match_dialog: None,
            match_cache: HashMap::new(),
            history_dialog: None,
            post_game: None,
            debug_page: std::env::var("LOLAM_DEBUG_PAGE")
                .ok()
                .filter(|_| cfg!(debug_assertions)),
            login_flow: None,
            cursor: None,
            scroll_to_cursor: false,
            search_open: false,
            search_focus: false,
            command: None,
            help_open: false,
            games_view: None,
            debug_lobby: false,
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
            } => {
                self.state = BackendState::Ready;
                self.has_api_key = has_api_key;
                self.league_dir.input = league_dir.custom.clone().unwrap_or_default();
                self.league_dir.info = league_dir;
            }
            Event::StartupFailed(error) => self.state = BackendState::Failed(error),
            Event::Accounts(accounts) => {
                self.accounts = accounts;
                if let Some(page) = self.debug_page.take() {
                    self.debug_open(&page);
                }
            }
            Event::LcuStatus(status) => self.lcu = status,
            Event::Lobby(state) if !self.debug_lobby => self.lobby = state,
            Event::Lobby(_) => {}

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
                }
                // Открыто «Все игры» этого аккаунта — в архиве могли появиться новые игры.
                if self.games_view.as_ref().is_some_and(|v| v.account == account.id) {
                    self.backend.send(Command::LoadGames(account.id));
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
                }
                // Окно могли закрыть или открыть другую игру — тогда ответ только в кэш.
                if let Some(dialog) = &mut self.match_dialog
                    && dialog.summary.id == match_id
                {
                    dialog.details = Some(result);
                }
            }
            Event::GamesLoaded { account, result } => {
                if let Some(view) = &mut self.games_view
                    && view.account == account
                {
                    view.games = Some(result);
                }
            }
            Event::Warning(error) => self.notify_error(&error),
        }
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
                    id,
                    riot_id: account.riot_id.clone(),
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
        let details = self.match_cache.get(&summary.id).cloned().map(Ok);
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
            "lobby-demo" => {
                self.page = Page::Lobby;
                self.debug_lobby = true;
                self.lobby = debug_lobby(&self.accounts);
            }
            "games" => {
                if let Some(account) = self.accounts.iter().find(|a| a.archived_games > 0) {
                    self.games_view = Some(GamesView::new(account.id, account.riot_id.clone()));
                    self.backend.send(Command::LoadGames(account.id));
                }
            }
            "command" => {
                self.command = Some(CommandLine {
                    input: "войти dr4".into(),
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
        let (status, status_color, status_hint) = terminal_status(&self.lcu, self.lobby.phase);
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
        let scroll_to_cursor = std::mem::take(&mut self.scroll_to_cursor);
        // Панели аккаунта нужно ~190 px, остальное — таблице.
        const DETAILS_HEIGHT: f32 = 196.0;
        let table = terminal::Table {
            accounts: &visible,
            cursor: self.cursor,
            in_client,
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
            || self.history_dialog.is_some()
            || self.confirm_delete.is_some()
            || self.help_open
            || self.games_view.is_some()
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

    /// Выполняет действие — с клавиатуры, из командной строки или щелчком.
    fn run(&mut self, invocation: Invocation, ctx: &egui::Context) {
        use Action as A;
        let Invocation { action, account } = invocation;
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
            A::Screen(screen) => self.go_to(screen),
            A::Escape => {
                self.view.search.clear();
                self.view.tag = None;
                self.search_open = false;
            }
            // С аккаунтом — в `run_on` выше.
            A::Select
            | A::Login
            | A::CopyLogin
            | A::CopyPassword
            | A::CopyRiotId
            | A::Refresh
            | A::History
            | A::Game(_)
            | A::Games
            | A::OpGg
            | A::Edit
            | A::Delete => {}
        }
    }

    /// Действие с конкретным аккаунтом. Курсор переезжает на него (важно для команд
    /// вроде «войти benjm»: сразу видно, с каким аккаунтом работаем).
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
        // «Все игры» — под подробностями игры: клавиши ему, только когда подробности закрыты.
        let keys = self.match_dialog.is_none();
        if let Some((id, action)) = self.games_view.as_mut().map(|v| (v.account, v.show(ctx, keys))) {
            match action {
                GamesAction::None => {}
                GamesAction::Close => self.games_view = None,
                GamesAction::Open(summary) => {
                    if let Some(owner) = self.accounts.iter().find(|a| a.id == id).map(|a| a.riot_id.clone()) {
                        self.open_match(id, owner, summary);
                    }
                }
            }
        }
        if let Some(dialog) = &mut self.match_dialog {
            match dialog.show(ctx, &self.images) {
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
                        ui.label(RichText::new("Удалить аккаунт?").size(15.0).semibold().color(theme::TEXT));
                        ui.label(RichText::new(confirm.riot_id.to_string()).size(12.5).color(theme::TEXT_MUTED));
                    });
                });
                ui.add_space(12.0);
                let text = if confirm.has_credentials {
                    "Сохранённые логин и пароль этого аккаунта тоже будут удалены \
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
            self.backend.send(Command::DeleteAccount(confirm.id));
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
                self.transfer_settings(ui, frame);
            });
        });
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
        if let Some(command) = command {
            self.transfer.busy = true;
            self.transfer.result = None;
            self.backend.send(command);
        }
        result_line(ui, self.transfer.result.as_ref());
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
        })
        .collect();
    players.push(LobbyPlayer {
        cell_id: Some(4),
        position: Some(Position::Utility),
        ..LobbyPlayer::default()
    });
    LobbyState {
        phase: LobbyPhase::ChampSelect,
        players,
        problem: None,
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
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Some(event) = self.backend.try_recv() {
            self.handle_event(event);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Фон — первым: всё остальное рисуется поверх (панели прозрачные).
        window_frame::background(ui);
        self.request_images();
        // Context — дешёвый клон (указатель).
        let ctx = ui.ctx().clone();
        self.expire_notice(&ctx);
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
                    if lobby_view::show(ui, &self.lobby, self.client_region()) {
                        self.notify("Профиль открыт в браузере");
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
        window_frame::border(&ctx);
        // Последним: курсор «изменить размер» у краёв окна не должен перебиваться.
        window_frame::resize_edges(&ctx);
    }

    /// Что сохранить между запусками, кроме размера и положения окна (их eframe
    /// сохраняет сам). eframe вызывает это при выходе и раз в 30 секунд; всё
    /// попадает в window.ron в папке данных. Только несекретное: сейчас — сортировка.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(SORT_KEY, self.view.sort.key().to_string());
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
