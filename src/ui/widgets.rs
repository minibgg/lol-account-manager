//! Свои элементы интерфейса в стиле reveal: кнопки, кнопки-значки, плашки,
//! переключатель вкладок, точка статуса и сами значки (рисуются линиями).
//!
//! В egui свой виджет — это обычный код: занять место (`allocate_exact_size`),
//! узнать по ответу (`Response`), наведён ли курсор и было ли нажатие, и нарисовать
//! прямоугольники и текст через `painter`. Каждый кадр заново — как render() в React.

use std::f32::consts::{PI, TAU};

use eframe::egui::{
    self, Color32, CursorIcon, Pos2, Rect, Response, RichText, Sense, Shadow,
    Stroke, StrokeKind, TextWrapMode, Ui, Vec2, Widget, WidgetText, pos2, vec2,
};

use crate::ui::theme::{self, Weight, alpha, white};

// ----- Кнопки ----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Синяя, с мягким свечением — главное действие.
    Primary,
    /// Тёмная с тонкой рамкой — обычное действие.
    Outline,
    /// Красная — удаление.
    Danger,
}

/// Кнопка с текстом и, если нужно, значком слева.
///
/// Реализует трейт [`Widget`], поэтому работает со стандартными
/// `ui.add(...)` и `ui.add_enabled(false, ...)`.
pub struct Button {
    text: String,
    icon: Option<Icon>,
    kind: ButtonKind,
    small: bool,
    min_width: f32,
}

impl Button {
    fn new(kind: ButtonKind, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            icon: None,
            kind,
            small: false,
            min_width: 0.0,
        }
    }

    pub fn primary(text: impl Into<String>) -> Self {
        Self::new(ButtonKind::Primary, text)
    }

    pub fn outline(text: impl Into<String>) -> Self {
        Self::new(ButtonKind::Outline, text)
    }

    pub fn danger(text: impl Into<String>) -> Self {
        Self::new(ButtonKind::Danger, text)
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Компактная кнопка (28 px) — для строк списка.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }
}

impl Widget for Button {
    fn ui(self, ui: &mut Ui) -> Response {
        let (height, font_size, pad_x, icon_size) = if self.small {
            (28.0, 11.5, 10.0, 13.0)
        } else {
            (34.0, 12.5, 14.0, 14.0)
        };
        let galley = text_galley(ui, RichText::new(&self.text).size(font_size).medium());
        let icon_width = if self.icon.is_some() { icon_size + 6.0 } else { 0.0 };
        let width = (galley.size().x + icon_width + pad_x * 2.0).max(self.min_width);

        let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
        if ui.is_rect_visible(rect) {
            let enabled = ui.is_enabled();
            let hovered = enabled && response.hovered();
            let pressed = enabled && response.is_pointer_button_down_on();
            let (fill, stroke, text_color) = match self.kind {
                ButtonKind::Primary => (
                    if pressed { theme::ACCENT } else if hovered { theme::ACCENT_LIGHT } else { theme::ACCENT },
                    Stroke::NONE,
                    theme::ON_ACCENT,
                ),
                ButtonKind::Outline => (
                    white(if pressed { 24 } else if hovered { 18 } else { 9 }),
                    Stroke::new(1.0, white(if hovered { 38 } else { 26 })),
                    theme::TEXT,
                ),
                ButtonKind::Danger => (
                    alpha(theme::RED, if hovered { 255 } else { 205 }),
                    Stroke::NONE,
                    Color32::WHITE,
                ),
            };
            // Выключенная кнопка — полупрозрачная (opacity-50 в reveal).
            let dim = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };

            let painter = ui.painter();
            if self.kind == ButtonKind::Primary && enabled && hovered {
                // Еле заметное свечение акцента под кнопкой — только под курсором.
                let glow = Shadow {
                    offset: [0, 4],
                    blur: 14,
                    spread: 0,
                    color: alpha(theme::ACCENT, 40),
                };
                painter.add(glow.as_shape(rect, theme::RADIUS));
            }
            painter.rect(rect, theme::RADIUS, dim(fill), Stroke::new(stroke.width, dim(stroke.color)), StrokeKind::Inside);

            let content_width = icon_width + galley.size().x;
            let mut x = rect.center().x - content_width / 2.0;
            if let Some(icon) = self.icon {
                let icon_rect = Rect::from_center_size(pos2(x + icon_size / 2.0, rect.center().y), Vec2::splat(icon_size));
                paint_icon(painter, icon_rect, icon, dim(text_color));
                x += icon_width;
            }
            let text_pos = pos2(x, rect.center().y - galley.size().y / 2.0);
            painter.galley(text_pos, galley, dim(text_color));
        }
        if ui.is_enabled() {
            response.on_hover_cursor(CursorIcon::PointingHand)
        } else {
            response
        }
    }
}

