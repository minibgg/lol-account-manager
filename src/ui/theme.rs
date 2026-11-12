//! Тёмная «терминальная» тема (вариант D4): почти чёрный графит, один янтарный акцент,
//! тонкие рамки. Экран аккаунтов, шапка и подвал набраны моноширинным JetBrains Mono,
//! остальные экраны пока — Inter.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText,
    Shadow, Stroke, TextStyle, Theme, Visuals, style::ScrollStyle,
};

use crate::riot::Tier;

// `pub const` — константа времени компиляции. Color32::from_rgb — const fn,
// поэтому цвета можно объявить константами и использовать из любого модуля
// как `theme::ACCENT`.

/// Фон окна.
pub const BG: Color32 = Color32::from_rgb(0x0C, 0x0D, 0x10);
/// Панели, диалоги, выпадающие списки, командная строка.
pub const CARD: Color32 = Color32::from_rgb(0x11, 0x13, 0x18);
/// Рамки панелей и разделители.
pub const BORDER: Color32 = Color32::from_rgb(0x1E, 0x22, 0x28);
/// Фон маленьких плашек поверх картинок (уровень, позиция).
pub const BADGE_BG: Color32 = Color32::from_rgb(0x0E, 0x10, 0x14);

/// Основной текст.
pub const TEXT: Color32 = Color32::from_rgb(0xD6, 0xDA, 0xE0);
/// Второстепенный текст.
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x7A, 0x81, 0x8C);
/// Подсказки, подписи колонок, «пустые» значения.
pub const TEXT_HINT: Color32 = Color32::from_rgb(0x4D, 0x54, 0x60);

/// Акцент — янтарный: курсор-строка, главные кнопки, клавиши в подсказках.
pub const ACCENT: Color32 = Color32::from_rgb(0xE8, 0xB0, 0x4B);
/// Акцент под курсором мыши.
pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(0xF2, 0xC4, 0x6C);
/// Янтарный текст на тёмном фоне.
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0xF0, 0xC3, 0x6A);
/// Текст поверх акцента (на курсор-строке и главных кнопках).
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x15, 0x11, 0x0A);
pub const GREEN: Color32 = Color32::from_rgb(0x4F, 0xB8, 0x6A);
pub const GREEN_TEXT: Color32 = Color32::from_rgb(0x7B, 0xD8, 0x8F);
pub const AMBER: Color32 = Color32::from_rgb(0xE8, 0x91, 0x3A);
pub const AMBER_TEXT: Color32 = Color32::from_rgb(0xF0, 0xA8, 0x5C);
pub const RED: Color32 = Color32::from_rgb(0xE5, 0x53, 0x4B);
pub const RED_TEXT: Color32 = Color32::from_rgb(0xF2, 0x77, 0x7A);

/// Моноширинный шрифт нужного размера (JetBrains Mono).
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Толщина шрифта (ось `wght` у Inter и JetBrains Mono): обычный, средний, полужирный.
pub const REGULAR: f32 = 400.0;
pub const MEDIUM: f32 = 500.0;
pub const SEMIBOLD: f32 = 600.0;

/// Цвет с прозрачностью: `alpha(ACCENT, 26)` — синий на 10% (26 из 255).
/// `const fn` — можно вызывать и при объявлении констант.
pub const fn alpha(color: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(color.r(), color.g(), color.b(), a)
}

/// Белый с прозрачностью — для еле заметных рамок и подложек, как `white/10` в reveal.
pub const fn white(a: u8) -> Color32 {
    // Цвета в egui хранятся «premultiplied»: у белого все каналы равны прозрачности.
    Color32::from_rgba_premultiplied(a, a, a, a)
}

