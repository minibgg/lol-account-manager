//! Карточка «после игры» в правом нижнем углу: результат, чемпион, K/D/A и сколько LP
//! получено — полоска LP плавно заполняется от старого значения к новому.
//!
//! Появляется, когда обновился аккаунт, в который выполнен вход в клиенте League,
//! и у него новая игра или изменился ранг (после игры аккаунт обновляется сам).

use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Align2, Color32, Layout, Margin, Rect, RichText, Sense, Shadow, Stroke, StrokeKind, vec2};

use crate::history::{self, Queue, RankChange};
use crate::riot::{MatchSummary, RiotId};
use crate::storage::{Account, AccountId};
use crate::ui::image_store::ImageStore;
use crate::ui::theme::{self, Weight, alpha, white};
use crate::ui::widgets::{self, Icon, IconButton, Tone};
use crate::ui::{account_card, match_view, rank_history};

/// Сколько карточка видна (пока на неё не навели курсор).
const SHOW_FOR: Duration = Duration::from_secs(14);
/// Появление и исчезновение.
const FADE: Duration = Duration::from_millis(350);
/// Сколько «едет» полоска LP.
const ANIMATION: Duration = Duration::from_millis(1600);
const WIDTH: f32 = 340.0;

pub struct PostGame {
    pub account: AccountId,
    owner: RiotId,
    /// Самая новая из новых игр.
    game: Option<MatchSummary>,
    /// Сколько новых игр с прошлого обновления.
    new_games: usize,
    change: Option<RankChange>,
    shown_at: Instant,
    /// Когда началась анимация LP.
    change_at: Instant,
}

impl PostGame {
    /// Что нового у аккаунта после обновления; `None` — ничего (или сравнивать не с чем).
    pub fn detect(old: &Account, new: &Account) -> Option<Self> {
        let (old_cache, new_cache) = (old.cache.as_ref()?, new.cache.as_ref()?);
        let changes = history::rank_changes(&old_cache.ranked, &new_cache.ranked);
        // Новые игры — те, что идут в списке раньше первой уже известной.
        let new_games: Vec<&MatchSummary> = match (&old_cache.matches, &new_cache.matches) {
            (Some(known), Some(fresh)) => fresh
                .iter()
                .take_while(|game| !known.iter().any(|k| k.id == game.id))
                .collect(),
            _ => Vec::new(),
        };
        if changes.is_empty() && new_games.is_empty() {
            return None;
        }
        let game = new_games.first().map(|g| (*g).clone());
        // Изменение той очереди, в которой сыграна игра; иначе — первое.
        let queue = game.as_ref().and_then(|g| Queue::from_queue_id(g.queue_id));
        let change = changes
            .iter()
            .find(|c| Some(c.queue) == queue)
            .or(changes.first())
            .cloned();
        let now = Instant::now();
        Some(Self {
            account: new.id,
            owner: new.riot_id.clone(),
            game,
            new_games: new_games.len(),
            change,
            shown_at: now,
            change_at: now,
        })
    }

    /// Ещё одно обновление того же аккаунта: клиент League иногда сначала отдаёт новые LP,
    /// а игру в истории — через несколько секунд. Собираем всё в одну карточку.
    pub fn merge(&mut self, newer: Self) {
        if newer.game.is_some() {
            self.game = newer.game;
            self.new_games = self.new_games.max(newer.new_games);
        }
        if newer.change.is_some() {
            self.change = newer.change;
            self.change_at = Instant::now();
        }
        self.shown_at = Instant::now();
    }

    pub fn expired(&self) -> bool {
        self.shown_at.elapsed() >= SHOW_FOR
    }

