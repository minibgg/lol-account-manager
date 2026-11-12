//! Окно без системной рамки (как у reveal): свой фон со свечением, своя шапка,
//! кнопки «свернуть / развернуть / закрыть» и изменение размера за края окна.
//!
//! Системная рамка выключена в main.rs (`with_decorations(false)`). Перетаскивание,
//! «прилипание» к краям экрана и изменение размера всё равно делает Windows:
//! мы только говорим ей, когда начать (`StartDrag`, `BeginResize`).

use eframe::egui::{
    self, CursorIcon, Id, LayerId, Order, Rect, Sense, Stroke, StrokeKind, Ui, ViewportCommand,
    viewport::ResizeDirection,
};

use crate::ui::theme::{self, white};
use crate::ui::widgets::{Icon, IconButton};

/// Насколько близко к краю окна (в точках) курсор превращается в «изменить размер».
const EDGE: f32 = 5.0;
/// В углах зона шире — так легче попасть.
const CORNER: f32 = 14.0;

/// Фон всего окна — ровный графит, как у терминала. Рисуется первым, поэтому панели
/// (прозрачные) оказываются поверх.
pub fn background(ui: &Ui) {
    ui.painter().rect_filled(ui.ctx().viewport_rect(), 0, theme::BG);
}

/// Шапка как заголовок окна: тянуть — двигать окно, двойной щелчок — развернуть.
/// Вызывать ДО кнопок шапки: в egui нажатие получает виджет, добавленный последним,
/// поэтому кнопки поверх этой области остаются нажимаемыми.
pub fn drag_area(ui: &mut Ui, rect: Rect) {
    let response = ui.interact(rect, Id::unique("title_bar"), Sense::click_and_drag());
    if response.double_clicked() {
        toggle_maximized(ui.ctx());
    } else if response.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// Кнопки окна справа в шапке. Вызывать внутри раскладки справа налево:
/// первой добавляется «закрыть» — она окажется правее всех.
pub fn window_controls(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    ui.spacing_mut().item_spacing.x = 2.0;
    if ui.add(IconButton::new(Icon::Close).danger()).on_hover_text("Закрыть").clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let (icon, hint) = if is_maximized(&ctx) {
        (Icon::Restore, "Свернуть в окно")
    } else {
        (Icon::Maximize, "Развернуть")
    };
    if ui.add(IconButton::new(icon)).on_hover_text(hint).clicked() {
        toggle_maximized(&ctx);
    }
    if ui.add(IconButton::new(Icon::Minimize)).on_hover_text("Свернуть").clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

/// Изменение размера за края окна. Вызывать в КОНЦЕ кадра: тогда наш курсор-стрелка
/// не перебивается курсорами виджетов под ним.
pub fn resize_edges(ctx: &egui::Context) {
    if is_maximized(ctx) {
        return;
    }
    let rect = ctx.viewport_rect();
    // `let Some(x) = ... else { return }` — достать значение или выйти из функции.
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let (left, right) = (pos.x - rect.left(), rect.right() - pos.x);
    let (top, bottom) = (pos.y - rect.top(), rect.bottom() - pos.y);
    let near = |d: f32, zone: f32| (0.0..zone).contains(&d);

    use ResizeDirection as D;
    let direction = if (near(left, EDGE) && near(top, CORNER)) || (near(top, EDGE) && near(left, CORNER)) {
        Some(D::NorthWest)
    } else if (near(right, EDGE) && near(top, CORNER)) || (near(top, EDGE) && near(right, CORNER)) {
        Some(D::NorthEast)
    } else if (near(left, EDGE) && near(bottom, CORNER)) || (near(bottom, EDGE) && near(left, CORNER)) {
        Some(D::SouthWest)
    } else if (near(right, EDGE) && near(bottom, CORNER)) || (near(bottom, EDGE) && near(right, CORNER)) {
        Some(D::SouthEast)
    } else if near(left, EDGE) {
        Some(D::West)
    } else if near(right, EDGE) {
        Some(D::East)
    } else if near(top, EDGE) {
        Some(D::North)
    } else if near(bottom, EDGE) {
        Some(D::South)
    } else {
        None
    };

    if let Some(direction) = direction {
        ctx.set_cursor_icon(match direction {
            D::West | D::East => CursorIcon::ResizeHorizontal,
            D::North | D::South => CursorIcon::ResizeVertical,
            D::NorthWest | D::SouthEast => CursorIcon::ResizeNwSe,
            D::NorthEast | D::SouthWest => CursorIcon::ResizeNeSw,
        });
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}

/// Тонкая светлая рамка по краю окна (border-white/10), чтобы окно не сливалось
/// с тёмным рабочим столом. В развёрнутом окне не нужна.
pub fn border(ctx: &egui::Context) {
    if is_maximized(ctx) {
        return;
    }
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("window_border")));
    painter.rect_stroke(ctx.viewport_rect(), 0, Stroke::new(1.0, white(22)), StrokeKind::Inside);
}

fn is_maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

fn toggle_maximized(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!is_maximized(ctx)));
}
