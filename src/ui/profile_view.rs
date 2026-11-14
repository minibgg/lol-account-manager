//! Окно «Профиль» (клавиша i): статистика аккаунта по архиву игр. Сверху — ранги
//! и оценка сгорания, общий итог (винрейт, KDA, CS и урон в минуту), последние игры
//! буквами W/L; ниже — таблица чемпионов.
//!
//! Клавиши: Tab — очередь, ← → — период, s — сортировка чемпионов, ↑ ↓ PgUp PgDn
//! Home End — курсор, Esc — закрыть.

use eframe::egui::{self, Align, Layout, Margin, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::decay;
use crate::error::AppError;
use crate::history::{DAY, Queue};
use crate::lobby::Position;
use crate::profile::{self, ChampionSort, ChampionStats, Totals};
use crate::riot::{Inventory, MatchSummary};
use crate::storage::{Account, AccountId, unix_now};
use crate::ui::format;
use crate::ui::games_view::Filter;
use crate::ui::match_view;
use crate::ui::terminal::{self, FONT, ROW_HEIGHT, job};
use crate::ui::theme::{self, Weight, white};

/// Ширины колонок: курсор, чемпион, игр, W-L, винрейт, KDA, K/D/A, CS/мин, урон/мин;
/// дальше — «последняя игра».
const COLUMNS: [f32; 9] = [16.0, 150.0, 50.0, 70.0, 132.0, 58.0, 112.0, 60.0, 70.0];
const HEADERS: [&str; 10] = ["", "чемпион", "игр", "w-l", "винрейт", "kda", "k/d/a", "cs/м", "урон/м", "последняя"];
const WIDTH: f32 = 820.0;
/// Сколько клеток в полоске винрейта.
const BAR_CELLS: usize = 10;
/// Сколько последних игр показать буквами W/L.
const FORM_GAMES: usize = 24;

/// За какое время считать (← →).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Period {
    Month,
    Quarter,
    All,
}

impl Period {
    const ALL: [Self; 3] = [Self::Month, Self::Quarter, Self::All];

    fn label(self) -> &'static str {
        match self {
            Self::Month => "30 дней",
            Self::Quarter => "90 дней",
            Self::All => "всё время",
        }
    }

    /// С какого момента (unix-время) игры входят в период.
    fn since(self, now: i64) -> i64 {
        match self {
            Self::Month => now - 30 * DAY,
            Self::Quarter => now - 90 * DAY,
            Self::All => i64::MIN,
        }
    }
}

/// Следующий (или предыдущий) элемент списка по кругу.
fn cycle<T: Copy + PartialEq>(all: &[T], current: T, forward: bool) -> T {
    let index = all.iter().position(|x| *x == current).unwrap_or(0);
    let step = if forward { 1 } else { all.len() - 1 };
    all[(index + step) % all.len()]
}

/// Открытое окно «Профиль».
pub struct ProfileView {
    pub account: AccountId,
    /// Все игры из архива, новые первыми. `None` — ещё грузятся.
    pub games: Option<Result<Vec<MatchSummary>, AppError>>,
    /// Инвентарь: `None` — ещё грузится, `Some(None)` — этот аккаунт в клиенте ещё не видели.
    pub inventory: Option<Option<Inventory>>,
    filter: Filter,
    period: Period,
    sort: ChampionSort,
    cursor: usize,
    scroll_to_cursor: bool,
    offset: f32,
}

impl ProfileView {
    pub fn new(account: AccountId) -> Self {
        Self {
            account,
            games: None,
            inventory: None,
            filter: Filter::All,
            period: Period::All,
            sort: ChampionSort::Games,
            cursor: 0,
            scroll_to_cursor: false,
            offset: 0.0,
        }
    }

