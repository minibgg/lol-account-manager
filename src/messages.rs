//! Сообщения между окном (UI-поток) и фоновым потоком (backend, tokio-runtime).
//!
//! ```text
//!   UI ──Command──▶ backend   «обнови аккаунт 3», «сохрани ключ»
//!   UI ◀──Event──── backend   «аккаунт 3 обновлён», «ошибка: аккаунт не найден»
//! ```
//!
//! Это похоже на postMessage между страницей и Web Worker в JS: потоки не трогают
//! данные друг друга, а обмениваются сообщениями. Отправленное значение *перемещается*
//! (move) к получателю, у отправителя его больше нет. Поэтому двум потокам нечего
//! менять одновременно, и гонок данных не бывает — это проверяет компилятор.
//!
//! Безопасность: у всех типов здесь есть `Debug`, и команды пишутся в лог
//! (`RUST_LOG=debug`). Поэтому секреты лежат только в типах, чей `Debug` печатает `***`:
//! [`Secret`], `Credentials`, `SecretChange`. Сам пароль из хранилища вообще не проходит
//! через UI: копирование в буфер целиком делает backend.

use std::fmt;
use std::path::PathBuf;

use zeroize::Zeroize;

use crate::error::AppError;
use crate::images::{ImageBytes, ImageKey};
use crate::lcu::LcuStatus;
use crate::lobby::{LiveGame, LobbyState};
use crate::riot::{DataSource, Inventory, MatchDetails, MatchSummary, RankedQueues, RiotId};
use crate::storage::{Account, AccountId, AccountInput, Credentials, SecretChange};
use crate::update::UpdateInfo;

/// Папка игры: какую указал пользователь и где игра нашлась на самом деле.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeagueDirInfo {
    /// Папка из настроек (`None` — искать автоматически).
    pub custom: Option<String>,
    /// Первая папка, где есть LeagueClient.exe (`None` — игра не найдена).
    pub found: Option<PathBuf>,
}

/// Строка-секрет (например, API-ключ из поля ввода).
/// `Debug` печатает `Secret(***)`, а `Display` нет вовсе — случайно вывести не получится.
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Явный доступ к значению. Отдельный метод с говорящим именем: в коде сразу видно,
    /// где секрет «раскрывается».
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Когда секрет больше не нужен, его байты в памяти затираются нулями (crate zeroize):
/// иначе строка просто «освобождается» и её содержимое остаётся лежать в памяти,
/// пока это место не займёт что-то другое.
impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// Что копируем в буфер обмена.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    Login,
    Password,
}

impl CredentialKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Login => "Логин",
            Self::Password => "Пароль",
        }
    }
}

