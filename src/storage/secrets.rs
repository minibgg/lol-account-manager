//! Секреты в системном хранилище паролей:
//! Windows Credential Manager / Secret Service (Linux) / Keychain (macOS).
//!
//! Что здесь хранится:
//! - API-ключ Riot — запись `riot-api-key.lol-account-manager`;
//! - логин и пароль каждого аккаунта — запись `account-<uuid>.lol-account-manager`,
//!   одна на аккаунт (JSON с двумя полями), поэтому логин и пароль меняются вместе.
//!
//! Посмотреть записи в Windows: Панель управления → Диспетчер учётных данных →
//! Учётные данные Windows → Общие учётные данные.
//!
//! Правило модуля: секреты никогда не попадают в лог и в тексты ошибок.

use std::fmt;

use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::riot::ApiKey;

const SERVICE: &str = "lol-account-manager";
const API_KEY_USER: &str = "riot-api-key";

/// Логин и пароль аккаунта.
///
/// `Debug` реализован вручную и печатает `***` вместо значений,
/// поэтому `tracing::debug!("{creds:?}")` по ошибке ничего не раскроет.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl Credentials {
    /// Пустые строки превращаются в None. Логин обрезаем по краям, пароль — НЕТ:
    /// пробелы в начале или в конце могут быть частью пароля.
    pub fn new(login: Option<String>, password: Option<String>) -> Self {
        Self {
            login: login
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty()),
            password: password.filter(|p| !p.is_empty()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.login.is_none() && self.password.is_none()
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mask = |v: &Option<String>| v.as_ref().map(|_| "***");
        f.debug_struct("Credentials")
            .field("login", &mask(&self.login))
            .field("password", &mask(&self.password))
            .finish()
    }
}

/// Доступ к системному хранилищу паролей.
#[derive(Debug, Clone)]
pub struct Secrets {
    service: String,
}

impl Default for Secrets {
    fn default() -> Self {
        Self::new()
    }
}

impl Secrets {
    pub fn new() -> Self {
        Self::with_service(SERVICE)
    }

    /// Другое имя сервиса — для тестов, чтобы не трогать настоящие записи.
    pub fn with_service(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }

    fn entry(&self, user: &str) -> AppResult<Entry> {
        Ok(Entry::new(&self.service, user)?)
    }

    // ----- API-ключ ---------------------------------------------------------

    /// Сохранённый ключ или None, если его ещё не вводили.
    pub fn api_key(&self) -> AppResult<Option<ApiKey>> {
        match self.entry(API_KEY_USER)?.get_password() {
            Ok(raw) => Ok(Some(ApiKey::new(&raw)?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_api_key(&self, raw: &str) -> AppResult<()> {
        ApiKey::new(raw)?; // проверка формата: не пустой, только ASCII
        self.entry(API_KEY_USER)?.set_password(raw.trim())?;
        Ok(())
    }

    pub fn delete_api_key(&self) -> AppResult<()> {
        delete_if_exists(&self.entry(API_KEY_USER)?)
    }

    // ----- Логин и пароль аккаунта ------------------------------------------

    pub fn credentials(&self, secret_ref: &str) -> AppResult<Credentials> {
        match self.entry(&account_user(secret_ref))?.get_password() {
            // Текст ошибки serde НЕ показываем: он может процитировать кусок секрета.
            Ok(json) => serde_json::from_str(&json)
                .map_err(|_| AppError::SecretStore("запись повреждена".into())),
            Err(keyring::Error::NoEntry) => Ok(Credentials::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Сохраняет логин и пароль. Если оба пустые — удаляет запись целиком.
    pub fn set_credentials(&self, secret_ref: &str, creds: &Credentials) -> AppResult<()> {
        if creds.is_empty() {
            return self.delete_credentials(secret_ref);
        }
        let json = serde_json::to_string(creds)
            .map_err(|_| AppError::SecretStore("не удалось подготовить запись".into()))?;
        self.entry(&account_user(secret_ref))?.set_password(&json)?;
        Ok(())
    }

    pub fn delete_credentials(&self, secret_ref: &str) -> AppResult<()> {
        delete_if_exists(&self.entry(&account_user(secret_ref))?)
    }
}

fn account_user(secret_ref: &str) -> String {
    format!("account-{secret_ref}")
}

/// Удаление записи; «записи и так нет» — не ошибка.
fn delete_if_exists(entry: &Entry) -> AppResult<()> {
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_debug_is_masked() {
        let c = Credentials::new(Some("my-login".into()), Some("my-password".into()));
        let text = format!("{c:?}");
        assert!(!text.contains("my-login") && !text.contains("my-password"), "{text}");
    }

    #[test]
    fn credentials_normalization() {
        let c = Credentials::new(Some("  login  ".into()), Some("  pass with spaces ".into()));
        assert_eq!(c.login.as_deref(), Some("login"));
        assert_eq!(c.password.as_deref(), Some("  pass with spaces "));
        assert!(Credentials::new(Some("   ".into()), Some(String::new())).is_empty());
    }

    /// Работает с НАСТОЯЩИМ хранилищем ОС (отдельное имя сервиса, за собой убирает).
    /// Запуск: `cargo test -- --ignored`.
    #[test]
    #[ignore = "использует системное хранилище паролей"]
    fn os_keyring_roundtrip() {
        let secrets = Secrets::with_service(&format!("lol-account-manager-test-{}", uuid::Uuid::new_v4()));
        let r = uuid::Uuid::new_v4().to_string();

        assert_eq!(secrets.credentials(&r).unwrap(), Credentials::default());
        let creds = Credentials::new(Some("логин".into()), Some("P@ss пароль 🔑".into()));
        secrets.set_credentials(&r, &creds).unwrap();
        assert_eq!(secrets.credentials(&r).unwrap(), creds);

        secrets.set_credentials(&r, &Credentials::default()).unwrap(); // пустые = удалить
        assert_eq!(secrets.credentials(&r).unwrap(), Credentials::default());
        secrets.delete_credentials(&r).unwrap(); // повторное удаление — не ошибка

        assert!(secrets.api_key().unwrap().is_none());
        secrets.set_api_key("RGAPI-test-key").unwrap();
        assert!(secrets.api_key().unwrap().is_some());
        secrets.delete_api_key().unwrap();
        assert!(secrets.api_key().unwrap().is_none());
    }
}
