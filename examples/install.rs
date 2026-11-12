//! Установка LoL Account Manager как обычной программы (Windows):
//!
//! 1. собирает оптимизированную версию (`cargo build --release`);
//! 2. копирует её в `%LOCALAPPDATA%\Programs\LoL Account Manager`;
//! 3. создаёт ярлыки на рабочем столе и в меню «Пуск».
//!
//! Запуск из папки проекта:
//!   cargo run --example install
//! Обновить установленную версию — та же команда (можно даже при открытом приложении:
//! новая версия запустится со следующего открытия).
//!
//! Данные не трогаются: база лежит в %APPDATA%\lol-account-manager, логины и пароли —
//! в диспетчере учётных данных Windows. Удаление: удалить папку программы и два ярлыка.

use std::path::{Path, PathBuf};
use std::process::Command;

const APP_NAME: &str = "LoL Account Manager";
const EXE_NAME: &str = "lol-account-manager.exe";

fn main() {
    if let Err(e) = run() {
        eprintln!("✗ {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    if !cfg!(windows) {
        return Err(
            "установка с ярлыками сделана для Windows. На Linux: cargo build --release, \
             программа — target/release/lol-account-manager"
                .into(),
        );
    }
    let project = Path::new(env!("CARGO_MANIFEST_DIR"));

    // 1. Сборка. `cargo run` передаёт программе путь к cargo в переменной CARGO.
    println!("Собираю оптимизированную версию (в первый раз — несколько минут)...");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--release", "--bin", "lol-account-manager"])
        .current_dir(project)
        .status()
        .map_err(|e| format!("не удалось запустить cargo: {e}"))?;
    if !status.success() {
        return Err("сборка не удалась — ошибки выше".into());
    }
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| project.join("target"));
    let built = target_dir.join("release").join(EXE_NAME);

    // 2. Копия в папку программ пользователя (права администратора не нужны).
    let install_dir = dirs::data_local_dir()
        .ok_or("не найдена папка %LOCALAPPDATA%")?
        .join("Programs")
        .join(APP_NAME);
    std::fs::create_dir_all(&install_dir).map_err(|e| format!("{}: {e}", install_dir.display()))?;
    let installed = install_dir.join(EXE_NAME);
    copy_replacing(&built, &installed)?;
    println!("✓ Программа: {}", installed.display());

    // 3. Ярлыки.
    let desktop = dirs::desktop_dir().ok_or("не найдена папка рабочего стола")?;
    let start_menu = dirs::data_dir()
        .ok_or("не найдена папка %APPDATA%")?
        .join(r"Microsoft\Windows\Start Menu\Programs");
    for folder in [desktop, start_menu] {
        let link = folder.join(format!("{APP_NAME}.lnk"));
        create_shortcut(&link, &installed, &install_dir)?;
        println!("✓ Ярлык: {}", link.display());
    }
    println!("\nГотово: откройте «{APP_NAME}» с рабочего стола или из меню «Пуск».");
    Ok(())
}

/// Скопировать новую версию поверх старой. Если старая сейчас открыта, Windows не даёт
/// её перезаписать, но даёт переименовать: открытая копия работает дальше,
/// а новая запустится со следующего открытия.
fn copy_replacing(from: &Path, to: &Path) -> Result<(), String> {
    let old = to.with_extension("old.exe");
    let _ = std::fs::remove_file(&old); // осталась от прошлого обновления — уже не нужна

    if std::fs::copy(from, to).is_ok() {
        return Ok(());
    }
    if to.exists() {
        std::fs::rename(to, &old).map_err(|e| {
            format!("не удалось заменить {}: {e}. Закройте приложение и повторите", to.display())
        })?;
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("не удалось скопировать {} → {}: {e}", from.display(), to.display()))
}

/// Ярлык создаёт сама Windows (объект WScript.Shell) — так он гарантированно правильный,
/// в том числе с русскими буквами в пути. Скрипт передаём в PowerShell через
/// -EncodedCommand (base64 от UTF-16): так не нужно экранировать кавычки и кириллицу
/// для командной строки.
fn create_shortcut(link: &Path, target: &Path, working_dir: &Path) -> Result<(), String> {
    // В строках PowerShell в одинарных кавычках единственный спецсимвол — сама кавычка.
    let quote = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
    let script = format!(
        "$ErrorActionPreference = 'Stop'\n\
         $ProgressPreference = 'SilentlyContinue'\n\
         $link = (New-Object -ComObject WScript.Shell).CreateShortcut({link})\n\
         $link.TargetPath = {target}\n\
         $link.WorkingDirectory = {dir}\n\
         $link.IconLocation = {target} + ',0'\n\
         $link.Description = 'Менеджер аккаунтов League of Legends'\n\
         $link.Save()",
        link = quote(link),
        target = quote(target),
        dir = quote(working_dir),
    );
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    // output() — дождаться и забрать вывод: показываем его, только если что-то пошло не так.
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &base64(&utf16)])
        .output()
        .map_err(|e| format!("не удалось запустить PowerShell: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "не удалось создать ярлык {}: {}",
            link.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Base64 (RFC 4648) — ради одной строки не подключаем библиотеку.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    // chunks(3): по 3 байта (24 бита) → 4 символа по 6 бит.
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_rfc_4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