/// Команды от окна к фоновому потоку.
#[derive(Debug)]
pub enum Command {
    /// Перечитать список аккаунтов из базы.
    ReloadAccounts,
    /// Добавить аккаунт. Riot ID сначала проверяется (через клиент League или Riot API),
    /// если хоть один из них доступен.
    AddAccount {
        input: AccountInput,
        credentials: Credentials,
    },
    /// Изменить аккаунт. Для логина и пароля — отдельное решение: оставить, стереть, заменить.
    UpdateAccount {
        id: AccountId,
        input: AccountInput,
        login: SecretChange,
        password: SecretChange,
    },
    DeleteAccount(AccountId),
    /// Обновить уровень, иконку и ранги одного аккаунта (клиент League или Riot API).
    RefreshAccount(AccountId),
    /// Обновить все аккаунты (с соблюдением лимитов Riot API, если идём через него).
    RefreshAll,
    /// Скопировать логин или пароль в буфер обмена (очистится через 30 с).
    CopyCredential { id: AccountId, kind: CredentialKind },
    /// Кнопка «Войти»: скопировать логин и запустить Riot Client (как ярлык на рабочем
    /// столе). В клиент ничего не вводится — логин и пароль пользователь вставляет сам.
    StartLogin(AccountId),
    /// Shift+Enter: войти сразу — запустить Riot Client и самим ввести логин и пароль
    /// в его окно входа (см. autologin.rs).
    AutoLogin(AccountId),
    /// Иконка профиля или эмблема ранга (из кэша на диске или из сети).
    LoadImage(ImageKey),
    /// Папка игры из настроек (`None` — искать автоматически).
    SetLeagueDir(Option<String>),
    /// Проверить API-ключ: введённый в поле (`Some`) или уже сохранённый (`None`).
    CheckApiKey(Option<Secret>),
    /// Проверить ключ в Riot API и, если он рабочий, сохранить в системное хранилище.
    SaveApiKey(Secret),
    DeleteApiKey,
    /// Сохранить список аккаунтов в файл (без логинов и паролей, см. transfer.rs).
    ExportAccounts(PathBuf),
    /// Добавить аккаунты из файла экспорта (те, которых ещё нет в списке).
    ImportAccounts(PathBuf),
    /// Сохранить полную копию — список вместе с логинами и паролями, — зашифрованную
    /// этим паролем (см. vault.rs).
    ExportVault { path: PathBuf, password: Secret },
    /// Восстановить аккаунты (с логинами и паролями) из зашифрованной копии.
    ImportVault { path: PathBuf, password: Secret },
    /// Подробности игры (обе команды) — для окна, которое открывается щелчком по игре.
    LoadMatch { account: AccountId, match_id: String },
    /// Все игры аккаунта из архива — для окна «Все игры».
    LoadGames(AccountId),
    /// Инвентарь аккаунта — для окна «Профиль»: свежий из клиента League, если в нём
    /// выполнен вход в этот аккаунт, иначе последний сохранённый.
    LoadInventory(AccountId),
    /// Нынешние ранги игроков одной игры — для окна «Подробности игры».
    /// `account` нужен ради сервера: игроки игры — с того же сервера, что и аккаунт.
    LoadRanks {
        account: AccountId,
        match_id: String,
        players: Vec<RiotId>,
    },
    /// Проверить, нет ли новой версии приложения (кнопка в настройках).
    CheckUpdate,
    /// Скачать установщик этой версии, проверить его и запустить.
    InstallUpdate(UpdateInfo),
    /// Проверять ли обновления при запуске.
    SetUpdateCheck(bool),
    /// Включить или выключить «спрашивать Windows Hello перед копированием пароля».
    /// И включение, и выключение сами требуют проверки Windows Hello.
    SetHelloLock(bool),
}

/// Итог импорта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    /// Уже были в списке.
    pub duplicates: usize,
    /// Записи с ошибкой в файле (неверный Riot ID или регион).
    pub invalid: usize,
}

/// События от фонового потока к окну.
#[derive(Debug)]
pub enum Event {
    /// Фоновый поток запущен, база открыта. `has_api_key` — сохранён ли ключ.
    Ready {
        has_api_key: bool,
        league_dir: LeagueDirInfo,
        /// Включена ли проверка обновлений при запуске.
        update_check: bool,
        /// Включена ли проверка Windows Hello перед копированием пароля.
        hello_lock: bool,
    },
    /// База не открылась — без неё приложение работать не может.
    StartupFailed(AppError),
    /// Полный список аккаунтов (после запуска или `ReloadAccounts`).
    Accounts(Vec<Account>),
    /// Клиент League запустился, закрылся или в нём сменился аккаунт.
    LcuStatus(LcuStatus),
    /// Изменилось лобби: группа, выбор чемпионов, игра.
    Lobby(LobbyState),
    /// Кто из аккаунтов (кроме того, что в клиенте) сейчас в игре — по данным Riot API.
    LiveGames(Vec<(AccountId, LiveGame)>),

    /// Аккаунт добавлен. `fetch_error` — почему не удалось сразу загрузить данные из Riot
    /// (нет ключа, нет сети...): аккаунт сохранён, обновить его можно позже.
    AccountAdded {
        account: Account,
        fetch_error: Option<AppError>,
    },
    AddFailed(AppError),
    AccountEdited(Account),
    EditFailed { id: AccountId, error: AppError },
    AccountDeleted(AccountId),
    DeleteFailed { id: AccountId, error: AppError },

    /// Аккаунт поставлен в очередь на обновление (показать индикатор загрузки).
    RefreshQueued(AccountId),
    /// Аккаунт обновлён; `source` — откуда пришли данные (клиент League или Riot API).
    AccountRefreshed { account: Account, source: DataSource },
    RefreshFailed { id: AccountId, error: AppError },
    /// «Обновить все» закончилось.
    RefreshAllDone { ok: usize, failed: usize },
    /// Riot ответил 429: запросы приостановлены на `secs` секунд, потом повтор.
    RateLimitWait { secs: u64 },

