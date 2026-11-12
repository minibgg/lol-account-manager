//! Единый тип ошибки приложения.
//!
//! В Rust нет исключений: функция, которая может не сработать, возвращает
//! `Result<T, E>` — либо `Ok(значение)`, либо `Err(ошибка)`. Оператор `?` после вызова
//! означает «если Err — сразу вернуть его из текущей функции, иначе достать значение».

use thiserror::Error;

/// Все ошибки, которые может увидеть пользователь.
///
/// `#[derive(Error)]` (crate thiserror) генерирует реализацию `std::error::Error`,
/// а `#[error("...")]` задаёт текст для `to_string()` / `{}` — его и покажем в UI.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AppError {
    #[error("Неверный Riot ID: нужен формат «Имя#TAG»")]
    InvalidRiotId,

    #[error("Неизвестный регион «{0}»")]
    UnknownRegion(String),

    #[error("API-ключ не задан — укажите его в настройках")]
    MissingApiKey,

    #[error("Аккаунт не найден")]
    AccountNotFound,

    #[error("Игра не найдена — возможно, она слишком старая")]
    MatchNotFound,

    /// Riot-аккаунт существует, но на выбранном сервере у него нет профиля LoL.
    /// Почти всегда это значит, что выбран не тот регион.
    #[error("Аккаунт найден, но на сервере {0} у него нет профиля LoL — проверьте регион")]
    SummonerNotFound(&'static str),

    #[error("API-ключ недействителен или истёк")]
    InvalidApiKey,

    #[error("Лимит запросов, повтор через {retry_after_secs} сек")]
    RateLimited { retry_after_secs: u64 },

    #[error("Riot API отклонил запрос (HTTP 400) — проверьте Riot ID и регион")]
    BadRequest,

    #[error("Серверы Riot временно недоступны (HTTP {0}), попробуйте позже")]
    RiotUnavailable(u16),

    #[error("Неожиданный ответ сервера: HTTP {0}")]
    UnexpectedStatus(u16),

    #[error("Нет связи с сервером: {0}")]
    Network(String),

    #[error("Не удалось разобрать ответ сервера: {0}")]
    Decode(String),

    // ----- Клиент League (LCU) -----
    #[error("Клиент League of Legends не запущен")]
    LcuNotRunning,

    #[error("Клиент League of Legends ещё загружается — попробуйте через несколько секунд")]
    LcuNotReady,

    #[error("В клиенте League of Legends не выполнен вход")]
    LcuNotLoggedIn,

    /// Клиент запущен, но на другом сервере, а API-ключа нет.
    #[error("Клиент League подключён к {client}, а аккаунт — с {account}. Для других серверов укажите API-ключ в настройках")]
    WrongClientRegion {
        client: &'static str,
        account: &'static str,
    },

    #[error("Нет источника данных: запустите клиент League или укажите API-ключ в настройках")]
    NoDataSource,

    /// Не удалось прочитать аргументы процесса LeagueClientUx.exe (порт и токен Riot Client).
    #[error("Не удалось прочитать данные клиента League. Если League запущена от имени администратора, запустите менеджер тоже от имени администратора")]
    LeagueArgsUnreadable,

    #[error("Чат клиента ещё не подключился — имена появятся через несколько секунд")]
    ChatUnavailable,

    #[error("Папка «{0}» не найдена")]
    LeagueDirNotFound(String),

    #[error("В этой папке нет LeagueClient.exe — укажите папку, куда установлена League of Legends")]
    LeagueDirInvalid,

    // ----- Riot Client (кнопка «Войти») -----
    #[error("Riot Client не найден — запустите League of Legends вручную")]
    RiotClientNotFound,

    #[error("Не удалось запустить Riot Client: {0}")]
    RiotClientLaunch(String),

    // ----- Хранилище -----
    #[error("Такой аккаунт уже есть в списке")]
    DuplicateAccount,

    #[error("Аккаунт не найден в базе")]
    AccountMissing,

    #[error("Ошибка базы данных: {0}")]
    Database(String),

    #[error("Ошибка системного хранилища паролей: {0}")]
    SecretStore(String),

    #[error("Слишком длинный логин или пароль для системного хранилища")]
    SecretTooLong,

    #[error("Ошибка файловой системы: {0}")]
    Io(String),

    // ----- Экспорт и импорт списка -----
    #[error("Это не файл списка аккаунтов LoL Account Manager")]
    NotAnExportFile,

    #[error("Файл создан более новой версией приложения (формат v{0}) — обновите приложение")]
    ExportTooNew(u32),

    #[error("Файл слишком большой для списка аккаунтов")]
    ImportTooLarge,

    // ----- Буфер обмена и прочее -----
    /// Для этого аккаунта не сохранён логин / пароль. Внутри — «Логин» или «Пароль».
    #[error("{0} для этого аккаунта не сохранён")]
    CredentialMissing(&'static str),

    #[error("Не удалось скопировать в буфер обмена: {0}")]
    Clipboard(String),

    #[error("Картинка не загрузилась: {0}")]
    Image(String),

    /// Сбой внутри приложения (например, фоновая задача аварийно завершилась).
    #[error("Внутренняя ошибка: {0}")]
    Internal(String),
}

/// Сокращение: `AppResult<T>` вместо `Result<T, AppError>`.
pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    /// Нет связи с сервером (нет интернета, сервер не отвечает). Сохранённые данные
    /// при этом остаются на экране — интерфейс говорит об этом коротко.
    pub fn is_offline(&self) -> bool {
        matches!(self, Self::Network(_))
    }

    /// HTTP-статус ответа Riot API → ошибка.
    /// 404 и 429 сюда не попадают: их обрабатывает клиент (см. riot/client.rs).
    pub fn from_status(status: u16) -> Self {
        match status {
            400 => Self::BadRequest,
            // 401 — ключ не передан, 403 — ключ неверный или просрочен
            // (Riot не различает «неверный путь» и «неверный ключ» в 403).
            401 | 403 => Self::InvalidApiKey,
            500 | 502 | 503 | 504 => Self::RiotUnavailable(status),
            other => Self::UnexpectedStatus(other),
        }
    }
}