/// Квадратная кнопка-значок (28 px) без рамки — «обновить», «изменить», «удалить».
/// Всплывающую подсказку добавляйте через `.on_hover_text(...)`: значок без слов.
pub struct IconButton {
    icon: Icon,
    danger: bool,
    size: f32,
    /// Цвет значка в покое (по умолчанию — приглушённый).
    color: Color32,
}

impl IconButton {
    pub fn new(icon: Icon) -> Self {
        Self {
            icon,
            danger: false,
            size: 28.0,
            color: theme::TEXT_MUTED,
        }
    }

    /// При наведении — красный («удалить», «закрыть окно»).
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Color32) -> Self {
        self.color = color;
        self
    }
}

impl Widget for IconButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(self.size), Sense::click());
        if ui.is_rect_visible(rect) {
            let enabled = ui.is_enabled();
            let hovered = enabled && response.hovered();
            let (fill, color) = match (hovered, self.danger) {
                (true, true) => (alpha(theme::RED, 38), theme::RED_TEXT),
                (true, false) => (white(13), theme::TEXT),
                (false, _) => (Color32::TRANSPARENT, self.color),
            };
            let color = if enabled { color } else { color.gamma_multiply(0.35) };
            let painter = ui.painter();
            painter.rect_filled(rect, 6, fill);
            let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(14.0));
            paint_icon(painter, icon_rect, self.icon, color);
        }
        if ui.is_enabled() {
            response.on_hover_cursor(CursorIcon::PointingHand)
        } else {
            response
        }
    }
}

