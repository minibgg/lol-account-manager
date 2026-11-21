//! Вкладка «Обои»: поиск образа по имени/ID и установка его фоном профиля.
//!
//! Каталог образов берётся из CommunityDragon (см. [`crate::wallpapers`]), а сама смена
//! фона — один запрос к локальному клиенту League (см. `LcuClient::set_background_skin`).
//! Фон меняется у того аккаунта, в который сейчас выполнен вход в клиенте.

use eframe::egui::{self, RichText};

use crate::ui::terminal::FONT;
use crate::ui::theme;
use crate::ui::widgets::Button;
use crate::wallpapers::Skin;

/// Сколько результатов показываем максимум (чтобы список не разрастался).
const MAX_RESULTS: usize = 300;

/// Состояние вкладки. Живёт в `App` всё время работы программы, чтобы каталог
/// не перезагружался при каждом переключении вкладок.
#[derive(Default)]
pub struct State {
    /// Строка поиска.
    pub query: String,
    /// Весь загруженный каталог образов.
    pub skins: Vec<Skin>,
    /// Идёт загрузка каталога.
    pub loading: bool,
    /// Каталог уже запрашивали (чтобы загрузить его только один раз при открытии).
    pub requested: bool,
    /// Почему не удалось загрузить каталог.
    pub load_error: Option<String>,
    /// Выбранный в списке образ (его id).
    pub selected: Option<i64>,
    /// Идёт установка фона.
    pub setting: bool,
    /// Итог последней установки (текст для нижней строки).
    pub status: Option<String>,
}

/// Что пользователь сделал на вкладке — обрабатывает `App`.
pub enum Action {
    None,
    /// Загрузить (или перезагрузить) каталог образов.
    Load,
    /// Поставить этот образ фоном.
    Set(i64),
}

/// Рисует вкладку. `connected` — запущен ли клиент League со входом (без него фон не
/// сменить). Возвращает действие для `App`.
pub fn show(ui: &mut egui::Ui, state: &mut State, connected: bool) -> Action {
    let mut action = Action::None;

    ui.label(
        RichText::new("Фон профиля")
            .font(theme::mono(15.0))
            .color(theme::TEXT)
            .strong(),
    );
    ui.label(
        RichText::new("Найдите образ и поставьте его фоном профиля. Фон меняется у аккаунта, в который выполнен вход в клиенте.")
            .font(theme::mono(11.5))
            .color(theme::TEXT_MUTED),
    );
    ui.add_space(8.0);

    if !connected {
        ui.label(
            RichText::new("! Клиент League не подключён — запустите игру и войдите, чтобы менять фон.")
                .font(theme::mono(12.0))
                .color(theme::AMBER_TEXT),
        );
        ui.add_space(6.0);
    }

    // Загрузка каталога.
    if state.loading {
        ui.label(RichText::new("Загрузка каталога образов…").font(theme::mono(12.0)).color(theme::TEXT_MUTED));
        return action;
    }
    if let Some(error) = &state.load_error {
        ui.label(RichText::new(format!("Каталог не загрузился: {error}")).font(theme::mono(12.0)).color(theme::RED_TEXT));
        ui.add_space(6.0);
        if ui.add(Button::outline("Повторить")).clicked() {
            action = Action::Load;
        }
        return action;
    }

    // Поле поиска.
    let edit = egui::TextEdit::singleline(&mut state.query)
        .font(theme::mono(13.0))
        .text_color(theme::TEXT)
        .hint_text(RichText::new("имя чемпиона, образа или числовой ID").color(theme::TEXT_HINT))
        .desired_width(ui.available_width());
    ui.add(edit);
    ui.add_space(8.0);

    // Результаты.
    let matches: Vec<&Skin> = state.skins.iter().filter(|s| s.matches(&state.query)).take(MAX_RESULTS).collect();

    if state.query.trim().is_empty() {
        ui.label(RichText::new("Начните вводить название образа…").font(theme::mono(12.0)).color(theme::TEXT_HINT));
    } else if matches.is_empty() {
        ui.label(RichText::new("Ничего не найдено. Проверьте написание или введите ID.").font(theme::mono(12.0)).color(theme::TEXT_MUTED));
    } else {
        // Оставляем место внизу под кнопку и строку статуса.
        let list_height = (ui.available_height() - 72.0).max(100.0);
        let current = state.selected;
        let mut clicked = None;
        egui::ScrollArea::vertical().max_height(list_height).auto_shrink([false, false]).show(ui, |ui| {
            for skin in &matches {
                let label = RichText::new(format!("{}   ·   {}", skin.name, skin.id)).font(theme::mono(FONT));
                if ui.selectable_label(current == Some(skin.id), label).clicked() {
                    clicked = Some(skin.id);
                }
            }
        });
        if let Some(id) = clicked {
            state.selected = Some(id);
        }
    }

    ui.add_space(8.0);

    // Кнопка установки и строка статуса.
    ui.horizontal(|ui| {
        let can_set = connected && state.selected.is_some() && !state.setting;
        if ui.add_enabled(can_set, Button::primary("Поставить фоном")).clicked()
            && let Some(id) = state.selected
        {
            action = Action::Set(id);
        }
        if state.setting {
            ui.label(RichText::new("устанавливаю…").font(theme::mono(12.0)).color(theme::TEXT_MUTED));
        } else if let Some(status) = &state.status {
            ui.label(RichText::new(status).font(theme::mono(12.0)).color(theme::TEXT_MUTED));
        }
    });

    action
}
