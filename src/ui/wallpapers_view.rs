//! Вкладка «Обои»: сетка артов всех образов с поиском и превью выбранного образа
//! с кнопкой «Поставить фоном».
//!
//! Каталог образов берётся из CommunityDragon (см. [`crate::wallpapers`]), арты — тоже
//! оттуда, через общий кэш картинок (`ImageKey::SkinThumb` / `SkinSplash`). Сама смена
//! фона — один запрос к локальному клиенту League (`LcuClient::set_background_skin`).
//! Фон меняется у того аккаунта, в который сейчас выполнен вход в клиенте.

use eframe::egui::{self, Align, Align2, CursorIcon, Layout, Rect, Response, RichText, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

use crate::images::ImageKey;
use crate::ui::image_store::ImageStore;
use crate::ui::terminal::job;
use crate::ui::theme::{self, white};
use crate::ui::widgets::{self, Button};
use crate::wallpapers::Skin;

/// Ширина колонки превью справа.
const PREVIEW_WIDTH: f32 = 340.0;
/// Плитка в сетке — не уже этого; лишнее место делится между плитками поровну.
const CARD_MIN_WIDTH: f32 = 180.0;
/// Промежуток между плитками.
const GAP: f32 = 10.0;
/// Под артом две строки: название образа и чемпион.
const LABEL_HEIGHT: f32 = 36.0;

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
    /// Выбранный образ (его id).
    pub selected: Option<i64>,
    /// Идёт установка фона.
    pub setting: bool,
    /// Итог последней установки.
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
/// сменить). Возвращает действие и картинки, которые нужны для видимой части
/// (их `App` запросит у фонового потока).
pub fn show(ui: &mut egui::Ui, state: &mut State, images: &ImageStore, connected: bool) -> (Action, Vec<ImageKey>) {
    let mut action = Action::None;
    let mut needed = Vec::new();

    ui.label(RichText::new("Фон профиля").font(theme::mono(15.0)).color(theme::TEXT).strong());
    ui.label(
        RichText::new("Выберите арт и поставьте его фоном профиля. Фон меняется у аккаунта, в который выполнен вход в клиенте.")
            .font(theme::mono(11.5))
            .color(theme::TEXT_MUTED),
    );
    ui.add_space(8.0);

    if state.loading {
        ui.horizontal(|ui| {
            widgets::spinner(ui, 14.0);
            ui.label(RichText::new("Загрузка каталога образов…").font(theme::mono(12.0)).color(theme::TEXT_MUTED));
        });
        return (action, needed);
    }
    if let Some(error) = &state.load_error {
        ui.label(RichText::new(format!("Каталог не загрузился: {error}")).font(theme::mono(12.0)).color(theme::RED_TEXT));
        ui.add_space(6.0);
        if ui.add(Button::outline("Повторить")).clicked() {
            action = Action::Load;
        }
        return (action, needed);
    }

    let matches: Vec<&Skin> = state.skins.iter().filter(|s| s.matches(&state.query)).collect();

    // Поиск и счётчик.
    ui.horizontal(|ui| {
        let edit = egui::TextEdit::singleline(&mut state.query)
            .font(theme::mono(13.0))
            .text_color(theme::TEXT)
            .hint_text(RichText::new("поиск: чемпион, образ или ID").color(theme::TEXT_HINT))
            .desired_width((ui.available_width() - PREVIEW_WIDTH - GAP * 2.0).max(160.0));
        ui.add(edit);
        ui.label(RichText::new(format!("найдено: {}", matches.len())).font(theme::mono(12.0)).color(theme::TEXT_HINT));
    });
    ui.add_space(10.0);

    let body = ui.available_size();
    let grid_width = (body.x - PREVIEW_WIDTH - GAP * 2.0).max(CARD_MIN_WIDTH);
    let mut clicked = None;

    ui.horizontal_top(|ui| {
        // Слева — сетка.
        ui.allocate_ui_with_layout(vec2(grid_width, body.y), Layout::top_down(Align::Min), |ui| {
            if matches.is_empty() {
                ui.label(
                    RichText::new("Ничего не найдено. Проверьте написание или введите ID.")
                        .font(theme::mono(12.0))
                        .color(theme::TEXT_MUTED),
                );
                return;
            }
            clicked = grid(ui, &matches, images, state.selected, grid_width, body.y, &mut needed);
        });
        ui.add_space(GAP);
        // Справа — превью и кнопка.
        ui.allocate_ui_with_layout(vec2(PREVIEW_WIDTH, body.y), Layout::top_down(Align::Min), |ui| {
            let selected = state.selected.and_then(|id| state.skins.iter().find(|s| s.id == id));
            if let Some(id) = preview(ui, selected, state, images, connected, &mut needed) {
                action = Action::Set(id);
            }
        });
    });

    if clicked.is_some() {
        state.selected = clicked;
        state.status = None;
    }
    (action, needed)
}

/// Сетка плиток. Рисуются (и просят свои арты) только видимые ряды. Возвращает id
/// образа, по которому щёлкнули.
fn grid(
    ui: &mut egui::Ui,
    skins: &[&Skin],
    images: &ImageStore,
    selected: Option<i64>,
    width: f32,
    height: f32,
    needed: &mut Vec<ImageKey>,
) -> Option<i64> {
    let columns = (((width + GAP) / (CARD_MIN_WIDTH + GAP)).floor() as usize).max(1);
    // Немного места справа под полосу прокрутки.
    let card_width = ((width - 8.0 - GAP * (columns - 1) as f32) / columns as f32).floor();
    let art_height = (card_width * 9.0 / 16.0).round();
    let card = vec2(card_width, art_height + LABEL_HEIGHT);
    let rows = skins.len().div_ceil(columns);

    let mut clicked = None;
    ui.spacing_mut().item_spacing = vec2(GAP, GAP);
    egui::ScrollArea::vertical()
        .id_salt("wallpapers_grid")
        .max_height(height)
        .auto_shrink([false, false])
        .show_rows(ui, card.y, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for skin in skins.iter().skip(row * columns).take(columns) {
                        let key = ImageKey::SkinThumb(skin.id);
                        if skin.splash_url.is_some() {
                            needed.push(key);
                        }
                        let response = tile(ui, skin, images.get(key), selected == Some(skin.id), card, art_height);
                        if response.clicked() {
                            clicked = Some(skin.id);
                        }
                    }
                });
            }
        });
    clicked
}

