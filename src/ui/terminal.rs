//! Экран «Аккаунты» в стиле терминала (вариант D4): таблица с курсором-строкой,
//! панель выбранного аккаунта, строка подсказок клавиш, командная строка и справка.
//!
//! Всё набрано моноширинным шрифтом. Таблица рисуется вручную (painter): так строка
//! под курсором целиком перекрашивается в янтарный, а колонки стоят ровно.
//! Сами действия выполняет окно (app.rs): здесь только рисуем и возвращаем, что нажали.

use std::collections::{HashMap, HashSet};

use eframe::egui::{
    self, Align, Color32, CursorIcon, FontId, Layout, Margin, Rect, RichText, Sense, Stroke,
    StrokeKind, TextFormat, Ui, pos2, text::LayoutJob, vec2,
};

use crate::error::AppError;
use crate::history::{self, DAY};
use crate::riot::{MatchSummary, RankedStats};
use crate::storage::{Account, AccountId, unix_now};
use crate::ui::command::{Invocation, Suggestion};
use crate::ui::format;
use crate::ui::keymap::{self, Action};
use crate::ui::match_view;
use crate::ui::theme::{self, Weight, alpha, white};

/// Размер шрифта таблицы и панели.
pub const FONT: f32 = 12.5;
/// Высота строки таблицы.
pub const ROW_HEIGHT: f32 = 26.0;
/// Ширины колонок: курсор, аккаунт, регион, solo/duo, flex, 7 дней, игры. Остальное —
/// последняя колонка (в клиенте / обновляется / ошибка / теги).
const COLUMNS: [f32; 7] = [18.0, 238.0, 48.0, 168.0, 168.0, 58.0, 118.0];
const HEADERS: [&str; 8] = ["", "аккаунт", "рег", "solo/duo", "flex", "7д", "игры", ""];
/// Сколько клеток в полоске LP («████░░░»).
const BAR_CELLS: usize = 20;

fn font() -> FontId {
    theme::mono(FONT)
}

/// Строка из кусков разного цвета.
pub fn job(parts: &[(&str, Color32)], font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (text, color) in parts {
        job.append(text, 0.0, TextFormat::simple(font.clone(), *color));
    }
    job
}

/// «платина II 35lp», «мастер 120lp».
pub fn rank_text(stats: &RankedStats) -> String {
    let tier = stats.tier.label_ru().to_lowercase();
    if stats.tier.is_apex() {
        format!("{tier} {}lp", stats.lp)
    } else {
        format!("{tier} {} {}lp", stats.division.roman(), stats.lp)
    }
}

/// Ранг очереди для ячейки: текст и цвет. `None` — данных ещё нет.
fn rank_cell(stats: Option<Option<&RankedStats>>) -> (String, Color32) {
    match stats {
        None => ("—".into(), theme::TEXT_HINT),
        Some(None) => ("без ранга".into(), theme::TEXT_HINT),
        Some(Some(s)) => (rank_text(s), theme::tier_color(Some(s.tier))),
    }
}

/// «+62» / «−11» за неделю (Solo/Duo, а если там тихо — Flex).
fn week_cell(account: &Account) -> (String, Color32) {
    let since = unix_now() - 7 * DAY;
    let progress = history::Queue::ALL
        .into_iter()
        .find_map(|queue| history::progress_since(&account.history, queue, since));
    match progress {
        Some(p) => (
            history::signed_lp(p.lp).trim_end_matches(" LP").to_string(),
            match p.lp {
                0 => theme::TEXT_MUTED,
                lp if lp > 0 => theme::GREEN_TEXT,
                _ => theme::RED_TEXT,
            },
        ),
        None => ("·".into(), theme::TEXT_HINT),
    }
}

/// Буква результата игры: W, L или · (ремейк).
pub fn game_letter(game: &MatchSummary) -> (&'static str, Color32) {
    if game.remake {
        ("·", theme::TEXT_HINT)
    } else if game.win {
        ("W", theme::GREEN_TEXT)
    } else {
        ("L", theme::RED_TEXT)
    }
}