/// Плашка-переключатель (тег в форме): выбрана — синяя, нет — тёмная.
pub fn toggle_chip(ui: &mut Ui, selected: &mut bool, text: &str) -> Response {
    let galley = text_galley(ui, RichText::new(text).size(11.5).medium());
    let size = vec2(galley.size().x + 22.0, 28.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *selected = !*selected;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let (fill, stroke, color) = if *selected {
            (alpha(theme::ACCENT, 38), alpha(theme::ACCENT_LIGHT, 100), theme::ACCENT_TEXT)
        } else {
            (white(if hovered { 16 } else { 8 }), white(if hovered { 38 } else { 24 }), theme::TEXT_MUTED)
        };
        let painter = ui.painter();
        painter.rect(rect, 6, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
        painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

// ----- Плашки ----------------------------------------------------------------

/// Цвет плашки.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Accent,
    Green,
    Amber,
    Red,
}

impl Tone {
    /// (фон, рамка, текст).
    fn colors(self) -> (Color32, Color32, Color32) {
        match self {
            Self::Neutral => (white(13), white(26), theme::TEXT_MUTED),
            Self::Accent => (alpha(theme::ACCENT, 26), alpha(theme::ACCENT_LIGHT, 51), theme::ACCENT_TEXT),
            Self::Green => (alpha(theme::GREEN, 26), alpha(theme::GREEN, 51), theme::GREEN_TEXT),
            Self::Amber => (alpha(theme::AMBER, 26), alpha(theme::AMBER, 51), theme::AMBER_TEXT),
            Self::Red => (alpha(theme::RED, 38), alpha(theme::RED, 77), theme::RED_TEXT),
        }
    }

    /// Цвет текста этого тона.
    pub fn text(self) -> Color32 {
        self.colors().2
    }
}

/// Маленькая скруглённая плашка: «v0.1.0», «3», «в клиенте», «4/5 найдено».
pub fn pill(ui: &mut Ui, text: &str, tone: Tone) -> Response {
    badge(ui, text, tone, 9)
}

/// Плашка с почти прямыми углами: регион, теги аккаунта.
pub fn tag(ui: &mut Ui, text: &str, tone: Tone) -> Response {
    badge(ui, text, tone, 4)
}

fn badge(ui: &mut Ui, text: &str, tone: Tone, radius: u8) -> Response {
    let (fill, stroke, color) = tone.colors();
    let galley = text_galley(ui, RichText::new(text).size(10.0).medium());
    let size = vec2(galley.size().x + 12.0, 18.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect(rect, radius, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
        painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    }
    response
}

/// Точка статуса (6 px). `glow` — со свечением вокруг (подключено, в игре).
pub fn status_dot(ui: &mut Ui, color: Color32, glow: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
    let center = rect.center();
    let painter = ui.painter();
    if glow {
        painter.circle_filled(center, 6.0, alpha(color, 30));
        painter.circle_filled(center, 4.5, alpha(color, 60));
    }
    painter.circle_filled(center, 3.0, color);
    response
}

/// Квадратная подложка со значком (как у reveal на экранах «нет клиента», «пусто»).
pub fn icon_tile(ui: &mut Ui, icon: Icon, tone: Tone, size: f32) -> Response {
    let (fill, stroke, color) = tone.colors();
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    painter.add(
        Shadow {
            offset: [0, 0],
            blur: 30,
            spread: 0,
            color: alpha(color, 30),
        }
        .as_shape(rect, 12),
    );
    painter.rect(rect, 12, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(size * 0.42));
    paint_icon(painter, icon_rect, icon, color);
    response
}

/// Небольшой индикатор загрузки нужного размера и цвета.
pub fn spinner(ui: &mut Ui, size: f32) -> Response {
    ui.add(egui::Spinner::new().size(size).color(theme::ACCENT_LIGHT))
}

// ----- Переключатель вкладок -------------------------------------------------

/// Один вариант переключателя: значение, подпись и, если нужно, счётчик или точка.
pub struct Segment<T> {
    pub value: T,
    pub label: String,
    /// Число рядом с подписью («Аккаунты 3»), бледнее основного текста.
    pub count: Option<usize>,
    /// Цветная точка («Лобби ●» — идёт выбор чемпионов).
    pub dot: Option<Color32>,
}

impl<T> Segment<T> {
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
            count: None,
            dot: None,
        }
    }

    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    pub fn dot(mut self, color: Option<Color32>) -> Self {
        self.dot = color;
        self
    }
}

/// Группа вкладок в общей рамке, выбранная подсвечена (как Reveal / History / Auto).
/// Возвращает true, если выбор изменился.
///
/// `T: PartialEq + Clone` — значение можно сравнивать и копировать (вкладка, тег).
pub fn segmented<T: PartialEq + Clone>(
    ui: &mut Ui,
    id_salt: &str,
    current: &mut T,
    segments: &[Segment<T>],
    height: f32,
) -> bool {
    const PAD: f32 = 2.0;
    let font_size = if height >= 28.0 { 12.0 } else { 11.5 };
    // Сначала раскладываем все подписи, чтобы знать общую ширину группы.
    let items: Vec<_> = segments
        .iter()
        .map(|s| {
            let label = text_galley(ui, RichText::new(&s.label).size(font_size).medium());
            let count = s
                .count
                .map(|n| text_galley(ui, RichText::new(n.to_string()).size(font_size - 1.5).medium()));
            let extra = count.as_ref().map_or(0.0, |c| c.size().x + 5.0) + if s.dot.is_some() { 10.0 } else { 0.0 };
            let width = label.size().x + extra + 20.0;
            (label, count, width)
        })
        .collect();
    let total = items.iter().map(|(_, _, w)| w).sum::<f32>() + PAD * 2.0;
    let (outer, _) = ui.allocate_exact_size(vec2(total, height), Sense::hover());

    let painter = ui.painter();
    painter.rect(outer, 8, white(6), Stroke::new(1.0, white(18)), StrokeKind::Inside);

    let mut changed = false;
    let mut x = outer.left() + PAD;
    for (index, (segment, (label, count, width))) in segments.iter().zip(items).enumerate() {
        let rect = Rect::from_min_size(pos2(x, outer.top() + PAD), vec2(width, height - PAD * 2.0));
        x += width;
        let response = ui
            .interact(rect, ui.scope_id().with((id_salt, index)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() && *current != segment.value {
            *current = segment.value.clone();
            changed = true;
        }
        let selected = *current == segment.value;
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 6, white(26));
        }
        let color = if selected || response.hovered() { theme::TEXT } else { theme::TEXT_MUTED };

        let content = label.size().x
            + count.as_ref().map_or(0.0, |c| c.size().x + 5.0)
            + if segment.dot.is_some() { 10.0 } else { 0.0 };
        let mut cx = rect.center().x - content / 2.0;
        let label_width = label.size().x;
        painter.galley(pos2(cx, rect.center().y - label.size().y / 2.0), label, color);
        cx += label_width;
        if let Some(count) = count {
            cx += 5.0;
            let count_width = count.size().x;
            painter.galley(pos2(cx, rect.center().y - count.size().y / 2.0), count, color.gamma_multiply(0.7));
            cx += count_width;
        }
        if let Some(dot) = segment.dot {
            painter.circle_filled(pos2(cx + 6.0, rect.center().y), 2.5, dot);
        }
    }
    changed
}

// ----- Значки ----------------------------------------------------------------

/// Значки рисуются линиями, как SVG-иконки в reveal (сетка 16×16, линия 1.4 px).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Refresh,
    Edit,
    Trash,
    User,
    Key,
    Copy,
    ExternalLink,
    Plus,
    Search,
    Check,
    Warning,
    Play,
    Login,
    /// Стрелка вниз в лоток — «сохранить в файл».
    Download,
    /// Стрелка вверх из лотка — «загрузить из файла».
    Upload,
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Рисует значок в квадрате `rect` цветом `color`.
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    // Координаты значков заданы в сетке 16×16; `p(x, y)` переводит их в точки экрана.
    let scale = rect.width() / 16.0;
    let p = |x: f32, y: f32| rect.min + vec2(x, y) * scale;
    let stroke = Stroke::new(1.4 * scale.max(0.8), color);
    let line = |points: &[Pos2]| {
        painter.add(egui::Shape::line(points.to_vec(), stroke));
    };
    let arc = |center: Pos2, radius: f32, from: f32, to: f32| -> Vec<Pos2> {
        let steps = 24;
        (0..=steps)
            .map(|i| {
                let angle = from + (to - from) * i as f32 / steps as f32;
                center + vec2(angle.cos(), angle.sin()) * radius
            })
            .collect()
    };

    match icon {
        Icon::Close => {
            line(&[p(4.0, 4.0), p(12.0, 12.0)]);
            line(&[p(12.0, 4.0), p(4.0, 12.0)]);
        }
        Icon::Minimize => line(&[p(3.5, 8.0), p(12.5, 8.0)]),
        Icon::Maximize => {
            painter.rect_stroke(Rect::from_min_max(p(3.5, 3.5), p(12.5, 12.5)), 1.5 * scale, stroke, StrokeKind::Middle);
        }
        Icon::Restore => {
            painter.rect_stroke(Rect::from_min_max(p(3.5, 5.5), p(10.5, 12.5)), 1.5 * scale, stroke, StrokeKind::Middle);
            line(&[p(5.5, 5.5), p(5.5, 3.5), p(12.5, 3.5), p(12.5, 10.5), p(10.5, 10.5)]);
        }
        Icon::Plus => {
            line(&[p(8.0, 3.5), p(8.0, 12.5)]);
            line(&[p(3.5, 8.0), p(12.5, 8.0)]);
        }
        Icon::Search => {
            painter.circle_stroke(p(7.0, 7.0), 4.0 * scale, stroke);
            line(&[p(10.0, 10.0), p(13.0, 13.0)]);
        }
        Icon::Check => line(&[p(3.5, 8.5), p(6.5, 11.5), p(12.5, 4.5)]),
        Icon::Refresh => {
            // Дуга почти по кругу и стрелка на конце.
            line(&arc(p(8.0, 8.0), 4.8 * scale, -0.15 * PI, 1.5 * PI));
            line(&[p(5.6, 1.4), p(8.0, 3.2), p(6.2, 5.6)]);
        }
        Icon::Edit => {
            line(&[p(3.5, 12.5), p(4.0, 9.8), p(10.3, 3.5), p(12.5, 5.7), p(6.2, 12.0), p(3.5, 12.5)]);
            line(&[p(9.0, 4.8), p(11.2, 7.0)]);
        }
        Icon::Trash => {
            line(&[p(3.0, 4.5), p(13.0, 4.5)]);
            line(&[p(6.3, 4.5), p(6.3, 2.8), p(9.7, 2.8), p(9.7, 4.5)]);
            line(&[p(4.3, 4.5), p(5.0, 13.2), p(11.0, 13.2), p(11.7, 4.5)]);
            line(&[p(6.8, 7.0), p(6.8, 10.8)]);
            line(&[p(9.2, 7.0), p(9.2, 10.8)]);
        }
        Icon::User => {
            painter.circle_stroke(p(8.0, 5.5), 2.6 * scale, stroke);
            line(&arc(p(8.0, 14.0), 5.0 * scale, PI * 1.08, PI * 1.92));
        }
        Icon::Key => {
            painter.circle_stroke(p(5.2, 10.8), 2.6 * scale, stroke);
            line(&[p(7.1, 8.9), p(12.8, 3.2)]);
            line(&[p(10.6, 5.4), p(12.3, 7.1)]);
            line(&[p(12.0, 4.0), p(13.4, 5.4)]);
        }
        Icon::Copy => {
            painter.rect_stroke(Rect::from_min_max(p(5.5, 5.5), p(12.5, 12.5)), 1.5 * scale, stroke, StrokeKind::Middle);
            line(&[p(3.5, 10.0), p(3.5, 3.5), p(10.0, 3.5)]);
        }
        Icon::ExternalLink => {
            line(&[p(11.5, 9.0), p(11.5, 12.5), p(3.5, 12.5), p(3.5, 4.5), p(7.0, 4.5)]);
            line(&[p(9.0, 3.5), p(12.5, 3.5), p(12.5, 7.0)]);
            line(&[p(12.5, 3.5), p(7.5, 8.5)]);
        }
        Icon::Login => {
            // Стрелка в дверь: «войти».
            line(&[p(9.0, 3.5), p(12.5, 3.5), p(12.5, 12.5), p(9.0, 12.5)]);
            line(&[p(3.0, 8.0), p(9.5, 8.0)]);
            line(&[p(7.0, 5.5), p(9.5, 8.0), p(7.0, 10.5)]);
        }
        Icon::Download => {
            line(&[p(3.0, 10.0), p(3.0, 13.0), p(13.0, 13.0), p(13.0, 10.0)]);
            line(&[p(8.0, 2.5), p(8.0, 10.0)]);
            line(&[p(5.0, 7.0), p(8.0, 10.0), p(11.0, 7.0)]);
        }
        Icon::Upload => {
            line(&[p(3.0, 10.0), p(3.0, 13.0), p(13.0, 13.0), p(13.0, 10.0)]);
            line(&[p(8.0, 10.0), p(8.0, 2.5)]);
            line(&[p(5.0, 5.5), p(8.0, 2.5), p(11.0, 5.5)]);
        }
        Icon::Warning => {
            line(&[p(8.0, 2.8), p(13.8, 12.8), p(2.2, 12.8), p(8.0, 2.8)]);
            line(&[p(8.0, 6.5), p(8.0, 9.3)]);
            painter.circle_filled(p(8.0, 11.0), 0.8 * scale, color);
        }
        Icon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![p(5.5, 4.0), p(12.0, 8.0), p(5.5, 12.0)],
                color,
                Stroke::NONE,
            ));
        }
    }
}

