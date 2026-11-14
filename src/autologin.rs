//! Вход в аккаунт одним нажатием (Shift+Enter): программа сама вводит логин и пароль
//! в окно входа Riot Client — так же, как это сделал бы человек с клавиатуры.
//!
//! Почему именно «с клавиатуры»: войти через внутренний API Riot Client нельзя — он
//! требует пройти капчу, а её решает только окно самого клиента.
//!
//! Что важно для безопасности:
//! - пароль печатается **только** когда на переднем плане окно Riot Client. Это
//!   проверяется перед логином, перед паролем и перед Enter; если фокус ушёл в другое
//!   окно (игра, чат, браузер) — ввод сразу прерывается, и пароль туда не попадёт;
//! - символы отправляются как «юникод-нажатия» (не зависят от раскладки клавиатуры)
//!   и не проходят через буфер обмена;
//! - сюда логин и пароль приходят из системного хранилища паролей и после ввода
//!   затираются в памяти.
//!
//! Замечание о правилах Riot: это автоматизация входа. Риск для аккаунта невелик
//! (ввод неотличим от ручного), но это не «официальный» способ — поэтому обычная
//! кнопка «Войти» (Enter) по-прежнему ничего в клиент не вводит.

use crate::error::AppResult;

/// Имя процесса окна Riot Client.
pub const RIOT_WINDOW_PROCESS: &str = "Riot Client.exe";

/// Есть ли на этой системе автовход (у нас — только Windows).
pub fn is_supported() -> bool {
    cfg!(windows)
}

/// Окно на переднем плане принадлежит Riot Client?
pub fn riot_window_focused() -> bool {
    imp::riot_window_focused()
}

/// Вводит логин и пароль в окно входа Riot Client и нажимает Enter. Блокирует поток
/// примерно на секунду — вызывать из блокирующей задачи. Ошибка — окно Riot Client
/// потеряло фокус, ввод прерван.
pub fn type_credentials(login: &str, password: &str) -> AppResult<()> {
    imp::type_credentials(login, password)
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::thread::sleep;
    use std::time::Duration;

    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput,
        VIRTUAL_KEY, VK_A, VK_CONTROL, VK_RETURN, VK_TAB,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    use super::RIOT_WINDOW_PROCESS;
    use crate::error::{AppError, AppResult};

    /// Пауза между нажатиями: окно клиента — веб-страница, слишком быстрый ввод она теряет.
    const KEY_DELAY: Duration = Duration::from_millis(12);
    /// Пауза между полями (логин → Tab → пароль → Enter).
    const FIELD_DELAY: Duration = Duration::from_millis(150);

    pub fn riot_window_focused() -> bool {
        let mut pid = 0u32;
        // SAFETY: обычные вызовы Windows API без указателей на наши данные, кроме
        // `pid` — действительной переменной, в которую Windows записывает номер процесса.
        unsafe {
            let window = GetForegroundWindow();
            if window.is_invalid() {
                return false;
            }
            GetWindowThreadProcessId(window, Some(&mut pid));
        }
        if pid == 0 {
            return false;
        }
        // Имя процесса по номеру — через sysinfo (тот же способ, что и для клиента League).
        let Ok(mut system) = System::new() else {
            return false;
        };
        let pid = Pid::from_u32(pid);
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, ProcessRefreshKind::nothing());
        system
            .process(pid)
            .is_some_and(|process| process.name() == OsStr::new(RIOT_WINDOW_PROCESS))
    }

    fn lost_focus() -> AppError {
        AppError::AutoLogin(
            "окно Riot Client потеряло фокус — ввод прерван, пароль никуда не введён. \
             Откройте окно Riot Client и нажмите Shift+Enter ещё раз"
                .into(),
        )
    }

    fn ensure_focused() -> AppResult<()> {
        if riot_window_focused() { Ok(()) } else { Err(lost_focus()) }
    }

    fn key_event(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn send(inputs: &[INPUT]) -> AppResult<()> {
        // SAFETY: передаём срез правильно заполненных структур INPUT и их размер.
        let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
        if sent as usize == inputs.len() {
            Ok(())
        } else {
            Err(AppError::AutoLogin("Windows не приняла ввод с клавиатуры".into()))
        }
    }

    /// Нажать и отпустить клавишу (Tab, Enter).
    fn press(vk: VIRTUAL_KEY) -> AppResult<()> {
        send(&[key_event(vk, 0, KEYBD_EVENT_FLAGS(0)), key_event(vk, 0, KEYEVENTF_KEYUP)])
    }

    /// Ctrl+A — выделить всё в поле (чтобы заменить то, что там могло остаться).
    fn select_all() -> AppResult<()> {
        send(&[
            key_event(VK_CONTROL, 0, KEYBD_EVENT_FLAGS(0)),
            key_event(VK_A, 0, KEYBD_EVENT_FLAGS(0)),
            key_event(VK_A, 0, KEYEVENTF_KEYUP),
            key_event(VK_CONTROL, 0, KEYEVENTF_KEYUP),
        ])
    }

    /// Напечатать текст «юникод-нажатиями»: раскладка клавиатуры не важна.
    fn type_text(text: &str) -> AppResult<()> {
        // encode_utf16 — Windows принимает символы в UTF-16 (редкие символы — двумя частями).
        for unit in text.encode_utf16() {
            // Перед каждым символом: фокус всё ещё у Riot Client?
            ensure_focused()?;
            send(&[
                key_event(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE),
                key_event(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
            ])?;
            sleep(KEY_DELAY);
        }
        Ok(())
    }

    pub fn type_credentials(login: &str, password: &str) -> AppResult<()> {
        ensure_focused()?;
        select_all()?;
        sleep(KEY_DELAY);
        type_text(login)?;
        sleep(FIELD_DELAY);
        ensure_focused()?;
        press(VK_TAB)?;
        sleep(FIELD_DELAY);
        ensure_focused()?;
        select_all()?;
        sleep(KEY_DELAY);
        type_text(password)?;
        sleep(FIELD_DELAY);
        ensure_focused()?;
        press(VK_RETURN)
    }
}

#[cfg(not(windows))]
mod imp {
    use crate::error::{AppError, AppResult};

    pub fn riot_window_focused() -> bool {
        false
    }

    pub fn type_credentials(_login: &str, _password: &str) -> AppResult<()> {
        Err(AppError::AutoLogin("автовход есть только в Windows".into()))
    }
}