/// Короткий текст ошибки для последней колонки.
fn short_error(error: &AppError) -> String {
    if error.is_offline() {
        "! нет связи".into()
    } else {
        format!("! {error}")
    }
}

// ----- Таблица ------------------------------------------------------------------

/// Что нужно таблице, кроме самих аккаунтов.
pub struct Table<'a> {
    pub accounts: &'a [&'a Account],
    pub cursor: Option<AccountId>,
    pub in_client: Option<AccountId>,
    pub refreshing: &'a HashSet<AccountId>,
    pub errors: &'a HashMap<AccountId, AppError>,
    /// Курсор сдвинули с клавиатуры — прокрутить к нему.
    pub scroll_to_cursor: bool,
    pub max_height: f32,
}

impl Table<'_> {
    /// Рисует таблицу. Возвращает аккаунт, по строке которого щёлкнули.
    pub fn show(&self, ui: &mut Ui) -> Option<AccountId> {
        let font = font();
        // Заголовок колонок.
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        let painter = ui.painter();
        let mut x = head.left();
        for (index, title) in HEADERS.iter().enumerate() {
            painter.text(pos2(x, head.center().y), egui::Align2::LEFT_CENTER, *title, font.clone(), theme::TEXT_HINT);
            x += COLUMNS.get(index).copied().unwrap_or(0.0);
        }
        painter.hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::BORDER));
        ui.add_space(4.0);

        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("accounts_table")
            .max_height(self.max_height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for account in self.accounts {
                    let width = ui.available_width();
                    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
                    let is_cursor = self.cursor == Some(account.id);
                    if is_cursor && self.scroll_to_cursor {
                        ui.scroll_to_rect(rect, None);
                    }
                    if response.clicked() {
                        clicked = Some(account.id);
                    }
                    if ui.is_rect_visible(rect) {
                        self.row(ui, rect, account, is_cursor, response.hovered());
                    }
                }
            });
        clicked
    }

    fn row(&self, ui: &Ui, rect: Rect, account: &Account, is_cursor: bool, hovered: bool) {
        let painter = ui.painter_at(rect);
        if is_cursor {
            painter.rect_filled(rect, 0, theme::ACCENT);
        } else if hovered {
            painter.rect_filled(rect, 0, white(7));
        }
        // На курсор-строке весь текст тёмный — как выделение в терминале.
        let ink = |color: Color32| if is_cursor { theme::ON_ACCENT } else { color };
        let font = font();
        let cache = account.cache.as_ref();
        let ranked = cache.map(|c| &c.ranked);
        let (solo, solo_color) = rank_cell(ranked.map(|r| r.solo.as_ref()));
        let (flex, flex_color) = rank_cell(ranked.map(|r| r.flex.as_ref()));
        let (week, week_color) = week_cell(account);

        let games = match cache.map(|c| c.matches.as_deref()) {
            None => job(&[("—", ink(theme::TEXT_HINT))], &font),
            Some(None) => job(&[("нет данных", ink(theme::TEXT_HINT))], &font),
            Some(Some([])) => job(&[("нет игр", ink(theme::TEXT_HINT))], &font),
            Some(Some(games)) => {
                let mut parts = Vec::new();
                for (index, game) in games.iter().take(5).enumerate() {
                    if index > 0 {
                        parts.push((" ", ink(theme::TEXT_HINT)));
                    }
                    let (letter, color) = game_letter(game);
                    parts.push((letter, ink(color)));
                }
                job(&parts, &font)
            }
        };

        let extra = if self.in_client == Some(account.id) {
            job(&[("● в клиенте", ink(theme::GREEN_TEXT))], &font)
        } else if self.refreshing.contains(&account.id) {
            job(&[("обновляется…", ink(theme::ACCENT_TEXT))], &font)
        } else if let Some(error) = self.errors.get(&account.id) {
            job(&[(&short_error(error), ink(theme::RED_TEXT))], &font)
        } else {
            job(&[(&account.tags.join(" "), ink(theme::TEXT_HINT))], &font)
        };

        let cells = [
            job(&[(if is_cursor { "›" } else { "" }, ink(theme::TEXT))], &font),
            job(
                &[
                    (&account.riot_id.game_name, ink(theme::TEXT)),
                    (&format!("#{}", account.riot_id.tag_line), ink(theme::TEXT_MUTED)),
                ],
                &font,
            ),
            job(&[(account.region.label(), ink(theme::TEXT_MUTED))], &font),
            job(&[(&solo, ink(solo_color))], &font),
            job(&[(&flex, ink(flex_color))], &font),
            job(&[(&week, ink(week_color))], &font),
            games,
            extra,
        ];
        let mut x = rect.left();
        for (index, mut cell) in cells.into_iter().enumerate() {
            let width = COLUMNS.get(index).copied().unwrap_or(rect.right() - x) - 8.0;
            // Не влезает — обрезаем с «…».
            cell.wrap = egui::text::TextWrapping::truncate_at_width(width.max(8.0));
            let galley = painter.layout_job(cell);
            painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
            x += COLUMNS.get(index).copied().unwrap_or(0.0);
        }
    }
}