    /// Игры под текущими фильтром и периодом, новые первыми.
    fn visible(&self, now: i64) -> Vec<&MatchSummary> {
        let since = self.period.since(now);
        match &self.games {
            Some(Ok(games)) => games
                .iter()
                .filter(|g| self.filter.accepts(g) && g.played_at >= since)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Рисует окно. Возвращает true — «закрыть».
    pub fn show(&mut self, ctx: &egui::Context, account: &Account) -> bool {
        let now = unix_now();
        let games: Vec<MatchSummary> = self.visible(now).into_iter().cloned().collect();
        let champions = profile::champions(&games, self.sort);
        self.handle_keys(ctx, champions.len());

        let mut close = false;
        let height = (ctx.content_rect().height() - 410.0).max(ROW_HEIGHT * 3.0);
        let modal = egui::Modal::new(egui::Id::unique("profile_view"))
            .frame(theme::dialog_frame().inner_margin(Margin::symmetric(20, 16)))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing.y = 4.0;
                close = header(ui, account, now);
                inventory_line(ui, self.inventory.as_ref());
                ui.add_space(4.0);
                self.switches(ui);
                separator(ui);
                match &self.games {
                    None => {
                        ui.label(RichText::new("загружаю архив…").font(theme::mono(FONT)).color(theme::TEXT_MUTED));
                    }
                    Some(Err(error)) => {
                        ui.label(RichText::new(format!("! {error}")).font(theme::mono(FONT)).color(theme::RED_TEXT));
                    }
                    Some(Ok(_)) if games.is_empty() => {
                        ui.label(
                            RichText::new("игр за этот период нет — попробуйте другой период (← →) или очередь (tab)")
                                .font(theme::mono(FONT))
                                .color(theme::TEXT_HINT),
                        );
                    }
                    Some(Ok(_)) => {
                        summary(ui, &games);
                        separator(ui);
                        self.table(ui, &champions, height);
                    }
                }
            });
        close || modal.should_close()
    }

    fn handle_keys(&mut self, ctx: &egui::Context, count: usize) {
        use egui::Key;
        let none = egui::Modifiers::NONE;
        let (up, down, page_up, page_down, home, end, tab, left, right, sort) = ctx.input_mut(|i| {
            (
                i.consume_key(none, Key::ArrowUp) || i.consume_key(none, Key::K),
                i.consume_key(none, Key::ArrowDown) || i.consume_key(none, Key::J),
                i.consume_key(none, Key::PageUp),
                i.consume_key(none, Key::PageDown),
                i.consume_key(none, Key::Home),
                i.consume_key(none, Key::End),
                i.consume_key(none, Key::Tab),
                i.consume_key(none, Key::ArrowLeft),
                i.consume_key(none, Key::ArrowRight),
                i.consume_key(none, Key::S),
            )
        });
        if tab {
            self.filter = cycle(&Filter::ALL, self.filter, true);
        }
        if left || right {
            self.period = cycle(&Period::ALL, self.period, right);
        }
        if sort {
            self.sort = cycle(&ChampionSort::ALL, self.sort, true);
        }
        if tab || left || right || sort {
            // Список стал другим — курсор в начало.
            self.cursor = 0;
            self.scroll_to_cursor = true;
            return;
        }
        if count == 0 {
            self.cursor = 0;
            return;
        }
        let last = count - 1;
        let before = self.cursor;
        if up {
            self.cursor = self.cursor.saturating_sub(1);
        }
        if down {
            self.cursor += 1;
        }
        if page_up {
            self.cursor = self.cursor.saturating_sub(10);
        }
        if page_down {
            self.cursor += 10;
        }
        if home {
            self.cursor = 0;
        }
        if end {
            self.cursor = last;
        }
        self.cursor = self.cursor.min(last);
        self.scroll_to_cursor |= self.cursor != before;
    }

