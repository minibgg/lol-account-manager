//! Полная резервная копия: список аккаунтов **вместе с логинами и паролями**, зашифрованная
//! паролем, который придумывает пользователь. Нужна, чтобы переехать на другой компьютер
//! целиком. Обычный экспорт (transfer.rs) логины и пароли не сохраняет.
//!
//! Как устроена защита:
//! - из пароля копии получаем ключ шифрования алгоритмом **Argon2id** — он намеренно
//!   медленный и требует много памяти (64 МБ), поэтому подбирать пароль перебором дорого;
//!   «соль» (случайные байты, свои у каждого файла) не даёт заранее заготовить таблицу;
//! - сами данные шифруются **XChaCha20-Poly1305**: это шифр «с проверкой подлинности» —
//!   неверный пароль или изменённый файл не расшифруются вовсе, а не превратятся в мусор;
//! - расшифрованные данные и ключ затираются в памяти, как только больше не нужны.
//!
//! Без пароля копии файл бесполезен. Забытый пароль восстановить нельзя.
//!
//! Файл — JSON: параметры Argon2, соль, nonce (одноразовое число шифра) и зашифрованные
//! данные в base64. Только чистые функции (без диска) — их легко проверить тестами.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20poly1305::aead::{Aead, Generate, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};
use crate::riot::{Region, RiotId};
use crate::storage::{Account, AccountInput, Credentials};

/// Метка формата: по ней узнаём «свой» файл. Она же подмешивается в шифр как
/// «связанные данные»: подменить её в файле нельзя — расшифровка не пройдёт.
const FORMAT: &str = "lol-account-manager/vault";
const VERSION: u32 = 1;
/// Расширение файла копии.
pub const EXTENSION: &str = "lolvault";
/// Пароль копии короче этого не принимаем.
pub const MIN_PASSWORD_CHARS: usize = 8;

/// Параметры Argon2id для новых файлов: 64 МБ памяти, 3 прохода, 1 поток.
const MEMORY_KIB: u32 = 64 * 1024;
const PASSES: u32 = 3;
const LANES: u32 = 1;
/// Верхняя граница параметров из файла: чужой файл не должен заставить нас занять
/// гигабайты памяти или считать часами.
const MAX_MEMORY_KIB: u32 = 1024 * 1024;
const MAX_PASSES: u32 = 20;
const SALT_LEN: usize = 16;

#[derive(Serialize, Deserialize)]
struct VaultFile {
    format: String,
    version: u32,
    #[serde(default)]
    exported_at: i64,
    /// Память Argon2id в килобайтах, число проходов и потоков.
    memory_kib: u32,
    passes: u32,
    lanes: u32,
    salt: String,
    nonce: String,
    /// Зашифрованный JSON со списком [`VaultAccount`].
    data: String,
}

/// Запись аккаунта внутри зашифрованной части.
#[derive(Serialize, Deserialize)]
struct VaultAccount {
    riot_id: String,
    region: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    login: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

// Логин и пароль записи затираются в памяти вместе с ней.
impl Drop for VaultAccount {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.login.zeroize();
        self.password.zeroize();
    }
}

/// Аккаунт из копии, готовый к добавлению. `Debug` секретов не печатает
/// (см. `Credentials`).
#[derive(Debug)]
pub struct VaultEntry {
    pub input: AccountInput,
    pub credentials: Credentials,
}

/// Что получилось прочитать из копии.
#[derive(Debug)]
pub struct VaultPlan {
    pub entries: Vec<VaultEntry>,
    /// Записи с ошибкой (неверный Riot ID или регион) — пропущены.
    pub invalid: usize,
}

/// Подходит ли пароль для копии.
pub fn check_password(password: &str) -> AppResult<()> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(AppError::VaultWeakPassword(MIN_PASSWORD_CHARS));
    }
    Ok(())
}

/// Ключ шифрования из пароля (медленно — так задумано).
fn derive_key(password: &str, salt: &[u8], memory_kib: u32, passes: u32, lanes: u32) -> AppResult<Zeroizing<[u8; 32]>> {
    let params = Params::new(memory_kib, passes, lanes, Some(32)).map_err(|_| AppError::NotAVaultFile)?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|e| AppError::Internal(format!("Argon2: {e}")))?;
    Ok(key)
}

/// Аккаунты с логинами и паролями → текст файла копии.
pub fn seal(accounts: &[(Account, Credentials)], password: &str, exported_at: i64) -> AppResult<String> {
    check_password(password)?;
    seal_with(accounts, password, exported_at, MEMORY_KIB, PASSES)
}