/// Одна плитка: арт 16:9 и под ним название образа и чемпион.
fn tile(ui: &mut egui::Ui, skin: &Skin, image: Option<egui::Image<'static>>, selected: bool, size: Vec2, art_height: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let art = Rect::from_min_size(rect.min, vec2(rect.width(), art_height));
    art_or_placeholder(ui, art, image, skin);

    let painter = ui.painter();
    let hovered = response.hovered();
    let stroke = if selected {
        Stroke::new(2.0, theme::ACCENT)
    } else if hovered {
        Stroke::new(1.0, white(70))
    } else {
        Stroke::new(1.0, theme::BORDER)
    };
    painter.rect_stroke(art, theme::RADIUS, stroke, StrokeKind::Inside);

    let title_color = if selected { theme::ACCENT_TEXT } else { theme::TEXT };
    text_line(painter, &skin.title, theme::mono(12.0), title_color, pos2(rect.left(), art.bottom() + 5.0), rect.width());
    text_line(painter, &skin.champion, theme::mono(11.0), theme::TEXT_HINT, pos2(rect.left(), art.bottom() + 20.0), rect.width());

    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(format!("{} · {} · id {}", skin.title, skin.champion, skin.id))
}

/// Колонка превью: большой арт выбранного образа, название и кнопка «Поставить фоном».
/// Возвращает id образа, если нажали кнопку.
fn preview(
    ui: &mut egui::Ui,
    skin: Option<&Skin>,
    state: &State,
    images: &ImageStore,
    connected: bool,
    needed: &mut Vec<ImageKey>,
) -> Option<i64> {
    let Some(skin) = skin else {
        ui.label(RichText::new("Щёлкните по арту слева, чтобы выбрать образ.").font(theme::mono(12.0)).color(theme::TEXT_HINT));
        return None;
    };

    // Большой арт; пока он грузится — показываем уже загруженную миниатюру.
    let (splash, thumb) = (ImageKey::SkinSplash(skin.id), ImageKey::SkinThumb(skin.id));
    if skin.splash_url.is_some() {
        needed.extend([splash, thumb]);
    }
    let width = ui.available_width();
    let (art, _) = ui.allocate_exact_size(vec2(width, (width * 9.0 / 16.0).round()), Sense::hover());
    art_or_placeholder(ui, art, images.get(splash).or_else(|| images.get(thumb)), skin);
    ui.painter().rect_stroke(art, theme::RADIUS, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    ui.add_space(10.0);

    ui.add(egui::Label::new(RichText::new(&skin.title).font(theme::mono(14.0)).color(theme::TEXT).strong()).wrap());
    ui.label(RichText::new(format!("{}  ·  id {}", skin.champion, skin.id)).font(theme::mono(11.5)).color(theme::TEXT_MUTED));
    ui.add_space(12.0);

    let mut set = None;
    let can_set = connected && !state.setting;
    if ui.add_enabled(can_set, Button::primary("Поставить фоном").min_width(width)).clicked() {
        set = Some(skin.id);
    }
    ui.add_space(8.0);

    if !connected {
        ui.add(
            egui::Label::new(
                RichText::new("Клиент League не подключён — запустите игру и войдите в аккаунт.")
                    .font(theme::mono(11.5))
                    .color(theme::AMBER_TEXT),
            )
            .wrap(),
        );
    } else if state.setting {
        ui.label(RichText::new("устанавливаю…").font(theme::mono(11.5)).color(theme::TEXT_MUTED));
    } else if let Some(status) = &state.status {
        ui.add(egui::Label::new(RichText::new(status).font(theme::mono(11.5)).color(theme::TEXT_MUTED)).wrap());
    }
    set
}

/// Арт образа, а пока его нет — тёмная заглушка с подписью.
fn art_or_placeholder(ui: &egui::Ui, rect: Rect, image: Option<egui::Image<'static>>, skin: &Skin) {
    match image {
        Some(image) => image.corner_radius(theme::RADIUS).paint_at(ui, rect),
        None => {
            let painter = ui.painter();
            painter.rect_filled(rect, theme::RADIUS, theme::CARD);
            let text = if skin.splash_url.is_some() { "…" } else { "нет арта" };
            painter.text(rect.center(), Align2::CENTER_CENTER, text, theme::mono(11.0), theme::TEXT_HINT);
        }
    }
}

/// Строка текста, обрезанная по ширине многоточием.
fn text_line(painter: &egui::Painter, text: &str, font: egui::FontId, color: egui::Color32, pos: egui::Pos2, width: f32) {
    let mut line = job(&[(text, color)], &font);
    line.wrap = egui::text::TextWrapping::truncate_at_width(width.max(8.0));
    let galley = painter.layout_job(line);
    painter.galley(pos, galley, color);
}