    /// Переключатели: очередь, период, сортировка. Каждое слово — ещё и кнопка.
    fn switches(&mut self, ui: &mut egui::Ui) {
        let font = theme::mono(FONT);
        let mut changed = false;
        // Одно слово-кнопка; возвращает true при щелчке.
        let word = |ui: &mut egui::Ui, text: &str, active: bool| {
            let color = if active { theme::ACCENT_TEXT } else { theme::TEXT_MUTED };
            ui.add(egui::Label::new(RichText::new(text).font(font.clone()).color(color)).sense(Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            terminal::key_button(ui, "tab", "очередь", false);
            for filter in Filter::ALL {
                if word(ui, filter.label(), filter == self.filter) {
                    self.filter = filter;
                    changed = true;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            terminal::key_button(ui, "← →", "период", false);
            for period in Period::ALL {
                if word(ui, period.label(), period == self.period) {
                    self.period = period;
                    changed = true;
                }
            }
            ui.add_space(16.0);
            terminal::key_button(ui, "s", "сортировка", false);
            for sort in ChampionSort::ALL {
                if word(ui, sort.label(), sort == self.sort) {
                    self.sort = sort;
                    changed = true;
                }
            }
        });
        if changed {
            self.cursor = 0;
            self.scroll_to_cursor = true;
        }
    }

    /// Таблица чемпионов.
    fn table(&mut self, ui: &mut egui::Ui, champions: &[ChampionStats], height: f32) {
        let font = theme::mono(FONT);
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        let painter = ui.painter();
        let mut x = head.left();
        for (index, title) in HEADERS.iter().enumerate() {
            painter.text(pos2(x, head.center().y), egui::Align2::LEFT_CENTER, *title, font.clone(), theme::TEXT_HINT);
            x += COLUMNS.get(index).copied().unwrap_or(0.0);
        }
        painter.hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::BORDER));

        // Строки вплотную: show_rows считает высоту строки вместе с этим промежутком.
        ui.spacing_mut().item_spacing.y = 0.0;
        let view_height = height.min(champions.len() as f32 * ROW_HEIGHT);
        let mut area = egui::ScrollArea::vertical()
            .id_salt("profile_champions")
            .max_height(height)
            .min_scrolled_height(view_height)
            .auto_shrink([false, true]);
        // Курсор ушёл за край видимой части — прокручиваем к нему (как в «Всех играх»).
        if std::mem::take(&mut self.scroll_to_cursor) {
            let top = self.cursor as f32 * ROW_HEIGHT;
            if top < self.offset {
                self.offset = top;
            } else if top + ROW_HEIGHT > self.offset + view_height {
                self.offset = top + ROW_HEIGHT - view_height;
            }
            area = area.vertical_scroll_offset(self.offset);
        }
        let output = area.show_rows(ui, ROW_HEIGHT, champions.len(), |ui, range| {
            for index in range {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                if response.clicked() {
                    self.cursor = index;
                }
                row(ui, rect, &champions[index], index == self.cursor, response.hovered());
            }
        });
        self.offset = output.state.clamped_offset().y;
    }
}

/// Тонкая линия-разделитель.
fn separator(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(line, 0, theme::BORDER);
    ui.add_space(4.0);
}

/// Заголовок: чей профиль, ранги и сгорание. Возвращает true — «закрыть».
fn header(ui: &mut egui::Ui, account: &Account, now: i64) -> bool {
    let mut close = false;
    let font = theme::mono(FONT);
    let k = theme::TEXT_HINT;
    ui.horizontal(|ui| {
        ui.label(RichText::new("профиль").font(theme::mono(15.0)).color(theme::TEXT).semibold());
        ui.label(RichText::new(account.riot_id.to_string()).font(font.clone()).color(theme::TEXT_MUTED));
        let mut about = account.region.label().to_lowercase();
        if let Some(cache) = &account.cache {
            about.push_str(&format!(" · ур. {}", cache.summoner_level));
        }
        ui.label(RichText::new(about).font(font.clone()).color(k));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            close = terminal::key_button(ui, "esc", "закрыть", false).clicked();
        });
    });
    let Some(cache) = &account.cache else {
        ui.label(job(&[("рангов ещё нет — закройте окно и нажмите r, чтобы обновить аккаунт", k)], &font));
        return close;
    };
    for (title, queue) in [("solo ", Queue::Solo), ("flex ", Queue::Flex)] {
        let Some(stats) = queue.stats(&cache.ranked) else {
            ui.label(job(&[(title, k), ("без ранга", theme::TEXT_MUTED)], &font));
            continue;
        };
        let rank = terminal::rank_text(stats);
        let record = match (stats.losses, stats.winrate()) {
            (Some(losses), Some(winrate)) => format!("   {}W {losses}L · {winrate:.0}%", stats.wins),
            _ => format!("   {}W · поражения скрыты", stats.wins),
        };
        let mut parts = vec![
            (title, k),
            (rank.as_str(), theme::tier_color(Some(stats.tier))),
            (record.as_str(), theme::TEXT_MUTED),
        ];
        // Алмаз и выше: когда ранг начнёт сгорать без игр (оценка по архиву).
        let decay = decay::of_account(account, queue, now).map(|d| (format!("   {}", d.label()), d));
        if let Some((text, decay)) = &decay {
            let color = if decay.is_urgent() {
                theme::RED_TEXT
            } else if decay.is_warning() {
                theme::AMBER_TEXT
            } else {
                k
            };
            parts.push((text.as_str(), color));
        }
        ui.label(job(&parts, &font));
    }
    close
}