/// Применяет тему к egui. Вызывается один раз при старте.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    // Всегда тёмная тема, даже если в Windows включена светлая.
    ctx.set_theme(Theme::Dark);

    // style_mut_of принимает замыкание `|style| { ... }` и даёт ему `&mut Style`.
    // Так egui сам решает, когда и как безопасно менять общий стиль.
    ctx.style_mut_of(Theme::Dark, |style| {
        style.visuals = visuals();
        // Моноширинный шрифт шире обычного — размеры чуть меньше, чем были у Inter.
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(12.5)),
            (TextStyle::Button, FontId::proportional(12.5)),
            (TextStyle::Heading, FontId::proportional(15.0)),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();
        let spacing = &mut style.spacing;
        spacing.item_spacing = egui::vec2(8.0, 6.0);
        // Поля ввода и выпадающие списки получаются ~32 px — как наши кнопки.
        // (interact_size.y не трогаем: это ещё и минимальная высота каждой строки
        // `ui.horizontal`, и с большим значением строки списка раздуваются.)
        spacing.button_padding = egui::vec2(10.0, 8.0);
        spacing.menu_margin = Margin::same(6);
        spacing.scroll = scroll_style();
        // Текст надписей не выделяется мышью — как в обычном приложении, а не на веб-странице.
        style.interaction.selectable_labels = false;
    });
}

/// Шрифты вшиты в программу (лицензия OFL, файлы в assets/fonts):
/// - JetBrains Mono — основной везде, как в терминале: кириллица, символы рамок (█ ░ ›);
/// - Inter — запасной: символы, которых нет в JetBrains Mono, egui ищет дальше по списку.
///
/// Оба «вариативные»: одна и та же буква бывает разной толщины (ось `wght`),
/// толщину выбирает `RichText::variation` — см. [`Weight`].
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "JetBrainsMono".to_owned(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/JetBrainsMono.ttf"))),
    );
    fonts.font_data.insert(
        "Inter".to_owned(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Inter.ttf"))),
    );
    // В обоих семействах первым — JetBrains Mono, за ним Inter, потом стандартные шрифты egui.
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        list.insert(0, "Inter".to_owned());
        list.insert(0, "JetBrainsMono".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// Толщина текста: `RichText::new("…").semibold()`.
///
/// Это «трейт-расширение»: добавляем свои методы чужому типу `RichText`.
/// Чтобы они были видны, модуль импортирует `theme::Weight`.
pub trait Weight {
    fn medium(self) -> Self;
    fn semibold(self) -> Self;
}

impl Weight for RichText {
    fn medium(self) -> Self {
        self.variation(b"wght", MEDIUM)
    }
    fn semibold(self) -> Self {
        self.variation(b"wght", SEMIBOLD)
    }
}

/// Подпись над полем или блоком в стиле терминала: «solo/duo», «папка игры» — строчными,
/// приглушённо, как подписи колонок таблицы.
pub fn caps(text: &str) -> RichText {
    RichText::new(text.to_lowercase()).size(11.5).color(TEXT_HINT)
}

/// Панель: тёмная подложка, тонкая рамка, почти прямые углы — как окно терминала.
pub fn panel() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(RADIUS)
}

/// Скругление углов панелей и окон: еле заметное.
pub const RADIUS: u8 = 3;

/// Рамка модальных окон (форма аккаунта, подтверждение удаления).
pub fn dialog_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(RADIUS)
        .inner_margin(Margin::same(20))
        .shadow(Shadow {
            offset: [0, 16],
            blur: 48,
            spread: 0,
            color: Color32::from_black_alpha(150),
        })
}

/// Цвет названия ранга — примерно как цвет его эмблемы. `None` — без ранга.
pub fn tier_color(tier: Option<Tier>) -> Color32 {
    match tier {
        None => TEXT_MUTED,
        Some(Tier::Iron) => Color32::from_rgb(0x9A, 0x90, 0x8A),
        Some(Tier::Bronze) => Color32::from_rgb(0xC8, 0x8F, 0x72),
        Some(Tier::Silver) => Color32::from_rgb(0xB8, 0xC5, 0xCD),
        Some(Tier::Gold) => Color32::from_rgb(0xE8, 0xC0, 0x6A),
        Some(Tier::Platinum) => Color32::from_rgb(0x5E, 0xD2, 0xC8),
        Some(Tier::Emerald) => Color32::from_rgb(0x3E, 0xD2, 0x8A),
        Some(Tier::Diamond) => Color32::from_rgb(0x8A, 0xAC, 0xFF),
        Some(Tier::Master) => Color32::from_rgb(0xC4, 0x7E, 0xF0),
        Some(Tier::Grandmaster) => Color32::from_rgb(0xF8, 0x71, 0x71),
        Some(Tier::Challenger) => Color32::from_rgb(0xF5, 0xD5, 0x80),
    }
}

