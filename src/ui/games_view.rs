//! Окно «Все игры» (клавиша m): архив игр аккаунта — все, что приложение видело,
//! без ограничения количества. Вид терминала: курсор-строка, ↑ ↓ PgUp PgDn Home End,
//! Enter или щелчок — подробности игры, Tab — фильтр по очереди, `/` — поиск, Esc — закрыть.
//! `h` — загрузить в архив всю историю игр через Riot API (обычное обновление берёт
//! только последние 20), ещё раз `h` — остановить.
//! Сверху — итог: сколько игр, побед и поражений, винрейт и самые частые чемпионы.
//! У ранговых игр — ранг аккаунта после игры и сколько LP она принесла (по истории рангов).
//!
//! Поиск — по нику любого игрока в игре и по чемпиону (своему, союзника или соперника):
//! «Друг#RU1» — все игры с ним, «зед» — все игры с Зедом. Итог сверху считается по найденным,
//! поэтому сразу видно, например, винрейт вместе с другом.

use std::collections::HashMap;

use eframe::egui::{self, Align, Layout, Margin, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::error::AppError;
use crate::history::{self, GameRank, Queue, RankPoint};
use crate::messages::HistoryProgress;
use crate::riot::{MatchSummary, RiotId};
use crate::storage::{AccountId, ArchivedPlayer};
use crate::ui::command::swap_layout;
use crate::ui::format;
use crate::ui::match_view;
use crate::ui::terminal::{self, FONT, ROW_HEIGHT, job};
use crate::ui::theme::{self, Weight, white};

/// Ширины колонок: курсор, когда, результат, чемпион, K/D/A, KDA, CS, длительность,
/// ранг после игры; дальше — режим.
const COLUMNS: [f32; 9] = [16.0, 104.0, 26.0, 118.0, 88.0, 50.0, 48.0, 60.0, 190.0];
const HEADERS: [&str; 10] = ["", "когда", "", "чемпион", "k/d/a", "kda", "cs", "время", "ранг после игры", "режим"];
const WIDTH: f32 = 820.0;

/// Поле поиска — по этому id проверяем, идёт ли в нём ввод.
fn search_id() -> egui::Id {
    egui::Id::unique("games_search")
}

/// Подходит ли игра под поиск: запрос (без учёта регистра, в любой раскладке) — часть
/// своего чемпиона, ника или чемпиона любого другого игрока. `players` — другие игроки
/// этой игры; `None` — подробностей нет, ищем только по своему чемпиону.
pub fn search_matches(game: &MatchSummary, players: Option<&[ArchivedPlayer]>, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let swapped = swap_layout(&query);
    let hit = |text: &str| {
        let text = text.to_lowercase();
        text.contains(&query) || text.contains(&swapped)
    };
    hit(&match_view::champion_name(&game.me))
        || players.into_iter().flatten().any(|p| {
            hit(&p.champion) || p.riot_id.as_ref().is_some_and(|id| hit(&id.to_string()))
        })
}

/// Фильтр по очереди (Tab). Общий с окном «Профиль».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Solo,
    Flex,
    Other,
}

impl Filter {
    pub const ALL: [Self; 4] = [Self::All, Self::Solo, Self::Flex, Self::Other];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "все",
            Self::Solo => "solo/duo 5×5",
            Self::Flex => "flex 5×5",
            Self::Other => "другие",
        }
    }

    pub fn accepts(self, game: &MatchSummary) -> bool {
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
    /// Другие игроки в играх — для поиска: номер игры → игроки. `None` — ещё грузятся.
    pub players: Option<Result<HashMap<String, Vec<ArchivedPlayer>>, AppError>>,
    /// Поиск по нику и чемпиону.
    query: String,
    /// Поставить фокус в поле поиска в этом кадре (`/`).
    focus_search: bool,
    cursor: usize,
    scroll_to_cursor: bool,
    /// Прокрутка списка (запоминаем, чтобы двигать её вслед за курсором).
    offset: f32,
    filter: Filter,
    /// Ранги после игр — и для каких размеров архива и истории они посчитаны
    /// (пересчитываем, только когда что-то добавилось, а не каждый кадр).
    ranks: HashMap<String, GameRank>,
    ranks_for: (usize, usize),
}