/// «инвентарь чемпионов 75 · образов 56 · BE 12 415 · RP 0 · оранжевой эссенции 29».
fn inventory_line(ui: &mut egui::Ui, inventory: Option<&Option<Inventory>>) {
    let font = theme::mono(FONT);
    let k = theme::TEXT_HINT;
    let dot = (" · ", k);
    match inventory {
        None => {}
        Some(None) => {
            ui.label(job(
                &[("инвентарь ", k), ("появится, когда вы войдёте в этот аккаунт в клиенте League и откроете профиль", k)],
                &font,
            ));
        }
        Some(Some(inventory)) => {
            // Числа с разрядами через пробел; отрицательных здесь не бывает.
            let number = |n: i64| match_view::number(n.clamp(0, i64::from(u32::MAX)) as u32);
            let champions = inventory.champions.to_string();
            let skins = inventory.skins.to_string();
            let (blue, orange, rp) = (number(inventory.blue_essence), number(inventory.orange_essence), number(inventory.rp));
            let seen = format!("на {}", format::relative_time(inventory.fetched_at));
            ui.label(job(
                &[
                    ("инвентарь ", k),
                    ("чемпионов ", k),
                    (&champions, theme::TEXT),
                    dot,
                    ("образов ", k),
                    (&skins, theme::TEXT),
                    dot,
                    ("синей эссенции ", k),
                    (&blue, theme::TEXT),
                    dot,
                    ("оранжевой ", k),
                    (&orange, theme::TEXT),
                    dot,
                    ("rp ", k),
                    (&rp, theme::TEXT),
                    dot,
                    (&seen, k),
                ],
                &font,
            ));
        }
    }
}

/// Общий итог по играм и последние игры буквами.
fn summary(ui: &mut egui::Ui, games: &[MatchSummary]) {
    let font = theme::mono(FONT);
    let k = theme::TEXT_HINT;
    let dot = (" · ", k);
    let totals = Totals::of(games);

    let games_text = format!("{} игр", totals.games);
    let record = format!("{}W {}L", totals.wins, totals.losses());
    let winrate = totals.winrate().map(|w| (format!("{w:.0}%"), winrate_color(w)));
    let streak = profile::streak(games).filter(|(_, n)| *n >= 2).map(|(win, n)| {
        let (letter, color) = if win { ("W", theme::GREEN_TEXT) } else { ("L", theme::RED_TEXT) };
        (format!("серия {n}{letter}"), color)
    });
    let remakes = (totals.remakes > 0).then(|| format!("ремейков {}", totals.remakes));
    let mut line: Vec<(&str, egui::Color32)> = vec![(&games_text, theme::TEXT), dot, (&record, theme::TEXT)];
    if let Some((text, color)) = &winrate {
        line.extend([dot, (text.as_str(), *color)]);
    }
    if let Some((text, color)) = &streak {
        line.extend([dot, (text.as_str(), *color)]);
    }
    if let Some(text) = &remakes {
        line.extend([dot, (text.as_str(), k)]);
    }
    ui.label(job(&line, &font));

    // Средние: KDA и показатели в минуту (последние — только по играм, где они записаны).
    let ratio = totals.kda_ratio();
    let kda = format!("{ratio:.1}");
    let average = format!(" ({})", totals.average_kda());
    let length = match_view::duration(totals.average_secs());
    let optional = |value: Option<f32>, digits: usize| value.map_or("—".to_string(), |v| format!("{v:.digits$}"));
    let cs = optional(totals.cs_per_minute(), 1);
    let damage = optional(totals.damage_per_minute(), 0);
    let gold = optional(totals.gold_per_minute(), 0);
    let vision = optional(totals.vision_per_game(), 0);
    ui.label(job(
        &[
            ("kda ", k),
            (&kda, match_view::kda_color(ratio)),
            (&average, theme::TEXT_MUTED),
            dot,
            ("cs/мин ", k),
            (&cs, theme::TEXT),
            dot,
            ("урон/мин ", k),
            (&damage, theme::TEXT),
            dot,
            ("золото/мин ", k),
            (&gold, theme::TEXT),
            dot,
            ("обзор ", k),
            (&vision, theme::TEXT),
            dot,
            ("игра ", k),
            (&length, theme::TEXT),
        ],
        &font,
    ));

    // Роли: «мид 12 (58%) · бот 5 (40%)» — только игры, где позиция известна.
    let roles = profile::positions(games);
    if !roles.is_empty() {
        let texts: Vec<(String, String, egui::Color32)> = roles
            .iter()
            .map(|(position, totals)| {
                let name = match Position::from_lcu(position) {
                    Some(known) => known.label().to_lowercase(),
                    None => position.clone(),
                };
                let winrate = totals.winrate().unwrap_or(0.0);
                (format!("{name} {}", totals.games), format!(" {winrate:.0}%"), winrate_color(winrate))
            })
            .collect();
        let mut line: Vec<(&str, egui::Color32)> = vec![("роли ", k)];
        for (index, (name, winrate, color)) in texts.iter().enumerate() {
            if index > 0 {
                line.push(dot);
            }
            line.extend([(name.as_str(), theme::TEXT), (winrate.as_str(), *color)]);
        }
        ui.label(job(&line, &font));
    }

    // Последние игры: новые слева.
    let mut form: Vec<(&str, egui::Color32)> = vec![("форма ", k)];
    for game in games.iter().take(FORM_GAMES) {
        let (letter, color) = terminal::game_letter(game);
        form.extend([(letter, color), (" ", k)]);
    }
    form.push(("← новые слева", k));
    ui.label(job(&form, &font));
}