// ----- Вспомогательное -------------------------------------------------------

/// Раскладывает текст в одну строку (без переноса), чтобы узнать его размер заранее.
pub fn text_galley(ui: &Ui, text: RichText) -> std::sync::Arc<egui::Galley> {
    WidgetText::from(text).into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body)
}

/// Радиальное свечение: цвет `color` в центре плавно исчезает к краю круга `radius`.
/// Рисуется сеткой треугольников («веер») с цветами в вершинах — egui сам
/// плавно смешивает цвета между вершинами.
pub fn radial_glow(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    const SEGMENTS: u32 = 96;
    // Три кольца: центр → середина (45% яркости) → край (прозрачный). Так спад мягче,
    // чем один резкий переход.
    let rings = [(0.0_f32, 1.0_f32), (0.5, 0.45), (1.0, 0.0)];
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, color);
    for &(r, k) in &rings[1..] {
        for i in 0..SEGMENTS {
            let angle = TAU * i as f32 / SEGMENTS as f32;
            mesh.colored_vertex(center + vec2(angle.cos(), angle.sin()) * radius * r, color.gamma_multiply(k));
        }
    }
    let ring = |n: u32, i: u32| 1 + n * SEGMENTS + (i % SEGMENTS);
    for i in 0..SEGMENTS {
        mesh.add_triangle(0, ring(0, i), ring(0, i + 1));
        mesh.add_triangle(ring(0, i), ring(1, i), ring(1, i + 1));
        mesh.add_triangle(ring(0, i), ring(1, i + 1), ring(0, i + 1));
    }
    painter.add(egui::Shape::mesh(mesh));
}