/// То же с заданной «тяжестью» Argon2 (в тестах — лёгкой, чтобы они шли быстро).
fn seal_with(
    accounts: &[(Account, Credentials)],
    password: &str,
    exported_at: i64,
    memory_kib: u32,
    passes: u32,
) -> AppResult<String> {
    let list: Vec<VaultAccount> = accounts
        .iter()
        .map(|(account, credentials)| VaultAccount {
            riot_id: account.riot_id.to_string(),
            region: account.region.label().to_string(),
            tags: account.tags.clone(),
            note: account.note.clone(),
            login: credentials.login.clone(),
            password: credentials.password.clone(),
        })
        .collect();
    // Zeroizing — открытый текст с паролями затрётся в памяти при выходе из функции.
    let plain = Zeroizing::new(serde_json::to_vec(&list).map_err(|_| AppError::Internal("копия не собрана".into()))?);
    let salt = <[u8; SALT_LEN]>::generate();
    let nonce = XNonce::generate();
    let key = derive_key(password, &salt, memory_kib, passes, LANES)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|_| AppError::Internal("ключ шифра".into()))?;
    let data = cipher
        .encrypt(&nonce, Payload { msg: plain.as_slice(), aad: FORMAT.as_bytes() })
        .map_err(|_| AppError::Internal("шифрование не удалось".into()))?;
    let file = VaultFile {
        format: FORMAT.to_string(),
        version: VERSION,
        exported_at,
        memory_kib,
        passes,
        lanes: LANES,
        salt: BASE64.encode(salt),
        nonce: BASE64.encode(nonce),
        data: BASE64.encode(data),
    };
    serde_json::to_string_pretty(&file).map_err(|_| AppError::Internal("копия не записана".into()))
}

/// Текст файла копии + пароль → аккаунты. Неверный пароль и повреждённый файл
/// неразличимы (так устроен шифр) — ошибка одна: [`AppError::VaultWrongPassword`].
pub fn open(bytes: &[u8], password: &str) -> AppResult<VaultPlan> {
    let file: VaultFile = serde_json::from_slice(bytes).map_err(|_| AppError::NotAVaultFile)?;
    if file.format != FORMAT {
        return Err(AppError::NotAVaultFile);
    }
    if file.version > VERSION {
        return Err(AppError::ExportTooNew(file.version));
    }
    if file.memory_kib > MAX_MEMORY_KIB || file.passes > MAX_PASSES || file.lanes != LANES {
        return Err(AppError::NotAVaultFile);
    }
    let decode = |text: &str| BASE64.decode(text).map_err(|_| AppError::NotAVaultFile);
    let (salt, nonce, data) = (decode(&file.salt)?, decode(&file.nonce)?, decode(&file.data)?);
    let nonce = XNonce::try_from(nonce.as_slice()).map_err(|_| AppError::NotAVaultFile)?;
    let key = derive_key(password, &salt, file.memory_kib, file.passes, file.lanes)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|_| AppError::Internal("ключ шифра".into()))?;
    let plain = Zeroizing::new(
        cipher
            .decrypt(&nonce, Payload { msg: data.as_slice(), aad: FORMAT.as_bytes() })
            .map_err(|_| AppError::VaultWrongPassword)?,
    );
    // Текст ошибки serde не показываем: он может процитировать кусок секрета.
    let list: Vec<VaultAccount> = serde_json::from_slice(&plain).map_err(|_| AppError::NotAVaultFile)?;

    let mut entries = Vec::new();
    let mut invalid = 0;
    for mut entry in list {
        let riot_id = RiotId::parse(&entry.riot_id).ok();
        let region = entry.region.parse::<Region>().ok();
        match (riot_id, region) {
            (Some(riot_id), Some(region)) => entries.push(VaultEntry {
                input: AccountInput {
                    riot_id,
                    region,
                    note: entry.note.trim().to_string(),
                    tags: std::mem::take(&mut entry.tags),
                },
                // take() — забрать из записи (у неё есть Drop, поля просто так не вынуть).
                credentials: Credentials::new(entry.login.take(), entry.password.take()),
            }),
            _ => invalid += 1,
        }
    }
    Ok(VaultPlan { entries, invalid })
}