/// Тонкая полоса прокрутки (6 px), как в reveal: без подложки, ручка проявляется
/// при наведении на список.
fn scroll_style() -> ScrollStyle {
    ScrollStyle {
        floating: true,
        bar_width: 6.0,
        floating_width: 6.0,
        floating_allocated_width: 8.0,
        bar_inner_margin: 2.0,
        handle_min_length: 24.0,
        foreground_color: false,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: 0.0,
        active_handle_opacity: 0.7,
        interact_handle_opacity: 1.0,
        ..ScrollStyle::solid()
    }
}

fn visuals() -> Visuals {
    // Берём стандартную тёмную тему и переопределяем цвета.
    // `let mut` — переменная изменяемая; по умолчанию в Rust всё неизменяемое.
    let mut v = Visuals::dark();

    let radius = CornerRadius::same(RADIUS);

    v.panel_fill = BG;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.menu_corner_radius = CornerRadius::same(RADIUS);
    v.window_shadow = Shadow {
        offset: [0, 12],
        blur: 40,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    v.popup_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    v.extreme_bg_color = BG;
    // Поля ввода: еле заметная светлая подложка (white/3.5% в reveal).
    v.text_edit_bg_color = Some(white(9));
    v.faint_bg_color = white(5);
    v.code_bg_color = CARD;
    v.weak_text_color = Some(TEXT_HINT);
    v.hyperlink_color = ACCENT_LIGHT;
    v.error_fg_color = RED_TEXT;
    v.warn_fg_color = AMBER_TEXT;
    // Выделенный текст и рамка поля в фокусе — янтарные.
    v.selection.bg_fill = alpha(ACCENT, 70);
    v.selection.stroke = Stroke::new(1.0, ACCENT_LIGHT);
    v.text_cursor.stroke = Stroke::new(2.0, ACCENT);

    // `&mut v.widgets` — изменяемая ссылка на поле, чтобы не писать `v.widgets.` каждый раз.
    // weak_bg_fill — фон кнопок и выпадающих списков; bg_fill — ручка прокрутки, галочки.
    let w = &mut v.widgets;

    // noninteractive: обычный текст, разделители.
    w.noninteractive.bg_fill = CARD;
    w.noninteractive.weak_bg_fill = CARD;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = radius;

    // inactive: кнопки и поля в покое.
    w.inactive.bg_fill = alpha(TEXT_MUTED, 60);
    w.inactive.weak_bg_fill = white(9);
    w.inactive.bg_stroke = Stroke::new(1.0, white(26));
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = radius;
    w.inactive.expansion = 0.0;

    // hovered: под курсором.
    w.hovered.bg_fill = alpha(TEXT_MUTED, 110);
    w.hovered.weak_bg_fill = white(18);
    w.hovered.bg_stroke = Stroke::new(1.0, white(40));
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    // active: в момент нажатия (и поле ввода в фокусе).
    w.active.bg_fill = alpha(TEXT_MUTED, 150);
    w.active.weak_bg_fill = white(26);
    w.active.bg_stroke = Stroke::new(1.0, alpha(ACCENT_LIGHT, 140));
    w.active.fg_stroke = Stroke::new(1.0, TEXT);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    // open: раскрытый выпадающий список.
    w.open.bg_fill = alpha(TEXT_MUTED, 110);
    w.open.weak_bg_fill = white(18);
    w.open.bg_stroke = Stroke::new(1.0, white(40));
    w.open.fg_stroke = Stroke::new(1.0, TEXT);
    w.open.corner_radius = radius;
    w.open.expansion = 0.0;

    // Последнее выражение без `;` — это возвращаемое значение функции.
    v
}