    /// Рисует карточку. Возвращает true, когда её пора убрать (закрыли или время вышло).
    pub fn show(&mut self, ctx: &egui::Context, images: &ImageStore) -> bool {
        let shown_for = self.shown_at.elapsed();
        if shown_for >= SHOW_FOR {
            return true;
        }
        let left = SHOW_FOR - shown_for;
        // Появление: прозрачность и сдвиг снизу; исчезновение — только прозрачность.
        let appear = (shown_for.as_secs_f32() / FADE.as_secs_f32()).min(1.0);
        let opacity = appear.min(left.as_secs_f32() / FADE.as_secs_f32()).min(1.0);
        let animating = self.change_at.elapsed() < ANIMATION;
        if appear < 1.0 || animating || left < FADE {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(left - FADE);
        }

        let (title, color) = self.title();
        let mut close = false;
        let area = egui::Area::new(egui::Id::unique("post_game"))
            .order(egui::Order::Foreground)
            .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -72.0 + (1.0 - ease_out(appear)) * 16.0))
            .show(ctx, |ui| {
                ui.multiply_opacity(opacity);
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(0x0D, 0x13, 0x20, 250))
                    .stroke(Stroke::new(1.0, alpha(color, 90)))
                    .corner_radius(12)
                    .inner_margin(Margin::same(14))
                    .shadow(Shadow {
                        offset: [0, 10],
                        blur: 32,
                        spread: 0,
                        color: Color32::from_black_alpha(160),
                    })
                    .show(ui, |ui| {
                        ui.set_width(WIDTH);
                        close = self.game_row(ui, images, &title, color);
                        if let Some(change) = &self.change {
                            ui.add_space(10.0);
                            let (line, _) = ui.allocate_exact_size(vec2(WIDTH, 1.0), Sense::hover());
                            ui.painter().rect_filled(line, 0, white(12));
                            ui.add_space(10.0);
                            rank_row(ui, images, change, self.change_at.elapsed());
                        }
                    });
            });
        // Курсор на карточке — не убираем её, пока человек читает.
        if area.response.contains_pointer() {
            self.shown_at = Instant::now() - FADE;
        }
        close
    }

    /// Заголовок и его цвет: результат игры, «Новые игры: 3» или «Ранг обновился».
    fn title(&self) -> (String, Color32) {
        if let Some(change) = &self.change
            && change.before.is_none()
        {
            return ("Калибровка завершена".into(), theme::AMBER_TEXT);
        }
        match (&self.game, self.new_games) {
            (Some(game), 1) => {
                let (text, color) = match_view::result_label(game.win, game.remake);
                (text.into(), color)
            }
            (_, n) if n > 1 => (format!("Новые игры: {n}"), theme::ACCENT_TEXT),
            _ => ("Ранг обновился".into(), theme::ACCENT_TEXT),
        }
    }

    /// Иконка чемпиона, результат, K/D/A и чей аккаунт. Возвращает true — «закрыть».
    fn game_row(&self, ui: &mut egui::Ui, images: &ImageStore, title: &str, color: Color32) -> bool {
        let mut close = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            match &self.game {
                Some(game) => {
                    let (icon, _) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::hover());
                    match_view::champion_icon(ui, images, game.me.champion_id, icon, game.remake);
                    ui.painter().rect_stroke(icon, 9, Stroke::new(2.0, color), StrokeKind::Inside);
                }
                None => {
                    widgets::icon_tile(ui, Icon::Refresh, Tone::Accent, 46.0);
                }
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new(title).size(15.0).semibold().color(color));
                    if let Some(game) = &self.game {
                        ui.label(RichText::new(game.queue_label()).size(11.5).color(theme::TEXT_MUTED));
                    }
                });
                if let Some(game) = &self.game {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(
                            RichText::new(match_view::champion_name(&game.me))
                                .size(12.5)
                                .medium()
                                .color(theme::TEXT),
                        );
                        ui.label(RichText::new(game.kda()).size(12.5).semibold().color(theme::TEXT));
                        let ratio = game.me.kda_ratio();
                        ui.label(
                            RichText::new(format!("KDA {ratio:.1}"))
                                .size(11.5)
                                .color(match_view::kda_color(ratio)),
                        );
                    });
                }
                ui.label(RichText::new(self.owner.to_string()).size(11.0).color(theme::TEXT_HINT));
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                close = ui.add(IconButton::new(Icon::Close).size(22.0)).on_hover_text("Закрыть").clicked();
            });
        });
        close
    }
}