fn winrate_color(winrate: f32) -> egui::Color32 {
    if winrate >= 50.0 { theme::GREEN_TEXT } else { theme::RED_TEXT }
}

/// Название чемпиона, а если неизвестно — номер.
fn champion_name(champion: &ChampionStats) -> String {
    if champion.name.is_empty() {
        format!("чемпион #{}", champion.champion_id)
    } else {
        champion.name.to_lowercase()
    }
}

/// Строка чемпиона: «› экко  42  25-17  60% ██████░░░░  4.2  8.1/4.0/8.7  7.1  820  2 дн назад».
fn row(ui: &egui::Ui, rect: Rect, champion: &ChampionStats, is_cursor: bool, hovered: bool) {
    let painter = ui.painter_at(rect);
    if is_cursor {
        painter.rect_filled(rect, 0, theme::ACCENT);
    } else if hovered {
        painter.rect_filled(rect, 0, white(7));
    }
    // На курсор-строке весь текст тёмный — как выделение в терминале.
    let ink = |color| if is_cursor { theme::ON_ACCENT } else { color };
    let font = theme::mono(FONT);
    let totals = &champion.totals;
    let winrate = totals.winrate().unwrap_or(0.0);
    let filled = ((winrate / 100.0 * BAR_CELLS as f32).round() as usize).min(BAR_CELLS);
    let ratio = totals.kda_ratio();
    let optional = |value: Option<f32>, digits: usize| value.map_or("—".to_string(), |v| format!("{v:.digits$}"));

    let simple = |text: String, color| job(&[(&text, ink(color))], &font);
    let cells = [
        simple((if is_cursor { "›" } else { "" }).into(), theme::TEXT),
        simple(champion_name(champion), theme::TEXT),
        simple(totals.games.to_string(), theme::TEXT),
        simple(format!("{}-{}", totals.wins, totals.losses()), theme::TEXT_MUTED),
        job(
            &[
                (&format!("{winrate:>3.0}% "), ink(winrate_color(winrate))),
                (&"█".repeat(filled), ink(winrate_color(winrate))),
                (&"░".repeat(BAR_CELLS - filled), ink(theme::BORDER)),
            ],
            &font,
        ),
        simple(format!("{ratio:.1}"), match_view::kda_color(ratio)),
        simple(totals.average_kda(), theme::TEXT_MUTED),
        simple(optional(totals.cs_per_minute(), 1), theme::TEXT_MUTED),
        simple(optional(totals.damage_per_minute(), 0), theme::TEXT_MUTED),
        simple(format::relative_time(champion.last_played), theme::TEXT_HINT),
    ];
    let mut x = rect.left();
    for (index, mut cell) in cells.into_iter().enumerate() {
        let width = COLUMNS.get(index).copied().unwrap_or(rect.right() - x) - 8.0;
        cell.wrap = egui::text::TextWrapping::truncate_at_width(width.max(8.0));
        let galley = painter.layout_job(cell);
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
        x += COLUMNS.get(index).copied().unwrap_or(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::{Division, PlayerStats, RankedQueues, RankedStats, Region, RiotId, Tier};
    use crate::storage::CachedProfile;

    fn game(champion: i32, queue_id: i32, win: bool, days_ago: i64) -> MatchSummary {
        MatchSummary {
            id: format!("RU_{champion}_{queue_id}_{days_ago}"),
            win,
            remake: false,
            queue_id,
            played_at: unix_now() - days_ago * DAY,
            duration_secs: 1800,
            stats_version: crate::riot::models::MATCH_STATS_VERSION,
            me: PlayerStats {
                champion_id: champion,
                champion: format!("Чемпион {champion}"),
                kills: 5,
                deaths: 3,
                assists: 7,
                cs: 190,
                damage: 20_000,
                ..Default::default()
            },
        }
    }

    fn account() -> Account {
        let now = unix_now();
        Account {
            id: 1,
            riot_id: RiotId::parse("Name#RU1").unwrap(),
            region: Region::Ru,
            puuid: None,
            lcu_puuid: None,
            note: String::new(),
            tags: vec![],
            secret_ref: "ref".into(),
            has_login: false,
            has_password: false,
            created_at: 0,
            cache: Some(CachedProfile {
                profile_icon_id: 1,
                summoner_level: 300,
                ranked: RankedQueues {
                    solo: Some(RankedStats {
                        tier: Tier::Diamond,
                        division: Division::Two,
                        lp: 35,
                        wins: 40,
                        losses: Some(30),
                    }),
                    flex: None,
                },
                matches: None,
                fetched_at: now,
            }),
            history: vec![],
            archived_games: 0,
            ranked_games: vec![(Queue::Solo, now - 27 * DAY)],
        }
    }

    /// Один кадр egui без окна (1100×700) с этими событиями.
    fn frame(ctx: &egui::Context, view: &mut ProfileView, events: Vec<egui::Event>) -> bool {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 700.0))),
            events,
            ..Default::default()
        };
        let account = account();
        let mut close = false;
        ctx.run_ui(input, |ui| close = view.show(ui.ctx(), &account))
            .drop_without_applying_deltas();
        close
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
    fn period_and_queue_narrow_the_games() {
        let mut view = ProfileView::new(1);
        view.games = Some(Ok(vec![
            game(1, 420, true, 1),
            game(2, 440, true, 10),
            game(3, 450, false, 40),
            game(1, 420, false, 200),
        ]));
        let now = unix_now();
        assert_eq!(view.visible(now).len(), 4);
        view.period = Period::Month;
        assert_eq!(view.visible(now).len(), 2);
        view.period = Period::Quarter;
        assert_eq!(view.visible(now).len(), 3);
        view.filter = Filter::Solo;
        assert_eq!(view.visible(now).len(), 1);
        view.period = Period::All;
        assert_eq!(view.visible(now).len(), 2);
    }

    /// Окно рисуется во всех состояниях; клавиши переключают очередь, период, сортировку
    /// и двигают курсор, список прокручивается за ним.
    #[test]
    fn keys_switch_views_and_move_the_cursor() {
        let ctx = egui::Context::default();
        let mut view = ProfileView::new(1);
        assert!(!frame(&ctx, &mut view, vec![]), "пока грузится — окно открыто");
        view.games = Some(Err(AppError::AccountMissing));
        view.inventory = Some(None);
        frame(&ctx, &mut view, vec![]);
        view.inventory = Some(Some(Inventory {
            champions: 75,
            skins: 56,
            blue_essence: 12_415,
            orange_essence: 29,
            rp: 0,
            fetched_at: unix_now(),
        }));
        // 40 чемпионов по одной игре.
        view.games = Some(Ok((0..40).map(|i| game(i, 420, i % 2 == 0, i64::from(i))).collect()));
        frame(&ctx, &mut view, vec![]);
        for _ in 0..30 {
            frame(&ctx, &mut view, vec![press(egui::Key::ArrowDown)]);
        }
        assert_eq!(view.cursor, 30);
        assert!(view.offset > 0.0, "список прокрутился вслед за курсором");
        frame(&ctx, &mut view, vec![press(egui::Key::End)]);
        assert_eq!(view.cursor, 39);

        frame(&ctx, &mut view, vec![press(egui::Key::Tab)]);
        assert_eq!((view.filter, view.cursor), (Filter::Solo, 0));
        frame(&ctx, &mut view, vec![press(egui::Key::ArrowLeft)]);
        assert_eq!(view.period, Period::Quarter, "← от «всё время» — на 90 дней");
        frame(&ctx, &mut view, vec![press(egui::Key::ArrowRight)]);
        assert_eq!(view.period, Period::All);
        frame(&ctx, &mut view, vec![press(egui::Key::S)]);
        assert_eq!(view.sort, ChampionSort::Winrate);
        // Период без игр — подсказка вместо таблицы, без паники.
        view.filter = Filter::Flex;
        frame(&ctx, &mut view, vec![]);

        assert!(frame(&ctx, &mut view, vec![press(egui::Key::Escape)]), "Esc закрывает окно");
    }
}
