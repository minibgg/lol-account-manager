//! Как найти запущенный клиент League.
//!
//! Пока клиент работает, в папке игры лежит файл `lockfile` вида
//!
//! ```text
//! LeagueClient:<pid>:<порт>:<пароль>:https
//! ```
//!
//! Порт — где слушает локальный HTTPS-сервер, пароль — для Basic-авторизации
//! (имя пользователя всегда `riot`). Пароль новый при каждом запуске клиента.
//! Мы его нигде не храним и не пишем в лог; файл перечитывается перед каждым запросом,
//! поэтому перезапуск клиента подхватывается сам.

use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

/// Переменная окружения с папкой игры — если она не находится автоматически
/// (нестандартная установка, Wine на Linux).
pub const DIR_ENV: &str = "LOLAM_LEAGUE_DIR";

/// Файл, в котором установщик Riot записывает путь к игре (только Windows).
const PRODUCT_SETTINGS: &str = r"C:\ProgramData\Riot Games\Metadata\league_of_legends.live\league_of_legends.live.product_settings.yaml";

/// Стандартная папка установки на Windows.
const DEFAULT_WINDOWS_DIR: &str = r"C:\Riot Games\League of Legends";

/// Содержимое lockfile.
pub struct Lockfile {
    pub port: u16,
    password: String,
}

impl Lockfile {
    pub fn parse(text: &str) -> AppResult<Self> {
        // Ровно 5 частей через ':'. Сопоставление со срезом: если частей другое
        // количество, сработает ветка `_`.
        let parts: Vec<&str> = text.trim().split(':').collect();
        let [_name, _pid, port, password, _protocol] = parts[..] else {
            return Err(AppError::LcuNotReady);
        };
        let port = port.parse().map_err(|_| AppError::LcuNotReady)?;
        if password.is_empty() {
            return Err(AppError::LcuNotReady);
        }
        Ok(Self {
            port,
            password: password.to_string(),
        })
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    /// Ищет lockfile в папках по очереди. `Ok(None)` — клиент не запущен.
    pub async fn find(dirs: &[PathBuf]) -> AppResult<Option<Self>> {
        for dir in dirs {
            // tokio::fs — асинхронная версия std::fs: чтение файла не блокирует задачу.
            match tokio::fs::read_to_string(dir.join("lockfile")).await {
                Ok(text) => return Self::parse(&text).map(Some),
                Err(e) if e.kind() == ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(None)
    }
}

impl fmt::Debug for Lockfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lockfile")
            .field("port", &self.port)
            .field("password", &"***")
            .finish()
    }
}

/// Папки, где искать lockfile, в порядке приоритета:
/// 1. `custom` — папка из настроек приложения;
/// 2. переменная окружения `LOLAM_LEAGUE_DIR`;
/// 3. путь из файла настроек установщика Riot (Windows);
/// 4. `C:\Riot Games\League of Legends` (Windows).
///
/// Блокирующая функция (читает файл): вызывать через `spawn_blocking`.
pub fn candidate_dirs(custom: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = custom {
        dirs.push(dir.to_path_buf());
    }
    if let Some(dir) = std::env::var_os(DIR_ENV) {
        dirs.push(PathBuf::from(dir));
    }
    if cfg!(windows) {
        if let Some(dir) = std::fs::read_to_string(PRODUCT_SETTINGS)
            .ok()
            .and_then(|yaml| install_path_from_yaml(&yaml))
        {
            dirs.push(dir);
        }
        dirs.push(PathBuf::from(DEFAULT_WINDOWS_DIR));
    }
    dirs.dedup();
    dirs
}

/// Исполняемый файл клиента — по нему узнаём папку с игрой.
const CLIENT_EXE: &str = "LeagueClient.exe";

/// В этой папке установлена League of Legends? (Блокирующая: проверяет диск.)
pub fn is_install_dir(dir: &Path) -> bool {
    dir.join(CLIENT_EXE).is_file()
}

/// Первая из папок, где установлена игра, — чтобы показать её в настройках.
/// Блокирующая функция.
pub fn find_install_dir(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().find(|dir| is_install_dir(dir)).cloned()
}

/// Достаёт `product_install_full_path: "C:/Riot Games/League of Legends"` из YAML.
/// Ради одной строки не тянем YAML-библиотеку: ищем строку с нужным ключом.
fn install_path_from_yaml(yaml: &str) -> Option<PathBuf> {
    yaml.lines().find_map(|line| {
        let value = line.trim().strip_prefix("product_install_full_path:")?;
        let value = value.trim().trim_matches('"');
        (!value.is_empty()).then(|| PathBuf::from(value))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lockfile() {
        let lock = Lockfile::parse("LeagueClient:13676:52113:s3cr3t-Pa55:https\n").unwrap();
        assert_eq!(lock.port, 52113);
        assert_eq!(lock.password(), "s3cr3t-Pa55");
        assert!(!format!("{lock:?}").contains("s3cr3t"), "пароль не должен печататься");
        assert!(Lockfile::parse("LeagueClient:1:notaport:pw:https").is_err());
        assert!(Lockfile::parse("garbage").is_err());
    }

    #[test]
    fn reads_install_path_from_yaml() {
        let yaml = "product_install_full_path: \"C:/Riot Games/League of Legends\"\n\
                    product_install_root: \"C:/Riot Games\"\n";
        assert_eq!(
            install_path_from_yaml(yaml),
            Some(PathBuf::from("C:/Riot Games/League of Legends"))
        );
        assert_eq!(install_path_from_yaml("other: 1"), None);
    }
}
