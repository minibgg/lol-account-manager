//! Окно «Все игры» (клавиша m): архив игр аккаунта — все, что приложение видело,
//! без ограничения количества. Вид терминала: курсор-строка, ↑ ↓ PgUp PgDn Home End,
//! Enter или щелчок — подробности игры, Tab — фильтр по очереди, Esc — закрыть.
//! Сверху — итог: сколько игр, побед и поражений, винрейт и самые частые чемпионы.

use std::collections::HashMap;

use eframe::egui::{self, Align, Layout, Margin, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::error::AppError;
use crate::riot::{MatchSummary, RiotId};
use crate::storage::AccountId;
use crate::ui::format;
use crate::ui::match_view;
use crate::ui::terminal::{self, FONT, ROW_HEIGHT, job};
use crate::ui::theme::{self, Weight, white};

/// Ширины колонок: курсор, когда, результат, чемпион, K/D/A, KDA, CS, длительность; дальше — режим.
const COLUMNS: [f32; 8] = [16.0, 118.0, 26.0, 128.0, 92.0, 62.0, 70.0, 64.0];
const HEADERS: [&str; 9] = ["", "когда", "", "чемпион", "k/d/a", "kda", "cs", "время", "режим"];
const WIDTH: f32 = 820.0;

/// Фильтр по очереди (Tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Solo,
    Flex,
    Other,
}

impl Filter {
    const ALL: [Self; 4] = [Self::All, Self::Solo, Self::Flex, Self::Other];

    fn label(self) -> &'static str {
        match self {
            Self::All => "все",
            Self::Solo => "solo/duo",
            Self::Flex => "flex",
            Self::Other => "другие",
        }
    }

    fn accepts(self, game: &MatchSummary) -> bool {
        match self {
            Self::All => true,
            Self::Solo => game.queue_id == 420,
            Self::Flex => game.queue_id == 440,
            Self::Other => game.queue_id != 420 && game.queue_id != 440,
        }
    }
}

/// Открытое окно «Все игры».
pub struct GamesView {
    pub account: AccountId,
    owner: RiotId,
    /// `None` — ещё грузится.
    pub games: Option<Result<Vec<MatchSummary>, AppError>>,
    cursor: usize,
    scroll_to_cursor: bool,
    /// Прокрутка списка (запоминаем, чтобы двигать её вслед за курсором).
    offset: f32,
    filter: Filter,
}

/// Что сделать после кадра.
pub enum GamesAction {
    None,
    Close,
    /// Открыть подробности этой игры.
    Open(MatchSummary),
}

impl GamesView {
    pub fn new(account: AccountId, owner: RiotId) -> Self {
        Self {
            account,
            owner,
            games: None,
            cursor: 0,
            scroll_to_cursor: false,
            offset: 0.0,
            filter: Filter::All,
        }
    }

    /// Игры под текущим фильтром.
    fn visible(&self) -> Vec<&MatchSummary> {
        match &self.games {
            Some(Ok(games)) => games.iter().filter(|g| self.filter.accepts(g)).collect(),
            _ => Vec::new(),
        }
    }