// ----- Панель выбранного аккаунта ------------------------------------------------

/// Что ещё нужно панели.
pub struct Details<'a> {
    pub account: &'a Account,
    pub in_client: bool,
    pub refreshing: bool,
    pub error: Option<&'a AppError>,
    /// Нажата «Войти»: логин в буфере, Enter скопирует пароль.
    pub login_pending: bool,
}

impl Details<'_> {
    /// Рисует панель. Возвращает действие, если щёлкнули по игре.
    pub fn show(&self, ui: &mut Ui) -> Option<Invocation> {
        let mut action = None;
        let account = self.account;
        egui::Frame::new()
            .fill(theme::CARD)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 4.0;
                let font = font();
                let k = theme::TEXT_HINT;

                // Строка 1: кто это и что за неделю.
                let mut who = vec![("аккаунт ", k), (&account.riot_id.game_name as &str, theme::TEXT)];
                let tag = format!("#{}", account.riot_id.tag_line);
                who.push((&tag, theme::TEXT_MUTED));
                let level = account.cache.as_ref().map(|c| format!(" · ур. {}", c.summoner_level));
                if let Some(level) = &level {
                    who.push((level, theme::TEXT_MUTED));
                }
                let tags = (!account.tags.is_empty()).then(|| format!(" · {}", account.tags.join(" ")));
                if let Some(tags) = &tags {
                    who.push((tags, k));
                }
                let week = self.week_text();
                two_columns(ui, job(&who, &font), week);

                // Строки 2–3: ранги с полоской LP и общий счёт.
                let cache = account.cache.as_ref();
                for (title, queue) in [("solo ", history::Queue::Solo), ("flex ", history::Queue::Flex)] {
                    let stats = cache.map(|c| queue.stats(&c.ranked));
                    let left = rank_line(title, stats, &font);
                    let right = match (queue, stats) {
                        (history::Queue::Solo, Some(Some(s))) => record_line("всего ", s, &font),
                        (history::Queue::Flex, _) => self.status_line(&font),
                        _ => job(&[("", k)], &font),
                    };
                    two_columns(ui, left, right);
                }

                // Последние игры: по три в колонке, щелчок — подробности.
                match cache.and_then(|c| c.matches.as_deref()) {
                    Some(games) if !games.is_empty() => {
                        ui.add_space(2.0);
                        let games: Vec<_> = games.iter().take(5).enumerate().collect();
                        let (left, right) = games.split_at(games.len().min(3));
                        ui.columns(2, |columns| {
                            for (column, list) in columns.iter_mut().zip([left, right]) {
                                column.spacing_mut().item_spacing.y = 2.0;
                                for (index, game) in list {
                                    if game_line(column, *index, game, &font) {
                                        action = Some(Invocation::on(Action::Game(*index), account.id));
                                    }
                                }
                            }
                        });
                    }
                    Some(_) => {
                        ui.label(job(&[("игр за последнее время нет", k)], &font));
                    }
                    None => {
                        ui.label(job(&[("игры ещё не загружались — r обновить", k)], &font));
                    }
                }

                // Архив: сколько игр сохранено всего — и кнопка «все игры».
                if account.archived_games > 0 {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let total = format!("{} игр сохранено", account.archived_games);
                        ui.label(job(&[("архив ", k), (&total, theme::TEXT_MUTED)], &font));
                        if key_button(ui, "m", "все игры", false).clicked() {
                            action = Some(Invocation::on(Action::Games, account.id));
                        }
                    });
                }

                if let Some(note) = account.note.lines().next().filter(|l| !l.trim().is_empty()) {
                    let mut line = job(&[("заметка ", k), (note, theme::TEXT_MUTED)], &font);
                    line.wrap = egui::text::TextWrapping::truncate_at_width(ui.available_width());
                    ui.label(line).on_hover_text(&account.note);
                }
                if self.login_pending {
                    ui.label(job(
                        &[
                            ("логин в буфере обмена — вставьте его в Riot Client, затем ", theme::ACCENT_TEXT),
                            ("enter", theme::TEXT),
                            (" скопирует пароль", theme::ACCENT_TEXT),
                        ],
                        &font,
                    ));
                }
            });
        action
    }

    fn week_text(&self) -> LayoutJob {
        let font = font();
        let k = theme::TEXT_HINT;
        let since = unix_now() - 7 * DAY;
        let solo = history::progress_since(&self.account.history, history::Queue::Solo, since);
        match solo {
            Some(p) => {
                let color = if p.lp >= 0 { theme::GREEN_TEXT } else { theme::RED_TEXT };
                job(&[("7 дней ", k), (&history::signed_lp(p.lp), color), (" · ", k), (&p.record(), theme::TEXT)], &font)
            }
            None => job(&[("7 дней ", k), ("без изменений", theme::TEXT_MUTED)], &font),
        }
    }

    /// «обновлено 5 мин назад», «обновляется…», ошибка.
    fn status_line(&self, font: &FontId) -> LayoutJob {
        let k = theme::TEXT_HINT;
        if self.refreshing {
            return job(&[("обновляется…", theme::ACCENT_TEXT)], font);
        }
        let age = self
            .account
            .cache
            .as_ref()
            .map(|c| format!("обновлено {}", format::relative_time(c.fetched_at)));
        match (self.error, age) {
            (Some(error), _) => job(&[(&short_error(error), theme::RED_TEXT)], font),
            (None, Some(age)) if self.in_client => job(&[(&age, k), (" · ", k), ("● в клиенте", theme::GREEN_TEXT)], font),
            (None, Some(age)) => job(&[(&age, k)], font),
            (None, None) => job(&[("данных ещё нет — r обновить", k)], font),
        }
    }
}

