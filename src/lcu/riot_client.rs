//! Доступ к локальному API Riot Client — для имён игроков в лобби.
//!
//! У Riot Client свой локальный HTTPS-сервер (как у клиента League). Порт и токен
//! для него клиент League получает при запуске в аргументах командной строки:
//!
//! ```text
//! LeagueClientUx.exe ... --riotclient-app-port=56002 --riotclient-auth-token=XXXX ...
//! ```
//!
//! Токен из lockfile самого Riot Client не подходит: с ним чат отвечает
//! «not connected to chat». Поэтому читаем аргументы процесса LeagueClientUx.exe
//! (как и reveal). Токен нигде не сохраняется и не пишется в лог.

use std::ffi::OsStr;
use std::fmt;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::error::{AppError, AppResult};

const UX_PROCESS: &str = "LeagueClientUx.exe";
const PORT_ARG: &str = "--riotclient-app-port=";
const TOKEN_ARG: &str = "--riotclient-auth-token=";

/// Порт и токен API Riot Client.
pub struct RiotClientAuth {
    pub port: u16,
    token: String,
}

impl RiotClientAuth {
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Debug for RiotClientAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RiotClientAuth")
            .field("port", &self.port)
            .field("token", &"***")
            .finish()
    }
}

/// Найти процесс LeagueClientUx.exe и достать из его аргументов порт и токен.
/// Блокирующая функция (опрашивает систему): вызывать через `spawn_blocking`.
pub fn read_auth() -> AppResult<RiotClientAuth> {
    let mut system = System::new().map_err(|_| AppError::LeagueArgsUnreadable)?;
    // Сначала только список процессов (имена) — это быстро.
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let pid = system
        .processes_by_exact_name(OsStr::new(UX_PROCESS))
        .next()
        .map(|p| p.pid())
        .ok_or(AppError::LcuNotRunning)?;

    // Командную строку читаем только у этого процесса.
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let process = system.process(pid).ok_or(AppError::LcuNotRunning)?;
    // Пустая командная строка — Windows не дала её прочитать (например, клиент League
    // запущен от имени администратора, а менеджер — нет).
    parse_args(process.cmd().iter().map(|arg| arg.to_string_lossy())).ok_or(AppError::LeagueArgsUnreadable)
}

/// `--riotclient-app-port=…` и `--riotclient-auth-token=…` из списка аргументов.
/// `impl AsRef<str>` — подойдут и `&str`, и `String`, и `Cow<str>`.
fn parse_args(args: impl Iterator<Item = impl AsRef<str>>) -> Option<RiotClientAuth> {
    let (mut port, mut token) = (None, None);
    for arg in args {
        let arg = arg.as_ref().trim_matches('"');
        if let Some(value) = arg.strip_prefix(PORT_ARG) {
            port = value.parse().ok();
        } else if let Some(value) = arg.strip_prefix(TOKEN_ARG) {
            token = Some(value.to_string()).filter(|t| !t.is_empty());
        }
    }
    Some(RiotClientAuth {
        port: port?,
        token: token?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_port_and_token_from_args() {
        let args = [
            r"C:/Riot Games/League of Legends/LeagueClientUx.exe",
            "--riotclient-auth-token=s3cr3t-T0ken",
            "--riotclient-app-port=56002",
            "--app-port=53197",
            "--remoting-auth-token=other",
        ];
        let auth = parse_args(args.iter()).unwrap();
        assert_eq!(auth.port, 56002);
        assert_eq!(auth.token(), "s3cr3t-T0ken");
        assert!(!format!("{auth:?}").contains("s3cr3t"), "токен не печатается");

        assert!(parse_args(["--app-port=1"].iter()).is_none(), "нет нужных аргументов");
        assert!(parse_args(["--riotclient-app-port=x", "--riotclient-auth-token=t"].iter()).is_none());
    }

    /// Настоящий LeagueClientUx.exe (нужен запущенный клиент League).
    #[test]
    #[ignore = "нужен запущенный клиент League"]
    fn reads_auth_from_running_client() {
        let auth = read_auth().unwrap();
        assert!(auth.port > 0 && !auth.token().is_empty());
    }
}