    /// В буфере обмена логин или пароль; через `clear_after_secs` он будет стёрт.
    CredentialCopied {
        kind: CredentialKind,
        clear_after_secs: u64,
    },
    CopyFailed(AppError),
    /// Буфер обмена очищен по таймеру.
    ClipboardCleared,
    /// «Войти»: Riot Client запущен (или выведен на передний план).
    RiotClientLaunched,
    /// Автовход: что происходит сейчас («жду окно входа…») — для строки внизу окна.
    AutoLoginProgress(&'static str),
    /// Автовход закончился: текст для строки внизу окна или ошибка.
    AutoLoginDone(Result<&'static str, AppError>),

    ImageLoaded { key: ImageKey, bytes: ImageBytes },
    ImageFailed(ImageKey),
    LeagueDirSaved(Result<LeagueDirInfo, AppError>),

    ApiKeyChecked(Result<(), AppError>),
    ApiKeySaved(Result<(), AppError>),
    ApiKeyDeleted(Result<(), AppError>),

    /// Экспорт: сколько аккаунтов сохранено в файл.
    Exported(Result<usize, AppError>),
    /// Импорт закончен (новые аккаунты уже в списке и поставлены на обновление).
    Imported(Result<ImportReport, AppError>),

    /// Ответ на `LoadMatch`.
    MatchLoaded {
        match_id: String,
        result: Result<MatchDetails, AppError>,
    },
    /// Ответ на `LoadGames`: игры из архива, новые первыми.
    GamesLoaded {
        account: AccountId,
        result: Result<Vec<MatchSummary>, AppError>,
    },

    /// Ответ на `LoadInventory`: `None` — инвентарь этого аккаунта ещё ни разу не видели.
    InventoryLoaded {
        account: AccountId,
        inventory: Option<Inventory>,
    },

    /// Ответ на `LoadRanks`: ранги тех игроков, кого удалось найти.
    RanksLoaded {
        match_id: String,
        result: Result<Vec<(RiotId, RankedQueues)>, AppError>,
    },

    /// Проверка обновлений закончилась: `Ok(None)` — установлена последняя версия.
    /// `manual` — проверку запустил пользователь (тогда показываем и «обновлений нет»,
    /// и ошибку; о неудачной фоновой проверке окно вообще не узнаёт).
    UpdateChecked {
        result: Result<Option<UpdateInfo>, AppError>,
        manual: bool,
    },
    /// Установщик скачивается: сколько байт получено из скольких.
    UpdateProgress { received: u64, total: u64 },
    /// Установщик проверен и запущен — окну пора закрыться, чтобы он заменил программу.
    UpdateInstalling,
    UpdateFailed(AppError),

    /// Ответ на `SetHelloLock`: новое состояние настройки или почему оно не изменилось.
    HelloLockSaved(Result<bool, AppError>),

    /// Ошибка, не привязанная к конкретному действию (например, хранилище паролей
    /// недоступно при запуске).
    Warning(AppError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::{Region, RiotId};

    /// Команды пишутся в лог через `{:?}` — секретов там быть не должно.
    #[test]
    fn commands_debug_hides_secrets() {
        let input = AccountInput {
            riot_id: RiotId::parse("Name#TAG").unwrap(),
            region: Region::Euw,
            note: String::new(),
            tags: vec![],
        };
        let commands = [
            Command::AddAccount {
                input: input.clone(),
                credentials: Credentials::new(Some("my-login".into()), Some("my-pass".into())),
            },
            Command::UpdateAccount {
                id: 1,
                input,
                login: SecretChange::Set("my-login".into()),
                password: SecretChange::Set("my-pass".into()),
            },
            Command::SaveApiKey(Secret::new("RGAPI-my-key")),
            Command::CheckApiKey(Some(Secret::new("RGAPI-my-key"))),
            Command::ExportVault {
                path: "copy.lolvault".into(),
                password: Secret::new("my-pass"),
            },
            Command::ImportVault {
                path: "copy.lolvault".into(),
                password: Secret::new("my-pass"),
            },
        ];
        for command in &commands {
            let text = format!("{command:?}");
            for secret in ["my-login", "my-pass", "RGAPI"] {
                assert!(!text.contains(secret), "{secret} попал в лог: {text}");
            }
        }
    }
}
