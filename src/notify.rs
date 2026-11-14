//! Уведомления Windows — всплывающие карточки в углу экрана («игра закончилась,
//! +18 LP»). Нужны, когда окно программы спрятано в трей или закрыто другими окнами.
//!
//! Windows показывает уведомление только от «известной» программы: у неё должен быть
//! идентификатор (AppUserModelID) с названием и иконкой. Обычно его прописывает
//! установщик в ярлык; мы вместо этого один раз записываем его в реестр текущего
//! пользователя (`HKCU\Software\Classes\AppUserModelId\…`) — так уведомления работают
//! и у переносной версии, и у сборки из исходников. Права администратора не нужны.
//!
//! Сами карточки показывает crate `tauri-winrt-notification` (обёртка над WinRT).
//! В тексте уведомлений — только ник, результат игры и ранг. Логины и пароли сюда
//! не попадают никогда.

use std::sync::Arc;

/// Текст уведомления: заголовок и одна-две строки под ним.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub text: String,
    /// Вторая строка, серым (например, чей аккаунт).
    pub detail: Option<String>,
}

impl Message {
    pub fn new(title: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            text: text.into(),
            detail: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Есть ли на этой системе уведомления (у нас — только Windows).
pub fn is_supported() -> bool {
    cfg!(windows)
}

/// Что сделать по щелчку на уведомлении (показать окно). `Arc<dyn Fn>` — общая
/// «функция в коробке»: её можно отдать каждому уведомлению, не копируя.
type OnClick = Arc<dyn Fn() + Send + Sync>;

/// Показывает уведомления. Создаётся один раз при запуске.
pub struct Notifier {
    on_click: OnClick,
    /// Идентификатор программы записан в реестр — уведомления будут с нашим названием.
    registered: bool,
}

impl Notifier {
    /// `on_click` вызывается (из системного потока), когда по уведомлению щёлкнули.
    pub fn new(on_click: impl Fn() + Send + Sync + 'static) -> Self {
        let registered = match imp::register() {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "уведомления Windows: не удалось зарегистрировать программу");
                false
            }
        };
        Self {
            on_click: Arc::new(on_click),
            registered,
        }
    }

    /// Показывает уведомление. Не ждёт и не мешает окну: работа идёт в отдельном
    /// коротком потоке, ошибка только пишется в лог (уведомление — не главное).
    pub fn show(&self, message: Message) {
        if !self.registered {
            return;
        }
        let on_click = Arc::clone(&self.on_click);
        std::thread::spawn(move || {
            if let Err(error) = imp::show(&message, on_click) {
                tracing::warn!(%error, "уведомление Windows не показано");
            }
        });
    }
}

#[cfg(windows)]
mod imp {
    use tauri_winrt_notification::{Duration, IconCrop, Toast};
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    use super::{Message, OnClick};
    use crate::error::{AppError, AppResult};
    use crate::storage;

    /// Идентификатор программы для Windows (AppUserModelID). НЕ МЕНЯТЬ: по нему
    /// Windows помнит настройки уведомлений пользователя («не показывать» и т. п.).
    const APP_ID: &str = "minibgg.LoLAccountManager";
    const APP_NAME: &str = "LoL Account Manager";
    const ICON_FILE: &str = "notification-icon.png";

    fn registry_error(e: std::io::Error) -> AppError {
        AppError::Io(format!("реестр Windows: {e}"))
    }

    fn icon_path() -> AppResult<std::path::PathBuf> {
        Ok(storage::data_dir()?.join(ICON_FILE))
    }

    /// Записывает название и иконку программы в реестр (каждый запуск — на случай,
    /// если папку данных перенесли). Иконку кладём файлом в папку данных: Windows
    /// нужен путь к файлу, а не байты.
    pub fn register() -> AppResult<()> {
        let icon = icon_path()?;
        let bytes: &[u8] = include_bytes!("../assets/icon.png");
        // Не переписываем файл без нужды: он не меняется от запуска к запуску.
        if std::fs::metadata(&icon).map(|m| m.len()).ok() != Some(bytes.len() as u64) {
            std::fs::write(&icon, bytes)?;
        }
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}"))
            .map_err(registry_error)?;
        key.set_value("DisplayName", &APP_NAME).map_err(registry_error)?;
        key.set_value("IconUri", &icon.display().to_string())
            .map_err(registry_error)?;
        Ok(())
    }

    pub fn show(message: &Message, on_click: OnClick) -> AppResult<()> {
        let mut toast = Toast::new(APP_ID)
            .title(&message.title)
            .text1(&message.text)
            .duration(Duration::Short)
            .on_activated(move |_| {
                on_click();
                Ok(())
            });
        if let Some(detail) = &message.detail {
            toast = toast.text2(detail);
        }
        if let Ok(icon) = icon_path()
            && icon.exists()
        {
            toast = toast.icon(&icon, IconCrop::Square, APP_NAME);
        }
        toast
            .show()
            .map_err(|e| AppError::Internal(format!("уведомление Windows: {e}")))
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Message, OnClick};
    use crate::error::{AppError, AppResult};

    pub fn register() -> AppResult<()> {
        Err(AppError::Internal("уведомления есть только в Windows".into()))
    }

    pub fn show(_message: &Message, _on_click: OnClick) -> AppResult<()> {
        Ok(())
    }
}
