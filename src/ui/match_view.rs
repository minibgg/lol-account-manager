//! Игры: общие кусочки для подсказки на строке аккаунта и окно «Подробности игры»
//! (открывается щелчком по игре) — обе команды, K/D/A, урон, золото, CS, предметы.

use eframe::egui::{
    self, Align, Color32, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2, vec2,
};

use crate::error::AppError;
use crate::images::ImageKey;
use crate::riot::{MatchDetails, MatchSummary, PlayerLine, PlayerStats, RiotId, TeamDetails};
use crate::storage::AccountId;
use crate::ui::format;
use crate::ui::image_store::ImageStore;
use crate::ui::theme::{self, Weight, alpha, white};
use crate::ui::widgets::{self, Icon, IconButton, Tone};

// ----- Общие кусочки ----------------------------------------------------------

/// «Победа» / «Поражение» / «Ремейк» и цвет текста.
pub fn result_label(win: bool, remake: bool) -> (&'static str, Color32) {
    if remake {
        ("Ремейк", theme::TEXT_MUTED)
    } else if win {
        ("Победа", theme::GREEN_TEXT)
    } else {
        ("Поражение", theme::RED_TEXT)
    }
}

/// Цвет полоски результата под иконкой.
pub fn result_color(win: bool, remake: bool) -> Color32 {
    if remake {
        theme::TEXT_HINT
    } else if win {
        theme::GREEN
    } else {
        theme::RED
    }
}

/// Название чемпиона, а если неизвестно — номер.
pub fn champion_name(stats: &PlayerStats) -> String {
    if stats.champion.is_empty() {
        format!("чемпион #{}", stats.champion_id)
    } else {
        stats.champion.clone()
    }
}