    /// Рисует окно. `keys` — false, если поверх открыто другое окно (подробности игры):
    /// тогда клавиши достаются ему.
    pub fn show(&mut self, ctx: &egui::Context, keys: bool) -> GamesAction {
        let mut action = GamesAction::None;
        if keys {
            action = self.handle_keys(ctx);
        }
        let height = (ctx.content_rect().height() - 230.0).max(140.0);
        let modal = egui::Modal::new(egui::Id::unique("games_view"))
            .frame(theme::dialog_frame().inner_margin(Margin::symmetric(20, 16)))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                if self.header(ui) {
                    action = GamesAction::Close;
                }
                ui.add_space(8.0);
                match &self.games {
                    None => {
                        ui.label(RichText::new("загружаю архив…").font(theme::mono(FONT)).color(theme::TEXT_MUTED));
                    }
                    Some(Err(error)) => {
                        ui.label(RichText::new(format!("! {error}")).font(theme::mono(FONT)).color(theme::RED_TEXT));
                    }
                    Some(Ok(_)) => {
                        if let Some(game) = self.list(ui, height) {
                            action = GamesAction::Open(game);
                        }
                    }
                }
            });
        if keys && modal.should_close() {
            action = GamesAction::Close;
        }
        action
    }

    fn handle_keys(&mut self, ctx: &egui::Context) -> GamesAction {
        use egui::Key;
        let none = egui::Modifiers::NONE;
        let count = self.visible().len();
        let (up, down, page_up, page_down, home, end, enter, tab) = ctx.input_mut(|i| {
            (
                i.consume_key(none, Key::ArrowUp) || i.consume_key(none, Key::K),
                i.consume_key(none, Key::ArrowDown) || i.consume_key(none, Key::J),
                i.consume_key(none, Key::PageUp),
                i.consume_key(none, Key::PageDown),
                i.consume_key(none, Key::Home),
                i.consume_key(none, Key::End),
                i.consume_key(none, Key::Enter),
                i.consume_key(none, Key::Tab),
            )
        });
        if tab {
            let index = Filter::ALL.iter().position(|f| *f == self.filter).unwrap_or(0);
            self.filter = Filter::ALL[(index + 1) % Filter::ALL.len()];
            self.cursor = 0;
            return GamesAction::None;
        }
        if count == 0 {
            return GamesAction::None;
        }
        let last = count - 1;
        let before = self.cursor;
        if up {
            self.cursor = self.cursor.saturating_sub(1);
        }
        if down {
            self.cursor = (self.cursor + 1).min(last);
        }
        if page_up {
            self.cursor = self.cursor.saturating_sub(10);
        }
        if page_down {
            self.cursor = (self.cursor + 10).min(last);
        }
        if home {
            self.cursor = 0;
        }
        if end {
            self.cursor = last;
        }
        self.cursor = self.cursor.min(last);
        self.scroll_to_cursor |= self.cursor != before;
        if enter {
            return self.visible().get(self.cursor).map_or(GamesAction::None, |g| GamesAction::Open((*g).clone()));
        }
        GamesAction::None
    }

    /// Заголовок, итог по играм и фильтр. Возвращает true — «закрыть».
    fn header(&mut self, ui: &mut egui::Ui) -> bool {
        let mut close = false;
        let font = theme::mono(FONT);
        let k = theme::TEXT_HINT;
        ui.horizontal(|ui| {
            ui.label(RichText::new("все игры").font(theme::mono(15.0)).color(theme::TEXT).semibold());
            ui.label(RichText::new(self.owner.to_string()).font(font.clone()).color(theme::TEXT_MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = terminal::key_button(ui, "esc", "закрыть", false).clicked();
            });
        });

        let visible = self.visible();
        let counted: Vec<_> = visible.iter().filter(|g| !g.remake).collect();
        let wins = counted.iter().filter(|g| g.win).count();
        let losses = counted.len() - wins;
        let mut summary = vec![
            (format!("{} игр", visible.len()), theme::TEXT),
            (" · ".into(), k),
            (format!("{wins}W {losses}L"), theme::TEXT),
        ];
        if !counted.is_empty() {
            let winrate = wins as f32 * 100.0 / counted.len() as f32;
            let color = if winrate >= 50.0 { theme::GREEN_TEXT } else { theme::RED_TEXT };
            summary.push((" · ".into(), k));
            summary.push((format!("{winrate:.0}%"), color));
        }
        let top = top_champions(&visible, 3);
        if !top.is_empty() {
            summary.push(("   чаще всего ".into(), k));
            summary.push((top, theme::TEXT_MUTED));
        }
        let parts: Vec<(&str, egui::Color32)> = summary.iter().map(|(t, c)| (t.as_str(), *c)).collect();
        ui.label(job(&parts, &font));

        // Фильтр по очереди — Tab или щелчок.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            terminal::key_button(ui, "tab", "очередь", false);
            for filter in Filter::ALL {
                let color = if filter == self.filter { theme::ACCENT_TEXT } else { theme::TEXT_MUTED };
                let label = ui
                    .add(egui::Label::new(RichText::new(filter.label()).font(font.clone()).color(color)).sense(Sense::click()))
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if label.clicked() {
                    self.filter = filter;
                    self.cursor = 0;
                }
            }
        });
        close
    }

    /// Список игр. Возвращает игру, по которой щёлкнули.
    fn list(&mut self, ui: &mut egui::Ui, height: f32) -> Option<MatchSummary> {
        let font = theme::mono(FONT);
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        let painter = ui.painter();
        let mut x = head.left();
        for (index, title) in HEADERS.iter().enumerate() {
            painter.text(pos2(x, head.center().y), egui::Align2::LEFT_CENTER, *title, font.clone(), theme::TEXT_HINT);
            x += COLUMNS.get(index).copied().unwrap_or(0.0);
        }
        painter.hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::BORDER));

        let visible: Vec<MatchSummary> = self.visible().into_iter().cloned().collect();
        if visible.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("игр нет").font(font).color(theme::TEXT_HINT));
            return None;
        }
        // Строки вплотную: show_rows считает высоту строки вместе с этим промежутком.
        ui.spacing_mut().item_spacing.y = 0.0;
        let view_height = height.min(visible.len() as f32 * ROW_HEIGHT);
        let mut area = egui::ScrollArea::vertical()
            .id_salt("games_list")
            .max_height(height)
            .min_scrolled_height(view_height)
            .auto_shrink([false, true]);
        // show_rows рисует только видимые строки, поэтому к курсору прокручиваем сами:
        // ушёл выше видимой части — она поднимается, ниже — опускается.
        if std::mem::take(&mut self.scroll_to_cursor) {
            let top = self.cursor as f32 * ROW_HEIGHT;
            if top < self.offset {
                self.offset = top;
            } else if top + ROW_HEIGHT > self.offset + view_height {
                self.offset = top + ROW_HEIGHT - view_height;
            }
            area = area.vertical_scroll_offset(self.offset);
        }
        let mut clicked = None;
        let output = area.show_rows(ui, ROW_HEIGHT, visible.len(), |ui, range| {
            for index in range {
                let game = &visible[index];
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                let is_cursor = index == self.cursor;
                if response.clicked() {
                    self.cursor = index;
                    clicked = Some(game.clone());
                }
                row(ui, rect, game, is_cursor, response.hovered());
            }
        });
        self.offset = output.state.clamped_offset().y;
        clicked
    }
}

