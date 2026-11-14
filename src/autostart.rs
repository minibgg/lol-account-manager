//! Запуск вместе с Windows.
//!
//! Windows при входе пользователя запускает всё, что перечислено в реестре в ключе
//! `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`. Мы добавляем туда
//! одну строку — путь к программе с параметром `--autostart` — или убираем её.
//! Ключ принадлежит текущему пользователю, права администратора не нужны. Эту же запись
//! видно в «Диспетчере задач → Автозагрузка», там её тоже можно выключить.

use crate::error::AppResult;

/// С этим параметром программу запускает Windows (а не пользователь ярлыком):
/// если включён трей, окно не показывается, программа сразу сидит в трее.
pub const AUTOSTART_ARG: &str = "--autostart";

/// Есть ли на этой системе автозапуск (у нас — только Windows).
pub fn is_supported() -> bool {
    cfg!(windows)
}

/// Запущена ли программа самой Windows при входе в систему.
pub fn started_by_system() -> bool {
    std::env::args().any(|arg| arg == AUTOSTART_ARG)
}

// `#[cfg(windows)]` — этот модуль компилируется только под Windows; для остальных
// систем ниже лежит «пустая» версия с теми же функциями.
#[cfg(windows)]
mod imp {
    use std::path::Path;

    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    use super::AUTOSTART_ARG;
    use crate::error::{AppError, AppResult};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "LoL Account Manager";

    /// Строка для реестра: путь в кавычках (в нём бывают пробелы) и параметр.
    pub fn command_line(exe: &Path) -> String {
        format!("\"{}\" {AUTOSTART_ARG}", exe.display())
    }

    fn registry_error(e: std::io::Error) -> AppError {
        AppError::Io(format!("реестр Windows: {e}"))
    }

    /// Включён ли автозапуск именно этой копии программы.
    pub fn is_enabled() -> bool {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .and_then(|key| key.get_value::<String, _>(VALUE_NAME))
            // В Windows регистр букв в путях не важен.
            .is_ok_and(|saved| saved.to_lowercase() == command_line(&exe).to_lowercase())
    }

    pub fn set_enabled(enabled: bool) -> AppResult<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(RUN_KEY)
            .map_err(registry_error)?;
        if enabled {
            let exe = std::env::current_exe()?;
            key.set_value(VALUE_NAME, &command_line(&exe)).map_err(registry_error)
        } else {
            match key.delete_value(VALUE_NAME) {
                // Записи и так нет — это то, чего мы хотели.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other.map_err(registry_error),
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use crate::error::{AppError, AppResult};

    pub fn is_enabled() -> bool {
        false
    }

    pub fn set_enabled(_enabled: bool) -> AppResult<()> {
        Err(AppError::Internal("автозапуск есть только в Windows".into()))
    }
}

/// Включён ли автозапуск.
pub fn is_enabled() -> bool {
    imp::is_enabled()
}

/// Добавить программу в автозапуск или убрать из него.
pub fn set_enabled(enabled: bool) -> AppResult<()> {
    imp::set_enabled(enabled)
}

#[cfg(all(test, windows))]
mod tests {
    use std::path::Path;

    #[test]
    fn command_line_quotes_the_path() {
        let line = super::imp::command_line(Path::new(r"C:\Users\Иван Середа\AppData\Local\Programs\LoL Account Manager\lol-account-manager.exe"));
        assert_eq!(
            line,
            r#""C:\Users\Иван Середа\AppData\Local\Programs\LoL Account Manager\lol-account-manager.exe" --autostart"#
        );
    }
}