/// «30:19».
pub fn duration(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// «32 145» — разряды через пробел, как принято по-русски.
pub fn number(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// KDA от 5 — золотой, от 3 — синий, иначе обычный.
pub fn kda_color(ratio: f32) -> Color32 {
    if ratio >= 5.0 {
        theme::AMBER_TEXT
    } else if ratio >= 3.0 {
        theme::ACCENT_TEXT
    } else {
        theme::TEXT_MUTED
    }
}

/// Иконка чемпиона в квадрате `rect` (пока картинка грузится — серая заглушка).
pub fn champion_icon(ui: &egui::Ui, images: &ImageStore, champion_id: i32, rect: Rect, dim: bool) {
    let radius = (rect.width() / 5.0) as u8;
    match images.get(ImageKey::Champion(champion_id)) {
        Some(image) => {
            let image = if dim { image.tint(Color32::from_white_alpha(110)) } else { image };
            image.corner_radius(radius).paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, radius, white(13));
        }
    }
}

/// Предметы в ряд (6 слотов и тринкет). Пустой слот — тёмный квадрат.
pub fn item_row(ui: &mut egui::Ui, images: &ImageStore, items: &[i32], size: f32) {
    const GAP: f32 = 2.0;
    let count = items.len().max(7);
    let width = count as f32 * size + (count - 1) as f32 * GAP;
    let (strip, _) = ui.allocate_exact_size(vec2(width, size), Sense::hover());
    for index in 0..count {
        let rect = Rect::from_min_size(pos2(strip.left() + index as f32 * (size + GAP), strip.top()), Vec2::splat(size));
        let item = items.get(index).copied().unwrap_or(0);
        match (item > 0).then(|| images.get(ImageKey::Item(item))).flatten() {
            Some(image) => image.corner_radius(3).paint_at(ui, rect),
            None => {
                ui.painter().rect_filled(rect, 3, white(8));
            }
        }
        ui.painter().rect_stroke(rect, 3, Stroke::new(1.0, white(18)), StrokeKind::Inside);
    }
}

// ----- Окно «Подробности игры» --------------------------------------------------

/// Открытое окно подробностей.
pub struct MatchDialog {
    pub account: AccountId,
    /// Чья это игра (для заголовка).
    pub owner: RiotId,
    /// Краткие данные — заголовок виден сразу, пока грузятся подробности.
    pub summary: MatchSummary,
    /// `None` — ещё грузится.
    pub details: Option<Result<MatchDetails, AppError>>,
}

/// Что сделать после кадра.
pub enum DialogAction {
    None,
    Close,
    /// Повторить загрузку после ошибки.
    Retry,
}

const COL_ICON: f32 = 30.0;
const COL_NAME: f32 = 176.0;
const COL_KDA: f32 = 104.0;
const COL_DAMAGE: f32 = 124.0;
const COL_GOLD: f32 = 62.0;
const COL_CS: f32 = 70.0;
const ITEM_SIZE: f32 = 20.0;
const DIALOG_WIDTH: f32 = 790.0;

impl MatchDialog {
    pub fn show(&self, ctx: &egui::Context, images: &ImageStore) -> DialogAction {
        let mut action = DialogAction::None;
        let modal = egui::Modal::new(egui::Id::unique("match_details"))
            .frame(theme::dialog_frame())
            .show(ctx, |ui| {
                ui.set_width(DIALOG_WIDTH);
                if self.header(ui, images) {
                    action = DialogAction::Close;
                }
                ui.add_space(12.0);
                match &self.details {
                    None => {
                        ui.horizontal(|ui| {
                            widgets::spinner(ui, 16.0);
                            ui.label(RichText::new("Загружаю подробности игры…").size(12.5).color(theme::TEXT_MUTED));
                        });
                        ui.add_space(8.0);
                    }
                    Some(Err(error)) => {
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                            widgets::paint_icon(ui.painter(), rect, Icon::Warning, theme::RED_TEXT);
                            ui.add(egui::Label::new(RichText::new(error.to_string()).size(12.5).color(theme::RED_TEXT)).wrap());
                        });
                        ui.add_space(10.0);
                        if ui.add(widgets::Button::outline("Повторить").icon(Icon::Refresh)).clicked() {
                            action = DialogAction::Retry;
                        }
                    }
                    Some(Ok(details)) => {
                        // Окно приложения может быть низким — тогда команды прокручиваются.
                        // Высоту задаём явно: иначе область прокрутки берёт место, которое
                        // осталось в окне с прошлого кадра (пока шла загрузка, окно было
                        // маленьким) — и остаётся полоской. `min_scrolled_height` заставляет
                        // её вырасти до `height`, `auto_shrink` — не больше содержимого.
                        let height = (ctx.content_rect().height() - 190.0).max(160.0);
                        egui::ScrollArea::vertical()
                            .max_height(height)
                            .min_scrolled_height(height)
                            .auto_shrink([false, true])
                            .show(ui, |ui| teams(ui, details, images));
                    }
                }
            });
        if modal.should_close() {
            action = DialogAction::Close;
        }
        action
    }

    /// «Победа · Корки» и строка с режимом, длительностью и временем. Возвращает true,
    /// если нажали «закрыть».
    fn header(&self, ui: &mut egui::Ui, images: &ImageStore) -> bool {
        let game = &self.summary;
        let mut close = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let (icon, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
            champion_icon(ui, images, game.me.champion_id, icon, game.remake);
            ui.painter().rect_stroke(
                icon,
                8,
                Stroke::new(2.0, result_color(game.win, game.remake)),
                StrokeKind::Inside,
            );
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let (result, color) = result_label(game.win, game.remake);
                    ui.label(RichText::new(result).size(16.0).semibold().color(color));
                    ui.label(RichText::new(champion_name(&game.me)).size(16.0).semibold().color(theme::TEXT));
                    widgets::tag(ui, &self.owner.to_string(), Tone::Neutral);
                });
                ui.label(
                    RichText::new(format!(
                        "{} · {} · {}",
                        game.queue_label(),
                        duration(game.duration_secs),
                        format::relative_time(game.played_at)
                    ))
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                close = ui.add(IconButton::new(Icon::Close)).on_hover_text("Закрыть (Esc)").clicked();
            });
        });
        close
    }
}

/// Две команды одна под другой (своя первой).
fn teams(ui: &mut egui::Ui, details: &MatchDetails, images: &ImageStore) {
    // Полоски урона — относительно лучшего урона в игре.
    let max_damage = details.teams.iter().flat_map(|t| &t.players).map(|p| p.stats.damage).max().unwrap_or(0).max(1);
    for (index, team) in details.teams.iter().enumerate() {
        if index > 0 {
            ui.add_space(12.0);
        }
        team_table(ui, team, details, max_damage, images);
    }
}