/// Строка игры: «› 4 дн назад  W  экко  14/5/7  4.2  165  27:25  Ранговая соло/дуо».
fn row(ui: &egui::Ui, rect: Rect, game: &MatchSummary, is_cursor: bool, hovered: bool) {
    let painter = ui.painter_at(rect);
    if is_cursor {
        painter.rect_filled(rect, 0, theme::ACCENT);
    } else if hovered {
        painter.rect_filled(rect, 0, white(7));
    }
    let ink = |color| if is_cursor { theme::ON_ACCENT } else { color };
    let font = theme::mono(FONT);
    let (letter, color) = terminal::game_letter(game);
    let me = &game.me;
    let ratio = me.kda_ratio();
    let cs = if game.has_full_stats() { me.cs.to_string() } else { "—".into() };
    let cells: [(String, egui::Color32); 9] = [
        ((if is_cursor { "›" } else { "" }).into(), ink(theme::TEXT)),
        (format::relative_time(game.played_at), ink(theme::TEXT_MUTED)),
        (letter.into(), ink(color)),
        (match_view::champion_name(me).to_lowercase(), ink(theme::TEXT)),
        (game.kda(), ink(theme::TEXT)),
        (format!("{ratio:.1}"), ink(match_view::kda_color(ratio))),
        (cs, ink(theme::TEXT_MUTED)),
        (match_view::duration(game.duration_secs), ink(theme::TEXT_MUTED)),
        (game.queue_label().to_lowercase(), ink(theme::TEXT_HINT)),
    ];
    let mut x = rect.left();
    for (index, (text, color)) in cells.into_iter().enumerate() {
        let width = COLUMNS.get(index).copied().unwrap_or(rect.right() - x) - 8.0;
        let mut cell = job(&[(&text, color)], &font);
        cell.wrap = egui::text::TextWrapping::truncate_at_width(width.max(8.0));
        let galley = painter.layout_job(cell);
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
        x += COLUMNS.get(index).copied().unwrap_or(0.0);
    }
}

