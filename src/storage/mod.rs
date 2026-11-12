//! Хранилище приложения: SQLite для обычных данных + системное хранилище для секретов.
//!
//! `Storage` — единая точка входа. Она следит, чтобы база и хранилище паролей
//! менялись согласованно (например, при удалении аккаунта удаляется и его пароль).

pub mod db;
pub mod secrets;

use std::fmt;
use std::path::{Path, PathBuf};

pub use db::{Account, AccountId, AccountInput, CachedProfile, Db, normalize_tags, unix_now};
pub use secrets::{Credentials, Secrets};

use crate::error::{AppError, AppResult};
use crate::riot::{AccountSnapshot, ApiKey, MatchDetails, MatchSummary};

pub const APP_DIR: &str = "lol-account-manager";
pub const DB_FILE: &str = "accounts.db";
/// Переменная окружения с другой папкой данных (для тестов и экспериментов).
pub const DATA_DIR_ENV: &str = "LOLAM_DATA_DIR";

/// Папка данных приложения (создаётся при первом обращении):
/// - Windows: `%APPDATA%\lol-account-manager`
/// - Linux:   `~/.local/share/lol-account-manager`
///
/// Переменная окружения `LOLAM_DATA_DIR` задаёт другую папку (для тестов и экспериментов).
pub fn data_dir() -> AppResult<PathBuf> {
    let dir = match std::env::var_os(DATA_DIR_ENV) {
        Some(custom) => PathBuf::from(custom),
        None => dirs::data_dir()
            .ok_or_else(|| AppError::Io("не найдена папка данных пользователя".into()))?
            .join(APP_DIR),
    };
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Что сделать с логином или паролем при редактировании аккаунта.
pub enum SecretChange {
    /// Не трогать (поле в форме оставили пустым).
    Keep,
    /// Удалить.
    Clear,
    /// Заменить новым значением.
    Set(String),
}

impl SecretChange {
    fn apply(self, slot: &mut Option<String>) {
        match self {
            Self::Keep => {}
            Self::Clear => *slot = None,
            Self::Set(value) => *slot = Some(value),
        }
    }
}

// Debug вручную: значение `Set(...)` — это пароль, печатать его нельзя.
impl fmt::Debug for SecretChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Keep => "Keep",
            Self::Clear => "Clear",
            Self::Set(_) => "Set(***)",
        })
    }
}

pub struct Storage {
    db: Db,
    secrets: Secrets,
}

impl Storage {
    /// Обычный режим: база в папке данных приложения, секреты в системном хранилище.
    pub fn open_default() -> AppResult<Self> {
        Self::open(&data_dir()?.join(DB_FILE), Secrets::new())
    }

    pub fn open(db_path: &Path, secrets: Secrets) -> AppResult<Self> {
        Ok(Self {
            db: Db::open(db_path)?,
            secrets,
        })
    }

    pub fn default_db_path() -> AppResult<PathBuf> {
        Ok(data_dir()?.join(DB_FILE))
    }

    // ----- Чтение -----------------------------------------------------------

    pub fn list_accounts(&self) -> AppResult<Vec<Account>> {
        self.db.list_accounts()
    }

    pub fn get_account(&self, id: AccountId) -> AppResult<Account> {
        self.db.get_account(id)
    }

    /// Логин и пароль аккаунта — для кнопок «Скопировать» и формы редактирования.
    pub fn credentials(&self, id: AccountId) -> AppResult<Credentials> {
        let account = self.db.get_account(id)?;
        if !account.has_login && !account.has_password {
            return Ok(Credentials::default()); // в хранилище даже не ходим
        }
        self.secrets.credentials(&account.secret_ref)
    }

    // ----- Изменение --------------------------------------------------------

    /// Добавляет аккаунт. Если сохранить логин/пароль не удалось, аккаунт тоже
    /// не добавляется — чтобы не было «аккаунта с потерянным паролем».
    pub fn add_account(&mut self, input: &AccountInput, creds: Credentials) -> AppResult<Account> {
        let account = self.db.insert_account(input)?;
        if creds.is_empty() {
            return Ok(account);
        }
        if let Err(e) = self.secrets.set_credentials(&account.secret_ref, &creds) {
            // Откат: удаляем только что добавленную строку. Ошибку удаления
            // игнорируем (`let _ =`) — главное вернуть исходную ошибку хранилища.
            let _ = self.db.delete_account(account.id);
            return Err(e);
        }
        self.db
            .set_credential_flags(account.id, creds.login.is_some(), creds.password.is_some())?;
        self.db.get_account(account.id)
    }

    /// Редактирование: обычные поля + отдельные решения для логина и пароля.
    pub fn update_account(
        &mut self,
        id: AccountId,
        input: &AccountInput,
        login: SecretChange,
        password: SecretChange,
    ) -> AppResult<Account> {
        let account = self.db.update_account(id, input)?;

        if !matches!((&login, &password), (SecretChange::Keep, SecretChange::Keep)) {
            let mut creds = self.secrets.credentials(&account.secret_ref)?;
            login.apply(&mut creds.login);
            password.apply(&mut creds.password);
            let creds = Credentials::new(creds.login, creds.password);
            self.secrets.set_credentials(&account.secret_ref, &creds)?;
            self.db
                .set_credential_flags(id, creds.login.is_some(), creds.password.is_some())?;
        }
        self.db.get_account(id)
    }

    /// Удаляет аккаунт вместе с логином и паролем.
    /// Сначала секреты: если хранилище недоступно, аккаунт останется на месте
    /// и пароль не «осиротеет» в системе без возможности его удалить из приложения.
    pub fn delete_account(&mut self, id: AccountId) -> AppResult<()> {
        let account = self.db.get_account(id)?;
        self.secrets.delete_credentials(&account.secret_ref)?;
        self.db.delete_account(id)
    }

    /// Сохраняет результат обновления из Riot API.
    pub fn save_snapshot(&mut self, id: AccountId, snapshot: &AccountSnapshot) -> AppResult<()> {
        self.db.save_snapshot(id, snapshot, unix_now())
    }

    // ----- Архив игр (см. Db) -----------------------------------------------

    pub fn archived_matches(&self, id: AccountId, limit: Option<usize>) -> AppResult<Vec<MatchSummary>> {
        self.db.archived_matches(id, limit)
    }

    pub fn archived_details(&self, id: AccountId, match_id: &str) -> AppResult<Option<MatchDetails>> {
        self.db.archived_details(id, match_id)
    }

    pub fn save_details(&mut self, id: AccountId, details: &MatchDetails) -> AppResult<()> {
        self.db.save_details(id, details)
    }

    pub fn matches_without_details(&self, id: AccountId, limit: usize) -> AppResult<Vec<String>> {
        self.db.matches_without_details(id, limit)
    }

    // ----- Настройки --------------------------------------------------------

    pub fn setting(&self, key: &str) -> AppResult<Option<String>> {
        self.db.setting(key)
    }

    pub fn set_setting(&mut self, key: &str, value: &str) -> AppResult<()> {
        self.db.set_setting(key, value)
    }

    pub fn delete_setting(&mut self, key: &str) -> AppResult<()> {
        self.db.delete_setting(key)
    }

    // ----- API-ключ ---------------------------------------------------------

    pub fn api_key(&self) -> AppResult<Option<ApiKey>> {
        self.secrets.api_key()
    }

    pub fn set_api_key(&self, raw: &str) -> AppResult<()> {
        self.secrets.set_api_key(raw)
    }

    pub fn delete_api_key(&self) -> AppResult<()> {
        self.secrets.delete_api_key()
    }
}
