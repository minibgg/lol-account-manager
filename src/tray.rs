//! Значок в трее (область уведомлений Windows, рядом с часами).
//!
//! Когда включено «при закрытии сворачивать в трей», крестик не завершает программу,
//! а прячет окно: значок остаётся в трее. Щелчок по значку возвращает окно, правая
//! кнопка открывает меню «Открыть / Выход».
//!
//! Значок создаёт и обслуживает crate `tray-icon`. Его события приходят в главный поток
//! вне кадра egui, поэтому мы складываем их в канал и будим окно (`request_repaint`),
//! а приложение забирает их в начале кадра ([`Tray::poll`]).

/// Что нажали в трее.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Щелчок по значку или пункт «Открыть».
    Show,
    /// Пункт «Выход».
    Quit,
}

/// Есть ли на этой системе трей (у нас — только Windows).
pub fn is_supported() -> bool {
    cfg!(windows)
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc::{self, Receiver};

    use eframe::egui;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::TrayAction;
    use crate::error::{AppError, AppResult};

    /// Сторона значка в пикселях (Windows сама уменьшит до 16–24 под масштаб экрана).
    const ICON_SIZE: u32 = 32;

    pub struct Tray {
        /// Пока это значение живо, значок виден. Удалили (drop) — значок исчез.
        _icon: TrayIcon,
        actions: Receiver<TrayAction>,
    }

    fn tray_error(e: impl std::fmt::Display) -> AppError {
        AppError::Internal(format!("значок в трее: {e}"))
    }

    fn icon() -> AppResult<Icon> {
        let image = image::load_from_memory(include_bytes!("../assets/icon.png"))
            .map_err(tray_error)?
            .resize(ICON_SIZE, ICON_SIZE, image::imageops::FilterType::Lanczos3)
            .into_rgba8();
        let (width, height) = image.dimensions();
        Icon::from_rgba(image.into_raw(), width, height).map_err(tray_error)
    }

    impl Tray {
        /// Создаёт значок. Вызывать из главного потока (там, где окно).
        pub fn new(ctx: egui::Context) -> AppResult<Self> {
            let show = MenuItem::new("Открыть", true, None);
            let quit = MenuItem::new("Выход", true, None);
            let menu = Menu::new();
            menu.append_items(&[&show, &PredefinedMenuItem::separator(), &quit])
                .map_err(tray_error)?;
            let (show_id, quit_id) = (show.id().clone(), quit.id().clone());

            let tray = TrayIconBuilder::new()
                .with_tooltip("LoL Account Manager")
                .with_icon(icon()?)
                .with_menu(Box::new(menu))
                // Левая кнопка — показать окно, меню только по правой.
                .with_menu_on_left_click(false)
                .build()
                .map_err(tray_error)?;

            // Канал: обработчики событий кладут действия, приложение их забирает.
            let (tx, actions) = mpsc::channel();
            let (menu_tx, menu_ctx) = (tx.clone(), ctx.clone());
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let action = if event.id == show_id {
                    TrayAction::Show
                } else if event.id == quit_id {
                    TrayAction::Quit
                } else {
                    return;
                };
                let _ = menu_tx.send(action);
                menu_ctx.request_repaint(); // разбудить окно, даже если оно спрятано
            }));
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                // Отпустили левую кнопку на значке.
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    let _ = tx.send(TrayAction::Show);
                    ctx.request_repaint();
                }
            }));
            Ok(Self { _icon: tray, actions })
        }

        /// Следующее нажатие в трее, если было.
        pub fn poll(&self) -> Option<TrayAction> {
            self.actions.try_recv().ok()
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use eframe::egui;

    use super::TrayAction;
    use crate::error::{AppError, AppResult};

    /// На других системах значка нет: создать его нельзя.
    pub struct Tray;

    impl Tray {
        pub fn new(_ctx: egui::Context) -> AppResult<Self> {
            Err(AppError::Internal("значок в трее есть только в Windows".into()))
        }

        pub fn poll(&self) -> Option<TrayAction> {
            None
        }
    }
}

pub use imp::Tray;
