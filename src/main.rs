// В release-сборке на Windows не открываем чёрное консольное окно рядом с приложением.
// В debug (`cargo run`) консоль остаётся: туда пишутся логи tracing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Модули объявлены в src/lib.rs (библиотека `lol_account_manager`),
// а main.rs — отдельная программа, которая её использует.
use std::sync::Arc;

use eframe::egui;
use lol_account_manager::app::App;
use lol_account_manager::storage;
use tracing_subscriber::EnvFilter;

/// Файл в папке данных, где eframe хранит размер и положение окна (и сортировку списка).
/// Обычный текст, только несекретное.
const WINDOW_STATE_FILE: &str = "window.ron";

/// `eframe::Result` — это `Result<(), eframe::Error>`.
/// Result в Rust — это либо `Ok(значение)`, либо `Err(ошибка)`; исключений нет.
/// Если main возвращает Err, программа завершится и напечатает ошибку.
fn main() -> eframe::Result {
    init_logging();
    tracing::info!("запуск LoL Account Manager v{}", env!("CARGO_PKG_VERSION"));

    // Где запоминать окно. Нет папки данных — окно просто не запоминается.
    let window_state = storage::data_dir().ok().map(|dir| dir.join(WINDOW_STATE_FILE));
    // eframe ставит окно по центру даже поверх сохранённого положения, поэтому
    // центрируем только при первом запуске (файла ещё нет).
    let first_run = window_state.as_ref().is_none_or(|file| !file.exists());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LoL Account Manager")
            .with_icon(Arc::new(app_icon()))
            // Без системной рамки и заголовка: свою шапку с кнопками окна рисует
            // приложение (ui/window_frame.rs), как в reveal. Тень и «прилипание»
            // к краям экрана Windows оставляет.
            .with_decorations(false)
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([940.0, 540.0]),
        centered: first_run,
        persist_window: true,
        persistence_path: window_state,
        wgpu_options: wgpu_options(),
        // `..Default::default()` — «остальные поля возьми из значения по умолчанию».
        ..Default::default()
    };

    eframe::run_native(
        "LoL Account Manager",
        options,
        // Замыкание, которое eframe вызовет один раз, когда окно и графика готовы.
        // `cc` (CreationContext) даёт доступ к egui::Context: через него ставим тему.
        // Box::new(...) кладёт значение в кучу: eframe хранит приложение как `Box<dyn App>` —
        // «какой-то тип, реализующий трейт App», размер которого неизвестен заранее.
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// Иконка окна (заголовок и панель задач). Та же картинка вшита в exe через build.rs —
/// её показывают ярлыки и проводник. Нарисована кодом: examples/make_icon.rs.
fn app_icon() -> egui::IconData {
    // include_bytes! вставляет файл в программу при компиляции — отдельно его возить не нужно.
    // Ошибки здесь быть не может (файл проверяет тест ниже), но на всякий случай —
    // пустая иконка вместо паники: окно тогда просто получит стандартную.
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default()
}

/// Настройки графики (wgpu).
///
/// На Windows рисуем через родной DirectX 12 (запасной вариант — OpenGL) и не трогаем Vulkan:
/// сторонние Vulkan-слои (оверлеи Epic, Steam, OBS) часто зарегистрированы криво
/// и засоряют лог ошибками. Если задана переменная окружения WGPU_BACKEND
/// (`dx12`, `vulkan`, `gl`), она имеет приоритет.
/// На Linux и других системах оставляем выбор eframe по умолчанию.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew, wgpu};

    let mut setup = WgpuSetupCreateNew::without_display_handle();
    // `cfg!(windows)` — true при компиляции под Windows. В отличие от `#[cfg]`, код
    // компилируется на всех платформах, просто ветка на остальных никогда не выполнится.
    // `is_none()` — у Option<T> два варианта: Some(значение) или None («ничего»).
    if cfg!(windows) && wgpu::Backends::from_env().is_none() {
        setup.instance_descriptor.backends = wgpu::Backends::DX12 | wgpu::Backends::GL;
    }

    WgpuConfiguration {
        wgpu_setup: WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

/// Логи в консоль. Уровень меняется переменной окружения RUST_LOG, например:
/// `$env:RUST_LOG="debug"; cargo run` (PowerShell) или `RUST_LOG=debug cargo run` (bash).
fn init_logging() {
    // try_from_default_env() возвращает Result: Ok, если RUST_LOG задан и корректен.
    // unwrap_or_else(...) — «если Err, то используй значение из замыкания».
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,lol_account_manager=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_icon_is_a_valid_png() {
        let icon = super::app_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }
}