/// Что сделать после кадра.
pub enum GamesAction {
    None,
    Close,
    /// Открыть подробности этой игры.
    Open(MatchSummary),
    /// Загрузить всю историю игр аккаунта (`h`).
    LoadHistory,
    /// Остановить загрузку истории (`h`, пока она идёт).
    StopHistory,
}

/// Загрузка всей истории аккаунта — для строки в окне «Все игры».
#[derive(Debug, Clone, Default)]
pub struct HistoryLoad {
    pub progress: HistoryProgress,
    pub running: bool,
    /// Чем кончилась прошлая загрузка, если ошибкой.
    pub error: Option<AppError>,
}

impl HistoryLoad {
    /// Что сделать клавишей `h`: идёт — остановить, иначе — загрузить (или продолжить).
    fn toggle(load: Option<&Self>) -> GamesAction {
        if load.is_some_and(|l| l.running) { GamesAction::StopHistory } else { GamesAction::LoadHistory }
    }

    /// Строка о загрузке: текст и его цвет.
    fn status(load: Option<&Self>) -> (String, egui::Color32) {
        let Some(load) = load else {
            return ("загрузить всю историю игр из Riot API — для статистики, напарников и поиска".into(), theme::TEXT_HINT);
        };
        let HistoryProgress { scanned, saved, stopped } = load.progress;
        if load.running {
            let text = format!("загружаю историю… +{saved} игр, просмотрено {scanned} — можно закрыть окно");
            return (text, theme::ACCENT_TEXT);
        }
        match &load.error {
            Some(error) => (format!("! {error} — загружено +{saved}, h — продолжить"), theme::RED_TEXT),
            None if stopped => (format!("остановлено: +{saved} игр, h — продолжить с того же места"), theme::TEXT_MUTED),
            None => (format!("история загружена: +{saved} новых игр, всего просмотрено {scanned}"), theme::GREEN_TEXT),
        }
    }
}

impl GamesView {
    pub fn new(account: AccountId, owner: RiotId) -> Self {
        Self {
            account,
            owner,
            games: None,
            players: None,
            query: String::new(),
            focus_search: false,
            cursor: 0,
            scroll_to_cursor: false,
            offset: 0.0,
            filter: Filter::All,
            ranks: HashMap::new(),
            ranks_for: (0, 0),
        }
    }