/// Две колонки одной строкой: левая — половина ширины.
fn two_columns(ui: &mut Ui, left: LayoutJob, right: LayoutJob) {
    let width = ui.available_width();
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(width / 2.0, 18.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_width(width / 2.0);
            ui.label(left);
        });
        ui.label(right);
    });
}

/// «solo платина II ███████░░░░░░░░░░░░░ 35/100».
fn rank_line(title: &str, stats: Option<Option<&RankedStats>>, font: &FontId) -> LayoutJob {
    let k = theme::TEXT_HINT;
    match stats {
        None => job(&[(title, k), ("нет данных", k)], font),
        Some(None) => job(&[(title, k), ("без ранга", theme::TEXT_MUTED)], font),
        Some(Some(s)) => {
            let color = theme::tier_color(Some(s.tier));
            let tier = s.tier.label_ru().to_lowercase();
            let name = if s.tier.is_apex() { tier } else { format!("{tier} {}", s.division.roman()) };
            if s.tier.is_apex() {
                return job(&[(title, k), (&name, color), (&format!(" {} LP", s.lp), theme::TEXT)], font);
            }
            let filled = (s.lp.clamp(0, 100) as usize * BAR_CELLS).div_ceil(100).min(BAR_CELLS);
            let bar_on = "█".repeat(filled);
            let bar_off = "░".repeat(BAR_CELLS - filled);
            job(
                &[
                    (title, k),
                    (&format!("{name:<12}"), color),
                    (&bar_on, color),
                    (&bar_off, theme::BORDER),
                    (&format!(" {}/100", s.lp), theme::TEXT),
                ],
                font,
            )
        }
    }
}