fn team_table(ui: &mut egui::Ui, team: &TeamDetails, details: &MatchDetails, max_damage: u32, images: &ImageStore) {
    let kills = team.kills();
    // Заголовок команды: «ВАША КОМАНДА · ПОБЕДА» и итоги справа.
    ui.horizontal(|ui| {
        let who = if team.is_mine() { "Ваша команда" } else { "Соперники" };
        let (result, color) = if details.remake {
            ("ремейк", theme::TEXT_MUTED)
        } else if team.win {
            ("победа", theme::GREEN_TEXT)
        } else {
            ("поражение", theme::RED_TEXT)
        };
        ui.label(theme::caps(&format!("{who} · {result}")).color(color));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!(
                    "убийства {kills} · башни {} · драконы {} · бароны {}",
                    team.towers, team.dragons, team.barons
                ))
                .size(11.5)
                .color(theme::TEXT_MUTED),
            );
        });
    });
    ui.add_space(4.0);
    column_titles(ui);
    for player in &team.players {
        player_row(ui, player, kills, details.duration_secs, max_damage, images);
    }
}

/// Подписи столбцов над игроками.
fn column_titles(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(10.0);
        let title = |ui: &mut egui::Ui, width: f32, text: &str| {
            ui.allocate_ui_with_layout(vec2(width, 14.0), Layout::left_to_right(Align::Center), |ui| {
                ui.set_width(width);
                ui.label(theme::caps(text));
            });
        };
        title(ui, COL_ICON + COL_NAME + 8.0, "Игрок");
        title(ui, COL_KDA, "K / D / A");
        title(ui, COL_DAMAGE, "Урон");
        title(ui, COL_GOLD, "Золото");
        title(ui, COL_CS, "CS");
        title(ui, 160.0, "Предметы");
    });
}

fn player_row(
    ui: &mut egui::Ui,
    player: &PlayerLine,
    team_kills: u32,
    duration_secs: u32,
    max_damage: u32,
    images: &ImageStore,
) {
    let stats = &player.stats;
    let fill = if player.is_me { alpha(theme::ACCENT, 30) } else { white(5) };
    let stroke = if player.is_me { alpha(theme::ACCENT_LIGHT, 70) } else { Color32::TRANSPARENT };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(10, 4))
        .outer_margin(Margin::symmetric(0, 2))
        .show(ui, |ui| {
            ui.spacing_mut().interact_size.y = 0.0;
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 32.0), Layout::left_to_right(Align::Center), |ui| {
                ui.set_min_height(32.0);
                ui.spacing_mut().item_spacing.x = 8.0;

                // Иконка чемпиона с уровнем в углу.
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(COL_ICON), Sense::hover());
                champion_icon(ui, images, stats.champion_id, rect, false);
                if stats.champ_level > 0 {
                    let galley = widgets::text_galley(ui, RichText::new(stats.champ_level.to_string()).size(8.5).semibold());
                    let badge = Rect::from_min_size(
                        pos2(rect.right() - galley.size().x - 4.0, rect.bottom() - 11.0),
                        vec2(galley.size().x + 4.0, 11.0),
                    );
                    ui.painter().rect_filled(badge, 3, theme::BADGE_BG);
                    ui.painter().galley(badge.center() - galley.size() / 2.0, galley, theme::TEXT);
                }

                // Имя и чемпион.
                cell(ui, COL_NAME, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let name = match &player.riot_id {
                            Some(id) => RichText::new(&id.game_name).color(theme::TEXT),
                            None => RichText::new("Скрытый игрок").italics().color(theme::TEXT_MUTED),
                        };
                        ui.add(egui::Label::new(name.size(12.5).medium()).truncate());
                        if player.is_me {
                            ui.add_space(6.0);
                            widgets::pill(ui, "вы", Tone::Accent);
                        }
                    });
                    ui.label(RichText::new(champion_name(stats)).size(11.0).color(theme::TEXT_MUTED));
                });

                // K/D/A, коэффициент и участие в убийствах.
                cell(ui, COL_KDA, |ui| {
                    ui.label(RichText::new(stats.kda()).size(12.5).semibold().color(theme::TEXT));
                    let mut line = format!("KDA {:.2}", stats.kda_ratio());
                    if let Some(kp) = player.kill_participation(team_kills) {
                        line.push_str(&format!(" · {kp:.0}%"));
                    }
                    ui.label(RichText::new(line).size(10.5).color(kda_color(stats.kda_ratio())))
                        .on_hover_text("KDA = (убийства + помощь) / смерти; % — участие в убийствах команды");
                });

                // Урон с полоской.
                cell(ui, COL_DAMAGE, |ui| {
                    ui.label(RichText::new(number(stats.damage)).size(12.0).color(theme::TEXT));
                    let (bar, _) = ui.allocate_exact_size(vec2(COL_DAMAGE - 10.0, 4.0), Sense::hover());
                    ui.painter().rect_filled(bar, 2, white(16));
                    let share = stats.damage as f32 / max_damage as f32;
                    let filled = Rect::from_min_size(bar.min, vec2(bar.width() * share, bar.height()));
                    let color = if player.is_me { theme::ACCENT_LIGHT } else { theme::RED_TEXT.gamma_multiply(0.8) };
                    ui.painter().rect_filled(filled, 2, color);
                });

                cell(ui, COL_GOLD, |ui| {
                    ui.label(RichText::new(number(stats.gold)).size(12.0).color(theme::AMBER_TEXT));
                });

                cell(ui, COL_CS, |ui| {
                    ui.label(RichText::new(stats.cs.to_string()).size(12.0).color(theme::TEXT));
                    ui.label(
                        RichText::new(format!("{:.1}/мин", stats.cs_per_minute(duration_secs)))
                            .size(10.5)
                            .color(theme::TEXT_MUTED),
                    );
                });

                item_row(ui, images, &stats.items, ITEM_SIZE);
                // Лучшая серия убийств: «×3» — тройное и т. д.
                if let Some(label) = stats.multikill_label() {
                    widgets::pill(ui, &format!("×{}", stats.largest_multi_kill.min(5)), Tone::Amber)
                        .on_hover_text(label);
                }
            });
        });
}

