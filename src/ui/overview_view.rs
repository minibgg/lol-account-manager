//! Окно «Сводка» (клавиша d): все аккаунты разом за период — сколько LP набрано,
//! побед и поражений, у кого лучший ранг и чей ранг сгорает.
//!
//! Клавиши: ← → — период, ↑ ↓ — курсор, Enter — перейти к аккаунту, Esc — закрыть.

use eframe::egui::{self, Align, Layout, Margin, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::history::{self, DAY};
use crate::overview::{self, Overview, Row};
use crate::storage::{Account, AccountId, unix_now};
use crate::ui::terminal::{self, FONT, ROW_HEIGHT, job};
use crate::ui::theme::{self, Weight, white};

/// Ширины колонок: курсор, аккаунт, ранг, изменение LP, игры; дальше — сгорание.
const COLUMNS: [f32; 5] = [16.0, 230.0, 170.0, 80.0, 90.0];
const HEADERS: [&str; 6] = ["", "аккаунт", "ранг", "lp", "игры", "сгорание"];
const WIDTH: f32 = 720.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Period {
    Day,
    Week,
    Month,
}

impl Period {
    const ALL: [Self; 3] = [Self::Day, Self::Week, Self::Month];

    fn label(self) -> &'static str {
        match self {
            Self::Day => "сутки",
            Self::Week => "7 дней",
            Self::Month => "30 дней",
        }
    }

    fn days(self) -> i64 {
        match self {
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
        }
    }
}

/// Что сделать после кадра.
#[derive(Debug, PartialEq, Eq)]
pub enum OverviewAction {
    None,
    Close,
    /// Закрыть окно и поставить курсор списка на этот аккаунт.
    Select(AccountId),
}

/// Открытое окно «Сводка».
pub struct OverviewView {
    period: Period,
    cursor: usize,
}

impl Default for OverviewView {
    fn default() -> Self {
        Self {
            period: Period::Week,
            cursor: 0,
        }
    }
}

impl OverviewView {
    pub fn show(&mut self, ctx: &egui::Context, accounts: &[Account]) -> OverviewAction {
        let now = unix_now();
        let overview = overview::build(accounts, now - self.period.days() * DAY, now);
        let mut action = self.handle_keys(ctx, &overview);

        let height = (ctx.content_rect().height() - 300.0).max(ROW_HEIGHT * 3.0);
        let modal = egui::Modal::new(egui::Id::unique("overview_view"))
            .frame(theme::dialog_frame().inner_margin(Margin::symmetric(20, 16)))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing.y = 4.0;
                let font = theme::mono(FONT);
                let k = theme::TEXT_HINT;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("сводка").font(theme::mono(15.0)).color(theme::TEXT).semibold());
                    ui.label(RichText::new(format!("аккаунтов {}", accounts.len())).font(font.clone()).color(theme::TEXT_MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if terminal::key_button(ui, "esc", "закрыть", false).clicked() {
                            action = OverviewAction::Close;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    terminal::key_button(ui, "← →", "период", false);
                    for period in Period::ALL {
                        let color = if period == self.period { theme::ACCENT_TEXT } else { theme::TEXT_MUTED };
                        let word = ui
                            .add(egui::Label::new(RichText::new(period.label()).font(font.clone()).color(color)).sense(Sense::click()))
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if word.clicked() {
                            self.period = period;
                        }
                    }
                });
                ui.add_space(4.0);
                totals(ui, &overview, accounts.len());
                ui.add_space(4.0);
                if overview.rows.is_empty() {
                    ui.label(job(&[("аккаунтов пока нет", k)], &font));
                } else if let Some(id) = self.table(ui, &overview, height) {
                    action = OverviewAction::Select(id);
                }
            });
        if modal.should_close() {
            action = OverviewAction::Close;
        }
        action
    }

    fn handle_keys(&mut self, ctx: &egui::Context, overview: &Overview) -> OverviewAction {
        use egui::Key;
        let none = egui::Modifiers::NONE;
        let (up, down, left, right, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(none, Key::ArrowUp) || i.consume_key(none, Key::K),
                i.consume_key(none, Key::ArrowDown) || i.consume_key(none, Key::J),
                i.consume_key(none, Key::ArrowLeft),
                i.consume_key(none, Key::ArrowRight),
                i.consume_key(none, Key::Enter),
            )
        });
        if left || right {
            let index = Period::ALL.iter().position(|p| *p == self.period).unwrap_or(0);
            let step = if right { 1 } else { Period::ALL.len() - 1 };
            self.period = Period::ALL[(index + step) % Period::ALL.len()];
            self.cursor = 0;
        }
        if up {
            self.cursor = self.cursor.saturating_sub(1);
        }
        if down {
            self.cursor += 1;
        }
        self.cursor = self.cursor.min(overview.rows.len().saturating_sub(1));
        match overview.rows.get(self.cursor) {
            Some(row) if enter => OverviewAction::Select(row.id),
            _ => OverviewAction::None,
        }
    }

    /// Таблица аккаунтов. Возвращает аккаунт, по строке которого щёлкнули.
    fn table(&mut self, ui: &mut egui::Ui, overview: &Overview, height: f32) -> Option<AccountId> {
        let font = theme::mono(FONT);
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        let painter = ui.painter();
        let mut x = head.left();
        for (index, title) in HEADERS.iter().enumerate() {
            painter.text(pos2(x, head.center().y), egui::Align2::LEFT_CENTER, *title, font.clone(), theme::TEXT_HINT);
            x += COLUMNS.get(index).copied().unwrap_or(0.0);
        }
        painter.hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::BORDER));

        ui.spacing_mut().item_spacing.y = 0.0;
        let view_height = height.min(overview.rows.len() as f32 * ROW_HEIGHT);
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("overview_rows")
            .max_height(height)
            .min_scrolled_height(view_height)
            .auto_shrink([false, true])
            .show_rows(ui, ROW_HEIGHT, overview.rows.len(), |ui, range| {
                for index in range {
                    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                    if response.clicked() {
                        self.cursor = index;
                        clicked = Some(overview.rows[index].id);
                    }
                    row(ui, rect, &overview.rows[index], index == self.cursor, response.hovered());
                }
            });
        clicked
    }
}