/// «всего 35W 29L · 55%».
fn record_line(title: &str, s: &RankedStats, font: &FontId) -> LayoutJob {
    let k = theme::TEXT_HINT;
    match (s.losses, s.winrate()) {
        (Some(losses), Some(wr)) => {
            let color = if wr >= 50.0 { theme::GREEN_TEXT } else { theme::RED_TEXT };
            job(&[(title, k), (&format!("{}W {losses}L", s.wins), theme::TEXT), (" · ", k), (&format!("{wr:.0}%"), color)], font)
        }
        _ => job(&[(title, k), (&format!("{}W", s.wins), theme::TEXT), (" · поражения скрыты", k)], font),
    }
}

/// «1 W экко 14/5/7 27:25 4 дн» — щелчок открывает подробности. Возвращает true при щелчке.
fn game_line(ui: &mut Ui, index: usize, game: &MatchSummary, font: &FontId) -> bool {
    let k = theme::TEXT_HINT;
    let (letter, color) = game_letter(game);
    let champion = match_view::champion_name(&game.me).to_lowercase();
    let line = job(
        &[
            (&format!("{} ", index + 1), theme::ACCENT_TEXT),
            (letter, color),
            (" ", k),
            (&format!("{champion:<10.10}"), theme::TEXT),
            (&format!(" {:<9}", game.kda()), theme::TEXT),
            (&format!(" {:>5}", match_view::duration(game.duration_secs)), k),
            (&format!("  {}", format::relative_time(game.played_at)), k),
        ],
        font,
    );
    let response = ui
        .add(egui::Label::new(line).sense(Sense::click()).truncate())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(format!("{} — подробности игры", index + 1));
    if response.hovered() {
        ui.painter().rect_filled(response.rect.expand(1.0), 0, white(8));
    }
    response.clicked()
}

// ----- Подсказки клавиш, командная строка, справка -------------------------------

/// Одна подсказка в нижней строке: клавиша, подпись и что делает.
pub struct Hint {
    pub key: &'static str,
    pub label: &'static str,
    pub action: Action,
    /// Подсветить (например, «enter пароль» после «Войти»).
    pub accent: bool,
}

impl Hint {
    pub fn new(key: &'static str, label: &'static str, action: Action) -> Self {
        Self {
            key,
            label,
            action,
            accent: false,
        }
    }
}

/// Строка подсказок «enter войти · p пароль …». Каждая подсказка — ещё и кнопка.
pub fn hints_bar(ui: &mut Ui, hints: &[Hint]) -> Option<Action> {
    let mut pressed = None;
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        for hint in hints {
            if key_button(ui, hint.key, hint.label, hint.accent).clicked() {
                pressed = Some(hint.action.clone());
            }
        }
    });
    pressed
}