/// «экко 12 · сайлас 5 · ривен 4» — самые частые чемпионы.
fn top_champions(games: &[&MatchSummary], count: usize) -> String {
    let mut by_champion: HashMap<i32, (usize, String)> = HashMap::new();
    for game in games {
        let entry = by_champion
            .entry(game.me.champion_id)
            .or_insert_with(|| (0, match_view::champion_name(&game.me).to_lowercase()));
        entry.0 += 1;
    }
    let mut list: Vec<(usize, String)> = by_champion.into_values().collect();
    // Чаще — выше; при равенстве — по имени, чтобы порядок не прыгал между кадрами.
    list.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    list.into_iter()
        .take(count)
        .map(|(n, name)| format!("{name} {n}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::PlayerStats;

    fn game(champion: i32, name: &str, queue_id: i32, win: bool) -> MatchSummary {
        MatchSummary {
            id: format!("RU_{champion}{queue_id}{win}"),
            win,
            remake: false,
            queue_id,
            played_at: 0,
            duration_secs: 1800,
            stats_version: crate::riot::models::MATCH_STATS_VERSION,
            me: PlayerStats {
                champion_id: champion,
                champion: name.into(),
                ..Default::default()
            },
        }
    }

    /// Один кадр egui без окна (1100×600) с этими событиями.
    fn frame(ctx: &egui::Context, view: &mut GamesView, events: Vec<egui::Event>) -> GamesAction {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut action = GamesAction::None;
        ctx.run_ui(input, |ui| action = view.show(ui.ctx(), true))
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

    /// ↓ ведёт курсор, список прокручивается за ним, Enter открывает игру под курсором.
    #[test]
    fn keys_move_cursor_and_list_follows() {
        let ctx = egui::Context::default();
        let mut view = GamesView::new(1, RiotId::parse("Name#RU1").unwrap());
        view.games = Some(Ok((0..40).map(|i| game(i, "Экко", 420, i % 2 == 0)).collect()));
        frame(&ctx, &mut view, vec![]);
        for _ in 0..30 {
            frame(&ctx, &mut view, vec![press(egui::Key::ArrowDown)]);
        }
        assert_eq!(view.cursor, 30);
        assert!(view.offset > 0.0, "список прокрутился вслед за курсором");
        let expected = view.visible()[30].id.clone();
        match frame(&ctx, &mut view, vec![press(egui::Key::Enter)]) {
            GamesAction::Open(game) => assert_eq!(game.id, expected),
            _ => panic!("Enter не открыл игру"),
        }
        // Tab — следующий фильтр (solo/duo), курсор — в начало.
        frame(&ctx, &mut view, vec![press(egui::Key::Tab)]);
        assert_eq!((view.filter, view.cursor), (Filter::Solo, 0));
    }

    #[test]
    fn filters_and_top_champions() {
        let games = vec![
            game(245, "Экко", 420, true),
            game(245, "Экко", 420, false),
            game(517, "Сайлас", 440, true),
            game(92, "Ривен", 450, true),
            game(245, "Экко", 450, true),
        ];
        let mut view = GamesView::new(1, RiotId::parse("Name#RU1").unwrap());
        view.games = Some(Ok(games));
        assert_eq!(view.visible().len(), 5);
        view.filter = Filter::Solo;
        assert_eq!(view.visible().len(), 2);
        view.filter = Filter::Other;
        assert_eq!(view.visible().len(), 2, "ARAM — в «другие»");

        view.filter = Filter::All;
        assert_eq!(top_champions(&view.visible(), 2), "экко 3 · ривен 1");
    }
}