/// Столбец фиксированной ширины: содержимое — сверху вниз, по центру строки.
fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(vec2(width, 32.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 1.0;
        add(ui);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::models::MATCH_STATS_VERSION;

    fn stats(champion_id: i32) -> PlayerStats {
        PlayerStats {
            champion_id,
            champ_level: 16,
            kills: 5,
            deaths: 3,
            assists: 7,
            cs: 180,
            gold: 11_000,
            damage: 20_000,
            items: vec![3031, 0, 0, 0, 0, 0, 3340],
            ..Default::default()
        }
    }

    fn details() -> MatchDetails {
        let team = |win: bool, first: i32| TeamDetails {
            win,
            towers: 5,
            dragons: 2,
            barons: 1,
            players: (0..5)
                .map(|i| PlayerLine {
                    riot_id: RiotId::parse(&format!("Игрок{}#RU1", first + i)).ok(),
                    is_me: first + i == 1,
                    stats: stats(first + i),
                })
                .collect(),
        };
        MatchDetails {
            id: "RU_1".into(),
            queue_id: 420,
            played_at: 0,
            duration_secs: 2005,
            remake: false,
            teams: vec![team(true, 1), team(false, 6)],
        }
    }

    fn dialog() -> MatchDialog {
        MatchDialog {
            account: 1,
            owner: RiotId::parse("Игрок1#RU1").unwrap(),
            summary: MatchSummary {
                id: "RU_1".into(),
                win: true,
                remake: false,
                queue_id: 420,
                played_at: 0,
                duration_secs: 2005,
                stats_version: MATCH_STATS_VERSION,
                me: stats(1),
            },
            details: None,
        }
    }

    /// Окно сначала маленькое («загружаю…»), потом приходят подробности — оно должно
    /// вырасти и показать игроков, а не остаться полоской (так было в окне ~1160×530).
    #[test]
    fn dialog_grows_when_details_arrive() {
        const SCREEN: Vec2 = vec2(1160.0, 530.0);
        let ctx = egui::Context::default();
        let images = ImageStore::default();
        let mut dialog = dialog();
        let frame = |dialog: &MatchDialog| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), SCREEN)),
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                dialog.show(ui.ctx(), &images);
            })
            .drop_without_applying_deltas();
        };
        for _ in 0..3 {
            frame(&dialog);
        }
        dialog.details = Some(Ok(details()));
        for _ in 0..5 {
            frame(&dialog);
        }
        let rect = ctx
            .memory(|m| m.area_rect(egui::Id::unique("match_details")))
            .expect("окно подробностей показано");
        assert!(rect.height() > 400.0, "окно подробностей сжалось: {rect:?}");
        assert!(rect.top() >= -1.0 && rect.bottom() <= SCREEN.y + 1.0, "вылезает за экран: {rect:?}");
    }

    #[test]
    fn formats_numbers_and_durations() {
        assert_eq!(number(0), "0");
        assert_eq!(number(999), "999");
        assert_eq!(number(15_200), "15 200");
        assert_eq!(number(1_234_567), "1 234 567");
        assert_eq!(duration(1819), "30:19");
        assert_eq!(duration(67), "1:07");
        assert_eq!(result_label(true, true).0, "Ремейк");
        assert_eq!(result_label(false, false).0, "Поражение");
    }
}
