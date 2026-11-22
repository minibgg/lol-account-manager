//! Глобальная горячая клавиша Ctrl+Shift+L: показать окно программы откуда угодно
//! (из браузера, с рабочего стола, из клиента League), а если окно уже перед глазами —
//! спрятать его обратно.
//!
//! Как это работает в Windows: `RegisterHotKey` просит систему присылать сообщение
//! `WM_HOTKEY`, когда нажато сочетание, — в очередь сообщений того потока, который его
//! зарегистрировал. Поэтому у клавиши свой маленький поток: он регистрирует сочетание и
//! ждёт сообщений в `GetMessageW`. Нажатие — отправляем в канал и будим окно
//! (`request_repaint`), окно забирает нажатия в начале кадра ([`GlobalHotkey::poll`]).
//!
//! Программа ничего не нажимает сама и не читает другие клавиши: система сообщает только
//! об одном зарегистрированном сочетании. Если его уже заняла другая программа,
//! регистрация не удастся — об этом скажет ошибка в настройках.

/// Сочетание — для подсказок в окне.
pub const LABEL: &str = "Ctrl+Shift+L";

/// Есть ли глобальные клавиши на этой системе (у нас — только Windows).
pub fn is_supported() -> bool {
    cfg!(windows)
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc::{self, Receiver};
    use std::thread::JoinHandle;

    use eframe::egui;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, PostThreadMessageW, WM_HOTKEY, WM_QUIT};

    use crate::error::{AppError, AppResult};

    /// Номер сочетания внутри нашего потока (любой, лишь бы свой).
    const HOTKEY_ID: i32 = 1;
    /// Код клавиши L (виртуальные коды букв совпадают с заглавными ASCII).
    const VK_L: u32 = b'L' as u32;

    pub struct GlobalHotkey {
        presses: Receiver<()>,
        /// Поток с очередью сообщений и его номер — чтобы попросить его завершиться.
        thread: Option<JoinHandle<()>>,
        thread_id: u32,
    }

    impl GlobalHotkey {
        /// Регистрирует Ctrl+Shift+L. Ошибка — сочетание уже занято другой программой.
        pub fn new(ctx: egui::Context) -> AppResult<Self> {
            let (tx, presses) = mpsc::channel();
            // Поток сообщает, удалась ли регистрация, и только потом начинает ждать нажатий.
            let (ready_tx, ready) = mpsc::channel::<Result<u32, String>>();
            let thread = std::thread::Builder::new()
                .name("global-hotkey".into())
                .spawn(move || {
                    // SAFETY: вызовы Win32 с корректными аргументами; окно не нужно (None) —
                    // тогда WM_HOTKEY приходит в очередь этого потока.
                    let registered =
                        unsafe { RegisterHotKey(None, HOTKEY_ID, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, VK_L) };
                    if let Err(error) = registered {
                        let _ = ready_tx.send(Err(error.message()));
                        return;
                    }
                    let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));
                    let mut msg = MSG::default();
                    // GetMessageW возвращает 0 на WM_QUIT и -1 при ошибке — тогда выходим.
                    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                        if msg.message == WM_HOTKEY && msg.wParam.0 == HOTKEY_ID as usize {
                            let _ = tx.send(());
                            ctx.request_repaint(); // разбудить окно, даже если оно спрятано
                        }
                    }
                    let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
                })
                .map_err(|e| AppError::Internal(format!("поток горячей клавиши: {e}")))?;
            match ready.recv() {
                Ok(Ok(thread_id)) => Ok(Self {
                    presses,
                    thread: Some(thread),
                    thread_id,
                }),
                Ok(Err(message)) => {
                    let _ = thread.join();
                    Err(AppError::Internal(format!(
                        "сочетание {} занято другой программой ({message})",
                        super::LABEL
                    )))
                }
                Err(_) => Err(AppError::Internal("поток горячей клавиши завершился".into())),
            }
        }

        /// Нажимали ли сочетание с прошлого кадра (несколько нажатий — одно).
        pub fn pressed(&self) -> bool {
            let mut any = false;
            while self.presses.try_recv().is_ok() {
                any = true;
            }
            any
        }
    }

    impl Drop for GlobalHotkey {
        /// Выключили в настройках или программа закрывается: снимаем сочетание.
        fn drop(&mut self) {
            // SAFETY: просим наш же поток выйти из цикла сообщений.
            let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use eframe::egui;

    use crate::error::{AppError, AppResult};

    pub struct GlobalHotkey;

    impl GlobalHotkey {
        pub fn new(_ctx: egui::Context) -> AppResult<Self> {
            Err(AppError::Internal("глобальная клавиша есть только в Windows".into()))
        }

        pub fn pressed(&self) -> bool {
            false
        }
    }
}

pub use imp::GlobalHotkey;

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use eframe::egui;

    /// Настоящая регистрация в Windows: сочетание занимается, второй раз его занять
    /// нельзя, после drop оно свободно. Трогает общесистемное сочетание — поэтому
    /// только вручную: `cargo test --lib hotkey -- --ignored`.
    #[test]
    #[ignore]
    fn registers_and_releases_the_combination() {
        let ctx = egui::Context::default();
        let first = GlobalHotkey::new(ctx.clone()).expect("сочетание свободно");
        assert!(!first.pressed());
        let second = GlobalHotkey::new(ctx.clone());
        assert!(second.is_err(), "второй раз то же сочетание не регистрируется");
        drop(first);
        let again = GlobalHotkey::new(ctx).expect("после drop сочетание снова свободно");
        drop(again);
    }
}