fn lp_color(lp: i32) -> egui::Color32 {
    match lp {
        0 => theme::TEXT_MUTED,
        lp if lp > 0 => theme::GREEN_TEXT,
        _ => theme::RED_TEXT,
    }
}

/// «+62», «−11» (без «LP» — оно в заголовке колонки).
fn short_lp(lp: i32) -> String {
    history::signed_lp(lp).trim_end_matches(" LP").to_string()
}

/// Итоговые строки: изменение за период, лучший ранг, архив, сгорание.
fn totals(ui: &mut egui::Ui, overview: &Overview, accounts: usize) {
    let font = theme::mono(FONT);
    let k = theme::TEXT_HINT;
    let dot = (" · ", k);
    if overview.active == 0 {
        ui.label(job(&[("за период ранги не менялись", theme::TEXT_MUTED)], &font));
    } else {
        let lp = history::signed_lp(overview.lp);
        let record = format!("{}W {}L", overview.wins, overview.losses);
        let played = format!("играли {} из {accounts}", overview.active);
        ui.label(job(
            &[
                ("итог ", k),
                (&lp, lp_color(overview.lp)),
                dot,
                (&record, theme::TEXT),
                dot,
                (&played, theme::TEXT_MUTED),
            ],
            &font,
        ));
    }
    let mut line: Vec<(String, egui::Color32)> = Vec::new();
    if let Some(best) = overview.best()
        && let Some((_, stats)) = &best.rank
    {
        line.push(("лучший ранг ".into(), k));
        line.push((terminal::rank_text(stats), theme::tier_color(Some(stats.tier))));
        line.push((format!(" — {}", best.name), theme::TEXT_MUTED));
        line.push((" · ".into(), k));
    }
    line.push((format!("в архиве {} игр", overview.archived_games), theme::TEXT_MUTED));
    let decaying = overview.decaying();
    if decaying > 0 {
        line.push((" · ".into(), k));
        line.push((format!("ранг сгорает у аккаунтов: {decaying}"), theme::AMBER_TEXT));
    }
    let parts: Vec<(&str, egui::Color32)> = line.iter().map(|(text, color)| (text.as_str(), *color)).collect();
    ui.label(job(&parts, &font));
}