/// Имя файла по умолчанию: «lol-accounts-2026-11-07.lolvault».
pub fn default_file_name(unix_secs: i64) -> String {
    crate::transfer::default_file_name(unix_secs).replace(".json", &format!(".{EXTENSION}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(name: &str, region: Region) -> Account {
        Account {
            id: 1,
            riot_id: RiotId::parse(name).unwrap(),
            region,
            puuid: Some("secret-puuid".into()),
            lcu_puuid: None,
            note: "заметка".into(),
            tags: vec!["main".into()],
            secret_ref: "keyring-ref".into(),
            has_login: true,
            has_password: true,
            created_at: 0,
            cache: None,
            history: vec![],
            archived_games: 0,
            ranked_games: vec![],
        }
    }

    /// Лёгкие параметры Argon2 — тест идёт миллисекунды, а не секунды.
    fn sealed(password: &str) -> String {
        let accounts = [
            (account("Name#EUW", Region::Euw), Credentials::new(Some("my-login".into()), Some(" p@ss word ".into()))),
            (account("Второй#RU1", Region::Ru), Credentials::default()),
        ];
        seal_with(&accounts, password, 123, 64, 1).unwrap()
    }

    #[test]
    fn round_trip_restores_accounts_and_credentials() {
        let text = sealed("правильный пароль");
        let plan = open(text.as_bytes(), "правильный пароль").unwrap();
        assert_eq!((plan.entries.len(), plan.invalid), (2, 0));
        let first = &plan.entries[0];
        assert_eq!(first.input.riot_id.to_string(), "Name#EUW");
        assert_eq!((first.input.region, first.input.note.as_str()), (Region::Euw, "заметка"));
        assert_eq!(first.input.tags, ["main"]);
        assert_eq!(first.credentials.login.as_deref(), Some("my-login"));
        assert_eq!(first.credentials.password.as_deref(), Some(" p@ss word "), "пробелы пароля сохранились");
        assert!(plan.entries[1].credentials.is_empty());
    }

    #[test]
    fn file_does_not_contain_secrets_in_the_clear() {
        let text = sealed("правильный пароль");
        for secret in ["my-login", "p@ss", "Name#EUW", "заметка", "secret-puuid", "keyring-ref", "правильный"] {
            assert!(!text.contains(secret), "{secret} виден в файле");
        }
        // Два файла с одним паролем — разные (своя соль и nonce у каждого).
        assert_ne!(text, sealed("правильный пароль"));
    }

    #[test]
    fn wrong_password_and_tampering_are_rejected() {
        let text = sealed("правильный пароль");
        assert_eq!(open(text.as_bytes(), "другой пароль").unwrap_err(), AppError::VaultWrongPassword);
        // Меняем один символ зашифрованных данных — расшифровка не проходит.
        let mut file: VaultFile = serde_json::from_str(&text).unwrap();
        let mut data = BASE64.decode(&file.data).unwrap();
        data[0] ^= 1;
        file.data = BASE64.encode(data);
        let tampered = serde_json::to_string(&file).unwrap();
        assert_eq!(open(tampered.as_bytes(), "правильный пароль").unwrap_err(), AppError::VaultWrongPassword);
    }

    #[test]
    fn foreign_and_hostile_files_are_rejected() {
        assert_eq!(open(b"not json", "password").unwrap_err(), AppError::NotAVaultFile);
        let list = crate::transfer::export_json(&[account("Name#EUW", Region::Euw)], 0);
        assert_eq!(open(list.as_bytes(), "password").unwrap_err(), AppError::NotAVaultFile, "обычный экспорт — не копия");
        // Файл просит 8 ГБ памяти под Argon2 — даже не пробуем.
        let mut file: VaultFile = serde_json::from_str(&sealed("правильный пароль")).unwrap();
        file.memory_kib = 8 * 1024 * 1024;
        let hostile = serde_json::to_string(&file).unwrap();
        assert_eq!(open(hostile.as_bytes(), "правильный пароль").unwrap_err(), AppError::NotAVaultFile);
        file.version = VERSION + 1;
        let newer = serde_json::to_string(&file).unwrap();
        assert_eq!(open(newer.as_bytes(), "x").unwrap_err(), AppError::ExportTooNew(VERSION + 1));
    }

    #[test]
    fn short_passwords_are_refused() {
        assert_eq!(seal(&[], "1234567", 0).unwrap_err(), AppError::VaultWeakPassword(MIN_PASSWORD_CHARS));
        assert!(check_password("пароль12").is_ok(), "считаем символы, а не байты");
        assert_eq!(default_file_name(0), "lol-accounts-1970-01-01.lolvault");
    }
}