    /// Игры под текущими фильтром и поиском.
    fn visible(&self) -> Vec<&MatchSummary> {
        let players = match &self.players {
            Some(Ok(players)) => Some(players),
            _ => None,
        };
        match &self.games {
            Some(Ok(games)) => games
                .iter()
                .filter(|g| self.filter.accepts(g))
                .filter(|g| {
                    let in_game = players.and_then(|p| p.get(&g.id)).map(Vec::as_slice);
                    search_matches(g, in_game, &self.query)
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Рисует окно. `keys` — false, если поверх открыто другое окно (подробности игры):
    /// тогда клавиши достаются ему. `history` — история рангов аккаунта.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        keys: bool,
        history: &[RankPoint],
        load: Option<&HistoryLoad>,
    ) -> GamesAction {
        let mut action = GamesAction::None;
        if let Some(Ok(games)) = &self.games
            && self.ranks_for != (games.len(), history.len())
        {
            self.ranks = history::game_ranks(history, games);
            self.ranks_for = (games.len(), history.len());
        }
        if keys {
            action = self.handle_keys(ctx, load);
        }
        let height = (ctx.content_rect().height() - 230.0).max(140.0);
        let modal = egui::Modal::new(egui::Id::unique("games_view"))
            .frame(theme::dialog_frame().inner_margin(Margin::symmetric(20, 16)))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                if let Some(clicked) = self.header(ui, load) {
                    action = clicked;
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

    fn handle_keys(&mut self, ctx: &egui::Context, load: Option<&HistoryLoad>) -> GamesAction {
        use egui::Key;
        let none = egui::Modifiers::NONE;
        // Идёт ввод в поиске: буквы (j k h) и Home End — поля ввода, а не списка.
        let typing = ctx.memory(|m| m.has_focus(search_id()));
        let (up, down, page_up, page_down, home, end, enter, tab, find, escape, history) = ctx.input_mut(|i| {
            (
                i.consume_key(none, Key::ArrowUp) || (!typing && i.consume_key(none, Key::K)),
                i.consume_key(none, Key::ArrowDown) || (!typing && i.consume_key(none, Key::J)),
                i.consume_key(none, Key::PageUp),
                i.consume_key(none, Key::PageDown),
                !typing && i.consume_key(none, Key::Home),
                !typing && i.consume_key(none, Key::End),
                i.consume_key(none, Key::Enter),
                i.consume_key(none, Key::Tab),
                // «/» ловим как текст: в русской раскладке он на другой клавише.
                i.consume_key(egui::Modifiers::COMMAND, Key::F) || (!typing && consume_text(i, "/")),
                // Esc при непустом поиске — очистить поиск, а не закрыть окно.
                (typing || !self.query.is_empty()) && i.consume_key(none, Key::Escape),
                !typing && i.consume_key(none, Key::H),
            )
        });
        if history {
            return HistoryLoad::toggle(load);
        }
        if find {
            self.focus_search = true;
        }
        if escape {
            self.query.clear();
            ctx.memory_mut(|m| m.surrender_focus(search_id()));
            self.cursor = 0;
            self.scroll_to_cursor = true;
        }
        let count = self.visible().len();
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

    /// Заголовок, итог по играм, фильтр, поиск и загрузка истории.
    /// Возвращает, что нажато мышью: «закрыть» или «загрузить / остановить историю».
    fn header(&mut self, ui: &mut egui::Ui, load: Option<&HistoryLoad>) -> Option<GamesAction> {
        let mut clicked = None;
        let font = theme::mono(FONT);
        let k = theme::TEXT_HINT;
        ui.horizontal(|ui| {
            ui.label(RichText::new("все игры").font(theme::mono(15.0)).color(theme::TEXT).semibold());
            ui.label(RichText::new(self.owner.to_string()).font(font.clone()).color(theme::TEXT_MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if terminal::key_button(ui, "esc", "закрыть", false).clicked() {
                    clicked = Some(GamesAction::Close);
                }
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
        self.search_field(ui);

        // Загрузка всей истории — h или щелчок.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let running = load.is_some_and(|l| l.running);
            let label = if running { "стоп" } else { "вся история" };
            if terminal::key_button(ui, "h", label, false).clicked() {
                clicked = Some(HistoryLoad::toggle(load));
            }
            let (text, color) = HistoryLoad::status(load);
            ui.label(RichText::new(text).font(theme::mono(11.5)).color(color));
            if running {
                // Перерисовать, когда придут новые игры (их присылает фоновый поток).
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            }
        });
        clicked
    }

    /// Поле поиска: «/ ник или чемпион…». Новый текст — курсор в начало списка.
    fn search_field(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("/").font(theme::mono(FONT)).color(theme::ACCENT_TEXT));
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .id(search_id())
                    .font(theme::mono(FONT))
                    .text_color(theme::TEXT)
                    .frame(egui::Frame::NONE)
                    .hint_text(RichText::new("ник игрока или чемпион: друг#ru1, зед…").color(theme::TEXT_HINT))
                    .desired_width(300.0),
            );
            if std::mem::take(&mut self.focus_search) {
                response.request_focus();
            }
            if response.changed() {
                self.cursor = 0;
                self.scroll_to_cursor = true;
            }
            terminal::focus_outline(ui, response.rect.expand2(vec2(6.0, 3.0)));
            // Игроков ещё нет или подробности есть не у всех игр — подскажем.
            if !self.query.trim().is_empty() {
                let note = match &self.players {
                    None => "ищу пока только по своим чемпионам — загружаю игроков…".to_string(),
                    Some(Err(error)) => format!("! игроки не загрузились: {error}"),
                    Some(Ok(_)) => "esc — сбросить".to_string(),
                };
                ui.label(RichText::new(note).font(theme::mono(11.5)).color(theme::TEXT_HINT));
            }
        });
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
            let text = if self.query.trim().is_empty() {
                "игр нет"
            } else {
                "ничего не нашлось — ищутся ники и чемпионы игр, у которых загружены подробности"
            };
            ui.label(RichText::new(text).font(font).color(theme::TEXT_HINT));
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
                row(ui, rect, game, self.ranks.get(&game.id), is_cursor, response.hovered());
            }
        });
        self.offset = output.state.clamped_offset().y;
        clicked
    }
}

/// Забрать из ввода кадра набранный текст `text` (чтобы его не получило поле ввода).
fn consume_text(input: &mut egui::InputState, text: &str) -> bool {
    let before = input.events.len();
    input.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == text));
    input.events.len() != before
}