/// Кнопка в виде клавиши с подписью: «[enter] войти». `accent` — янтарная клавиша.
pub fn key_button(ui: &mut Ui, key: &str, label: &str, accent: bool) -> egui::Response {
    let font = theme::mono(11.5);
    let (key_color, key_bg, label_color) = if accent {
        (theme::ON_ACCENT, theme::ACCENT, theme::ACCENT_TEXT)
    } else {
        (theme::TEXT, theme::BORDER, theme::TEXT_MUTED)
    };
    let key = ui.painter().layout_no_wrap(key.to_string(), font.clone(), key_color);
    let label = ui.painter().layout_no_wrap(label.to_string(), font, label_color);
    let size = vec2(key.size().x + 10.0 + 6.0 + label.size().x, 18.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let key_rect = Rect::from_min_size(rect.min, vec2(key.size().x + 10.0, rect.height()));
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect.expand(2.0), 2, white(8));
    }
    painter.rect_filled(key_rect, 2, key_bg);
    painter.galley(key_rect.center() - key.size() / 2.0, key, key_color);
    painter.galley(
        pos2(key_rect.right() + 6.0, rect.center().y - label.size().y / 2.0),
        label,
        label_color,
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Открытая командная строка.
#[derive(Default)]
pub struct CommandLine {
    pub input: String,
    /// Какая подсказка выбрана (↑ ↓).
    pub selected: usize,
}

/// Что сделали в командной строке.
pub enum CommandEvent {
    None,
    Close,
    Run(Invocation),
}

impl CommandLine {
    /// Рисует строку «:войти dr4g▌» и подсказки над ней (слой поверх окна).
    /// `suggestions` посчитаны окном по текущему тексту.
    pub fn show(&mut self, ui: &mut Ui, suggestions: &[Suggestion]) -> CommandEvent {
        let id = egui::Id::unique("command_line");
        // ↑ ↓ Tab Enter Esc забираем до поля ввода: однострочное поле само двигало бы
        // курсор в начало и конец.
        let (up, down, tab, enter, escape) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        let count = suggestions.len();
        if count > 0 {
            if up {
                self.selected = (self.selected + count - 1) % count;
            }
            if down {
                self.selected = (self.selected + 1) % count;
            }
            self.selected = self.selected.min(count - 1);
        }
        let current = suggestions.get(self.selected);

        let mut event = CommandEvent::None;
        if escape {
            return CommandEvent::Close;
        }
        if tab && let Some(s) = current {
            self.input = s.completion.clone();
            self.selected = 0;
            move_cursor_to_end(ui.ctx(), id, &self.input);
        }
        if enter {
            match current {
                Some(Suggestion { run: Some(run), .. }) => event = CommandEvent::Run(run.clone()),
                // Команде нужно продолжение («сортировка …») — дописываем, как Tab.
                Some(s) => {
                    self.input = s.completion.clone();
                    self.selected = 0;
                    move_cursor_to_end(ui.ctx(), id, &self.input);
                }
                None if self.input.trim().is_empty() => event = CommandEvent::Close,
                None => {}
            }
        }

        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(RichText::new(":").font(theme::mono(13.0)).color(theme::ACCENT_TEXT));
            let before = self.input.clone();
            let edit = egui::TextEdit::singleline(&mut self.input)
                .id(id)
                .font(theme::mono(13.0))
                .text_color(theme::TEXT)
                .frame(egui::Frame::NONE)
                .hint_text(RichText::new("команда или ник — например «войти dr4g»").color(theme::TEXT_HINT))
                .desired_width(ui.available_width());
            let response = ui.add(edit);
            if !response.has_focus() {
                response.request_focus();
            }
            if self.input != before {
                self.selected = 0;
            }
        });

        // Подсказки — над строкой, слоем поверх окна.
        if !suggestions.is_empty() {
            let bottom = ui.min_rect().top() - 4.0;
            let left = ui.min_rect().left();
            egui::Area::new(egui::Id::unique("command_suggestions"))
                .order(egui::Order::Foreground)
                .fixed_pos(pos2(left, bottom))
                .pivot(egui::Align2::LEFT_BOTTOM)
                .interactable(true)
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(theme::CARD)
                        .stroke(Stroke::new(1.0, theme::BORDER))
                        .inner_margin(Margin::symmetric(0, 4))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for (index, s) in suggestions.iter().enumerate() {
                                if suggestion_row(ui, s, index == self.selected) {
                                    match &s.run {
                                        Some(run) => event = CommandEvent::Run(run.clone()),
                                        None => {
                                            self.input = s.completion.clone();
                                            move_cursor_to_end(ui.ctx(), id, &self.input);
                                        }
                                    }
                                }
                            }
                        });
                });
        }
        event
    }
}