/// Плавное замедление к концу: быстро стартует, мягко останавливается.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Эмблема, «Золото II · 54 LP», полоска LP и «+19 LP» — с анимацией от старого значения.
fn rank_row(ui: &mut egui::Ui, images: &ImageStore, change: &RankChange, elapsed: Duration) {
    let after = &change.after;
    let to = history::ladder(after);
    let from = change.before.as_ref().map_or(to, history::ladder);
    let t = ease_out(elapsed.as_secs_f32() / ANIMATION.as_secs_f32());
    let value = from as f32 + (to - from) as f32 * t;
    let tier_color = theme::tier_color(Some(after.tier));

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (emblem_rect, _) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::hover());
        // Повышение — свечение цветом нового тира за эмблемой.
        if change.promotion() == Some(true) {
            widgets::radial_glow(ui.painter(), emblem_rect.center(), 34.0, alpha(tier_color, (90.0 * t) as u8));
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(emblem_rect));
        account_card::emblem(&mut child, images, Some(after.tier), true, 46.0);

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(after.rank_label()).size(13.5).semibold().color(tier_color));
                ui.label(RichText::new(format!("{} LP", after.lp)).size(12.0).color(theme::TEXT));
                match change.promotion() {
                    Some(true) => {
                        widgets::tag(ui, "Повышение!", Tone::Amber);
                    }
                    Some(false) => {
                        widgets::tag(ui, "Понижение", Tone::Red);
                    }
                    None => {}
                }
            });
            // Полоска — сколько LP внутри дивизиона. Переходя через границу дивизиона,
            // она доезжает до конца и начинается заново. У Мастера и выше дивизионов нет.
            if !after.tier.is_apex() {
                let (bar, _) = ui.allocate_exact_size(vec2(180.0, 6.0), Sense::hover());
                let painter = ui.painter();
                painter.rect_filled(bar, 3, white(14));
                let within = value.rem_euclid(100.0) / 100.0;
                let fill = Rect::from_min_size(bar.min, vec2(bar.width() * within, bar.height()));
                painter.rect_filled(fill, 3, tier_color);
            }
        });

        if let Some(before) = &change.before {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Число «набегает» вместе с полоской.
                let shown = (value - history::ladder(before) as f32).round() as i32;
                ui.label(
                    RichText::new(history::signed_lp(shown))
                        .size(19.0)
                        .semibold()
                        .color(rank_history::lp_color(to - from)),
                );
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::{Division, PlayerStats, RankedQueues, RankedStats, Region, Tier};
    use crate::storage::CachedProfile;

    fn account(lp: i32, wins: u32, games: &[&str]) -> Account {
        let game = |id: &&str| MatchSummary {
            id: id.to_string(),
            win: true,
            remake: false,
            queue_id: 420,
            played_at: 0,
            duration_secs: 1800,
            stats_version: crate::riot::models::MATCH_STATS_VERSION,
            me: PlayerStats::default(),
        };
        Account {
            id: 3,
            riot_id: RiotId::parse("Name#RU1").unwrap(),
            region: Region::Ru,
            puuid: None,
            lcu_puuid: None,
            note: String::new(),
            tags: vec![],
            secret_ref: String::new(),
            has_login: false,
            has_password: false,
            created_at: 0,
            cache: Some(CachedProfile {
                profile_icon_id: 1,
                summoner_level: 30,
                ranked: RankedQueues {
                    solo: Some(RankedStats {
                        tier: Tier::Gold,
                        division: Division::Two,
                        lp,
                        wins,
                        losses: Some(5),
                    }),
                    flex: None,
                },
                matches: Some(games.iter().map(game).collect()),
                fetched_at: 0,
            }),
            history: vec![],
            archived_games: 0,
        }
    }

    #[test]
    fn detects_new_game_and_merges_late_lp() {
        let old = account(40, 10, &["RU_1", "RU_0"]);
        assert!(PostGame::detect(&old, &old).is_none(), "ничего нового");

        // Клиент сначала отдал игру (LP ещё старые)…
        let with_game = account(40, 10, &["RU_2", "RU_1", "RU_0"]);
        let mut card = PostGame::detect(&old, &with_game).unwrap();
        assert_eq!(card.new_games, 1);
        assert_eq!(card.game.as_ref().map(|g| g.id.as_str()), Some("RU_2"));
        assert!(card.change.is_none());
        assert_eq!(card.title().0, "Победа");

        // …а через несколько секунд — новые LP: та же карточка, теперь с +19.
        let with_lp = account(59, 11, &["RU_2", "RU_1", "RU_0"]);
        card.merge(PostGame::detect(&with_game, &with_lp).unwrap());
        let progress = card.change.as_ref().and_then(|c| c.progress()).unwrap();
        assert_eq!((progress.lp, progress.wins), (19, 1));
        assert_eq!(card.game.as_ref().map(|g| g.id.as_str()), Some("RU_2"), "игра осталась");
    }
}