/// Ячейка «ранг после игры»: «платина I 40lp −18». «+46/2» — изменение сразу за две игры
/// (приложение было закрыто между ними). У неранговых игр ячейка пустая; «—» — ранговая
/// игра, для которой в истории рангов записи нет.
fn rank_cell(game: &MatchSummary, rank: Option<&GameRank>) -> Vec<(String, egui::Color32)> {
    let Some(rank) = rank else {
        let ranked = Queue::from_queue_id(game.queue_id).is_some() && !game.remake;
        return vec![((if ranked { "—" } else { "" }).into(), theme::TEXT_HINT)];
    };
    let lp = history::signed_lp(rank.lp);
    let mut change = format!(" {}", lp.trim_end_matches(" LP"));
    if rank.games > 1 {
        change.push_str(&format!("/{}", rank.games));
    }
    let color = match rank.lp {
        0 => theme::TEXT_MUTED,
        lp if lp > 0 => theme::GREEN_TEXT,
        _ => theme::RED_TEXT,
    };
    vec![
        (terminal::rank_text(&rank.after), theme::tier_color(Some(rank.after.tier))),
        (change, color),
    ]
}

/// Строка игры: «› 4 дн назад  W  экко  14/5/7  4.2  165  27:25  платина I 40lp −18  ранговая…».
fn row(ui: &egui::Ui, rect: Rect, game: &MatchSummary, rank: Option<&GameRank>, is_cursor: bool, hovered: bool) {
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
    // Каждая ячейка — куски текста своего цвета (в ячейке ранга их два).
    let one = |text: String, color| vec![(text, color)];
    let cells: [Vec<(String, egui::Color32)>; 10] = [
        one((if is_cursor { "›" } else { "" }).into(), theme::TEXT),
        one(format::relative_time(game.played_at), theme::TEXT_MUTED),
        one(letter.into(), color),
        one(match_view::champion_name(me).to_lowercase(), theme::TEXT),
        one(game.kda(), theme::TEXT),
        one(format!("{ratio:.1}"), match_view::kda_color(ratio)),
        one(cs, theme::TEXT_MUTED),
        one(match_view::duration(game.duration_secs), theme::TEXT_MUTED),
        rank_cell(game, rank),
        one(game.queue_label().to_lowercase(), theme::TEXT_HINT),
    ];
    let mut x = rect.left();
    for (index, parts) in cells.into_iter().enumerate() {
        let width = COLUMNS.get(index).copied().unwrap_or(rect.right() - x) - 8.0;
        let parts: Vec<(&str, egui::Color32)> = parts.iter().map(|(text, color)| (text.as_str(), ink(*color))).collect();
        let mut cell = job(&parts, &font);
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
        ctx.run_ui(input, |ui| action = view.show(ui.ctx(), true, &[], None))
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

    /// Поиск: по своему чемпиону, нику (часть, без учёта регистра) и чемпиону другого
    /// игрока; в другой раскладке; игры без подробностей — только по своему чемпиону.
    #[test]
    fn search_finds_players_and_champions() {
        let player = |name: Option<&str>, champion: &str, ally: bool| ArchivedPlayer {
            riot_id: name.map(|n| RiotId::parse(n).unwrap()),
            champion: champion.into(),
            ally,
        };
        let ekko = game(245, "Экко", 420, true);
        let players = [player(Some("Друг#RU1"), "Ари", true), player(Some("Foe#EUW"), "Зед", false), player(None, "Лукс", false)];
        let found = |query| search_matches(&ekko, Some(&players), query);
        assert!(found(""));
        assert!(found("экк"), "свой чемпион");
        assert!(found("ДРУГ"), "ник без учёта регистра");
        assert!(found("друг#ru1"));
        assert!(found("зед") && found("лукс"), "чемпион соперника — даже со скрытым ником");
        assert!(found("pt"), "«зе» в английской раскладке");
        assert!(!found("сайлас"));
        assert!(!search_matches(&ekko, None, "зед"), "без подробностей — только свой чемпион");

        // В окне: игры с другом; итог считается по найденным.
        let mut view = GamesView::new(1, RiotId::parse("Name#RU1").unwrap());
        let other = game(517, "Сайлас", 420, false);
        view.games = Some(Ok(vec![ekko.clone(), other.clone()]));
        view.players = Some(Ok([(ekko.id.clone(), players.to_vec()), (other.id.clone(), vec![])].into_iter().collect()));
        view.query = "друг".into();
        assert_eq!(view.visible().len(), 1);
        view.query = "сай".into();
        assert_eq!(view.visible()[0].id, other.id);
    }

    /// «/» ставит фокус в поиск, Esc сначала очищает поиск и только потом закрывает окно.
    #[test]
    fn slash_focuses_search_and_escape_clears_it() {
        let ctx = egui::Context::default();
        let mut view = GamesView::new(1, RiotId::parse("Name#RU1").unwrap());
        view.games = Some(Ok(vec![game(245, "Экко", 420, true)]));
        frame(&ctx, &mut view, vec![egui::Event::Text("/".into())]);
        frame(&ctx, &mut view, vec![]);
        assert!(ctx.memory(|m| m.has_focus(search_id())), "/ — фокус в поиске");
        assert!(view.query.is_empty(), "сам «/» в поле не попал");
        frame(&ctx, &mut view, vec![egui::Event::Text("j".into())]);
        assert_eq!(view.query, "j", "в поиске j — это буква, а не «вниз»");
        assert!(matches!(frame(&ctx, &mut view, vec![press(egui::Key::Escape)]), GamesAction::None));
        assert!(view.query.is_empty(), "Esc очистил поиск, окно открыто");
        assert!(matches!(frame(&ctx, &mut view, vec![press(egui::Key::Escape)]), GamesAction::Close));
    }

    /// h — загрузить всю историю, пока идёт — остановить; при вводе в поиске h — буква.
    #[test]
    fn h_starts_and_stops_history() {
        let ctx = egui::Context::default();
        let mut view = GamesView::new(1, RiotId::parse("Name#RU1").unwrap());
        view.games = Some(Ok(vec![game(245, "Экко", 420, true)]));
        let show = |view: &mut GamesView, load: Option<&HistoryLoad>, events| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut action = GamesAction::None;
            ctx.run_ui(input, |ui| action = view.show(ui.ctx(), true, &[], load)).drop_without_applying_deltas();
            action
        };
        assert!(matches!(show(&mut view, None, vec![press(egui::Key::H)]), GamesAction::LoadHistory));
        let running = HistoryLoad { running: true, ..Default::default() };
        assert!(matches!(show(&mut view, Some(&running), vec![press(egui::Key::H)]), GamesAction::StopHistory));
        let stopped = HistoryLoad::default();
        assert!(matches!(show(&mut view, Some(&stopped), vec![press(egui::Key::H)]), GamesAction::LoadHistory));

        show(&mut view, None, vec![egui::Event::Text("/".into())]);
        show(&mut view, None, vec![]);
        assert!(matches!(show(&mut view, None, vec![press(egui::Key::H)]), GamesAction::None), "в поиске h — не команда");
    }
}
