//! Запуск Riot Client для кнопки «Войти».
//!
//! Делает ровно то же, что ярлык «League of Legends» на рабочем столе:
//! `RiotClientServices.exe --launch-product=league_of_legends --launch-patchline=live`.
//! Если Riot Client уже запущен, он просто выходит на передний план.
//!
//! **Только запуск.** Логин и пароль программа в клиент не вводит и ничего в нём
//! не нажимает (правило проекта и правила Riot): пользователь вставляет их сам (Ctrl+V).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tracing::info;

use crate::error::{AppError, AppResult};

/// Аргументы официального ярлыка League of Legends.
pub const LAUNCH_ARGS: [&str; 2] = ["--launch-product=league_of_legends", "--launch-patchline=live"];

/// Здесь установщик Riot записывает, где лежит Riot Client.
const INSTALLS_JSON: &str = r"C:\ProgramData\Riot Games\RiotClientInstalls.json";

/// Стандартное место установки.
const DEFAULT_EXE: &str = r"C:\Riot Games\Riot Client\RiotClientServices.exe";

const EXE_NAME: &str = "RiotClientServices.exe";

/// Запустить Riot Client. `Ok(false)` — не Windows: Riot Client там нет, ничего не делаем.
/// Блокирующая функция (читает файлы): вызывать через `spawn_blocking`.
pub fn launch_riot_client() -> AppResult<bool> {
    if !cfg!(windows) {
        return Ok(false);
    }
    let exe = find_exe().ok_or(AppError::RiotClientNotFound)?;
    // spawn() запускает процесс и сразу возвращается — не ждём, пока клиент закроется.
    // Ввод и вывод отключаем: окну консоли (при `cargo run`) клиент ничего не пишет.
    std::process::Command::new(&exe)
        .args(LAUNCH_ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::RiotClientLaunch(e.to_string()))?;
    info!(exe = %exe.display(), "Riot Client запущен");
    Ok(true)
}

fn find_exe() -> Option<PathBuf> {
    let from_json = std::fs::read_to_string(INSTALLS_JSON)
        .map(|json| exe_candidates(&json))
        .unwrap_or_default();
    from_json
        .into_iter()
        .chain([PathBuf::from(DEFAULT_EXE)])
        .find(|path| is_riot_client(path))
}

/// Пути из RiotClientInstalls.json: `rc_default`, затем `rc_live`.
fn exe_candidates(json: &str) -> Vec<PathBuf> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    ["rc_default", "rc_live"]
        .iter()
        .filter_map(|key| value.get(key)?.as_str())
        .map(PathBuf::from)
        .collect()
}

/// Запускаем только файл с именем RiotClientServices.exe: путь берётся из чужого файла,
/// и подменённое значение не должно заставить нас запустить что-то другое.
fn is_riot_client(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(EXE_NAME))
        && path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_paths_from_installs_json() {
        let json = r#"{
            "associated_client": { "C:/Riot Games/League of Legends/": "C:/Riot Games/Riot Client/RiotClientServices.exe" },
            "rc_default": "C:/Riot Games/Riot Client/RiotClientServices.exe",
            "rc_live": "D:/Games/Riot Client/RiotClientServices.exe"
        }"#;
        assert_eq!(
            exe_candidates(json),
            [
                PathBuf::from("C:/Riot Games/Riot Client/RiotClientServices.exe"),
                PathBuf::from("D:/Games/Riot Client/RiotClientServices.exe"),
            ]
        );
        assert!(exe_candidates("не json").is_empty());
    }

    #[test]
    fn only_riot_client_exe_is_accepted() {
        // Чужое имя не подходит, даже если файл существует.
        let other = std::env::current_exe().unwrap();
        assert!(!is_riot_client(&other));
        assert!(!is_riot_client(Path::new(r"C:\nowhere\RiotClientServices.exe")));
    }
}