/// Автоматическое преобразование ошибок reqwest в нашу: благодаря `impl From`
/// оператор `?` сам превращает `reqwest::Error` в `AppError`.
impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_decode() {
            return Self::Decode(e.to_string());
        }
        if let Some(status) = e.status() {
            return Self::UnexpectedStatus(status.as_u16());
        }
        if e.is_timeout() {
            return Self::Network("сервер не ответил вовремя".into());
        }
        if e.is_connect() {
            return Self::Network("не удалось подключиться — проверьте интернет".into());
        }
        // without_url(): убираем URL из текста ошибки, чтобы не засорять сообщение.
        Self::Network(e.without_url().to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        // В SQLite нет секретов, поэтому текст ошибки показывать безопасно.
        Self::Database(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Ошибки keyring переводим вручную и осторожно: варианты `BadEncoding` и
/// `BadDataFormat` содержат СЫРЫЕ БАЙТЫ СЕКРЕТА. Их нельзя печатать ни в лог, ни в UI,
/// поэтому вместо содержимого — общий текст.
impl From<keyring::Error> for AppError {
    fn from(e: keyring::Error) -> Self {
        use keyring::Error as K;
        let message = match e {
            K::TooLong(..) => return Self::SecretTooLong,
            K::NoEntry => "запись не найдена".to_string(),
            K::NoDefaultStore => "хранилище паролей в системе недоступно".to_string(),
            K::NoStorageAccess(p) => format!("нет доступа к хранилищу (заблокировано?): {p}"),
            K::PlatformFailure(p) => format!("сбой системы: {p}"),
            K::BadEncoding(_) | K::BadDataFormat(..) => "данные в хранилище повреждены".to_string(),
            // Остальные варианты печатаем без подробностей: в будущих версиях keyring
            // могут появиться новые, и мы не знаем заранее, что в них лежит.
            _ => "неизвестная ошибка хранилища".to_string(),
        };
        Self::SecretStore(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_errors_never_contain_secret_bytes() {
        let secret = b"super-secret-password".to_vec();
        let text = AppError::from(keyring::Error::BadEncoding(secret)).to_string();
        assert!(!text.contains("super"), "{text}");
        let debug = format!("{:?}", AppError::from(keyring::Error::BadEncoding(b"abc".to_vec())));
        assert!(!debug.contains("97"), "байты не должны попасть даже в Debug: {debug}");
    }

    #[test]
    fn status_mapping() {
        assert_eq!(AppError::from_status(401), AppError::InvalidApiKey);
        assert_eq!(AppError::from_status(403), AppError::InvalidApiKey);
        assert_eq!(AppError::from_status(400), AppError::BadRequest);
        assert_eq!(AppError::from_status(503), AppError::RiotUnavailable(503));
        assert_eq!(AppError::from_status(418), AppError::UnexpectedStatus(418));
    }

    #[test]
    fn only_network_errors_mean_offline() {
        assert!(AppError::Network("не удалось подключиться".into()).is_offline());
        assert!(!AppError::RiotUnavailable(503).is_offline(), "сервер ответил — связь есть");
        assert!(!AppError::InvalidApiKey.is_offline());
    }

    #[test]
    fn user_facing_messages() {
        assert_eq!(AppError::AccountNotFound.to_string(), "Аккаунт не найден");
        assert_eq!(
            AppError::InvalidApiKey.to_string(),
            "API-ключ недействителен или истёк"
        );
        assert_eq!(
            AppError::RateLimited { retry_after_secs: 7 }.to_string(),
            "Лимит запросов, повтор через 7 сек"
        );
    }
}