/// Одна подсказка: «войти dr4gm4shroom#1933   логин в буфер…». Возвращает true при щелчке.
fn suggestion_row(ui: &mut Ui, s: &Suggestion, selected: bool) -> bool {
    let font = theme::mono(12.5);
    let (rect, response) = ui.allocate_exact_size(vec2(520.0, 24.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 0, alpha(theme::ACCENT, 34));
        painter.vline(rect.left() + 1.0, rect.y_range(), Stroke::new(2.0, theme::ACCENT));
    } else if response.hovered() {
        painter.rect_filled(rect, 0, white(6));
    }
    let mut label = job(&[(&s.label, if selected { theme::TEXT } else { theme::TEXT_MUTED })], &font);
    label.wrap = egui::text::TextWrapping::truncate_at_width(300.0);
    let galley = painter.layout_job(label);
    painter.galley(pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
    let hint = painter.layout_no_wrap(s.hint.to_string(), theme::mono(11.0), theme::TEXT_HINT);
    painter.galley(pos2(rect.right() - hint.size().x - 12.0, rect.center().y - hint.size().y / 2.0), hint, theme::TEXT_HINT);
    response.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

/// Курсор поля ввода — в конец текста (после Tab-дополнения).
fn move_cursor_to_end(ctx: &egui::Context, id: egui::Id, text: &str) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        let end = egui::text::CCursor::new(text.chars().count());
        state.cursor.set_char_range(Some(egui::text_selection::CCursorRange::one(end)));
        state.store(ctx, id);
    }
}

/// Окно справки по клавишам. Возвращает true, если его закрыли.
pub fn help(ctx: &egui::Context) -> bool {
    let close_keys = ctx.input_mut(|i| {
        i.consume_key(egui::Modifiers::NONE, egui::Key::F1)
            || i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "?"))
    });
    let mut close = close_keys;
    // Окно может быть низким: разделы — в две колонки, а если и так не влезает — прокрутка.
    // Высота задана явно (min_scrolled_height), как в окне игры: иначе в модальном окне
    // прокрутка берёт место с прошлого кадра и сжимается.
    let height = (ctx.content_rect().height() - 150.0).max(160.0);
    let modal = egui::Modal::new(egui::Id::unique("help"))
        .frame(theme::dialog_frame().inner_margin(Margin::symmetric(22, 16)))
        .show(ctx, |ui| {
            ui.set_width(860.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("горячие клавиши").font(theme::mono(15.0)).color(theme::TEXT).semibold());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    close |= key_button(ui, "esc", "закрыть", false).clicked();
                });
            });
            ui.label(
                RichText::new("буквы работают в любой раскладке; пока набираете текст, они не срабатывают")
                    .font(theme::mono(11.5))
                    .color(theme::TEXT_HINT),
            );
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(height)
                .min_scrolled_height(height.min(440.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.columns(2, |columns| {
                        // Слева «список» и «везде», справа — «аккаунт» и «лобби».
                        for (column, sections) in columns.iter_mut().zip([[0, 3].as_slice(), [1, 2].as_slice()]) {
                            for &index in sections {
                                if let Some(section) = keymap::HELP.get(index) {
                                    help_section(column, section);
                                }
                            }
                        }
                    });
                });
        });
    close || modal.should_close()
}

/// Раздел справки: «— список» и строки «клавиши  что делают».
fn help_section(ui: &mut Ui, section: &keymap::HelpSection) {
    ui.add_space(8.0);
    ui.label(RichText::new(format!("— {}", section.title)).font(theme::mono(12.0)).color(theme::ACCENT_TEXT));
    ui.add_space(2.0);
    for (keys, what) in section.rows {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let (rect, _) = ui.allocate_exact_size(vec2(124.0, 17.0), Sense::hover());
            ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, *keys, theme::mono(12.0), theme::TEXT);
            ui.add(egui::Label::new(RichText::new(*what).font(theme::mono(12.0)).color(theme::TEXT_MUTED)).wrap());
        });
        ui.add_space(2.0);
    }
}

/// Рамка вокруг области, в которой сейчас фокус клавиатуры (поиск), — янтарная.
pub fn focus_outline(ui: &Ui, rect: Rect) {
    ui.painter().rect_stroke(rect, 2, Stroke::new(1.0, alpha(theme::ACCENT, 160)), StrokeKind::Inside);
}