/// Строка аккаунта: «› Name#TAG  алмаз II 62lp  +40  5W 3L  ↓5д».
fn row(ui: &egui::Ui, rect: Rect, row: &Row, is_cursor: bool, hovered: bool) {
    let painter = ui.painter_at(rect);
    if is_cursor {
        painter.rect_filled(rect, 0, theme::ACCENT);
    } else if hovered {
        painter.rect_filled(rect, 0, white(7));
    }
    let ink = |color| if is_cursor { theme::ON_ACCENT } else { color };
    let font = theme::mono(FONT);
    let (rank, rank_color) = match &row.rank {
        Some((_, stats)) => (terminal::rank_text(stats), theme::tier_color(Some(stats.tier))),
        None => ("без ранга".to_string(), theme::TEXT_HINT),
    };
    let (lp, lp_ink) = match row.lp {
        Some(lp) => (short_lp(lp), lp_color(lp)),
        None => ("·".to_string(), theme::TEXT_HINT),
    };
    let games = match (row.lp, row.losses) {
        (None, _) => String::new(),
        (Some(_), Some(losses)) => format!("{}W {losses}L", row.wins),
        (Some(_), None) => format!("{}W", row.wins),
    };
    let (decay, decay_color) = match &row.decay {
        Some((queue, decay)) => (
            format!("{} {}", decay.short_label(), queue.label().to_lowercase()),
            if decay.is_urgent() { theme::RED_TEXT } else { theme::AMBER_TEXT },
        ),
        None => (String::new(), theme::TEXT_HINT),
    };
    let cells: [(String, egui::Color32); 6] = [
        ((if is_cursor { "›" } else { "" }).into(), theme::TEXT),
        (row.name.clone(), theme::TEXT),
        (rank, rank_color),
        (lp, lp_ink),
        (games, theme::TEXT_MUTED),
        (decay, decay_color),
    ];
    let mut x = rect.left();
    for (index, (text, color)) in cells.into_iter().enumerate() {
        let width = COLUMNS.get(index).copied().unwrap_or(rect.right() - x) - 8.0;
        let mut cell = job(&[(&text, ink(color))], &font);
        cell.wrap = egui::text::TextWrapping::truncate_at_width(width.max(8.0));
        let galley = painter.layout_job(cell);
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
        x += COLUMNS.get(index).copied().unwrap_or(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::{Region, RiotId};

    fn account(id: AccountId, name: &str) -> Account {
        Account {
            id,
            riot_id: RiotId::parse(name).unwrap(),
            region: Region::Ru,
            puuid: None,
            lcu_puuid: None,
            note: String::new(),
            tags: vec![],
            secret_ref: String::new(),
            has_login: false,
            has_password: false,
            created_at: 0,
            cache: None,
            history: vec![],
            archived_games: 0,
            ranked_games: vec![],
        }
    }

    fn frame(ctx: &egui::Context, view: &mut OverviewView, accounts: &[Account], events: Vec<egui::Event>) -> OverviewAction {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 700.0))),
            events,
            ..Default::default()
        };
        let mut action = OverviewAction::None;
        ctx.run_ui(input, |ui| action = view.show(ui.ctx(), accounts))
            .drop_without_applying_deltas();
        action
    }

    fn press(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn keys_switch_period_and_pick_an_account() {
        let ctx = egui::Context::default();
        let mut view = OverviewView::default();
        assert_eq!(frame(&ctx, &mut view, &[], vec![]), OverviewAction::None, "пустой список рисуется");
        // Без рангов аккаунты идут по имени: A, B.
        let accounts = [account(7, "B#RU1"), account(9, "A#RU1")];
        frame(&ctx, &mut view, &accounts, vec![press(egui::Key::ArrowRight)]);
        assert_eq!(view.period, Period::Month);
        frame(&ctx, &mut view, &accounts, vec![press(egui::Key::ArrowDown)]);
        assert_eq!(frame(&ctx, &mut view, &accounts, vec![press(egui::Key::Enter)]), OverviewAction::Select(7));
        assert_eq!(frame(&ctx, &mut view, &accounts, vec![press(egui::Key::Escape)]), OverviewAction::Close);
    }
}
