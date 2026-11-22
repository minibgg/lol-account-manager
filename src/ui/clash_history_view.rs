//! Окно «прошлые Clash»: открывается щелчком по нику на вкладке Clash. Последние игры
//! игрока в Clash по дням турнира (чемпион, роль, результат, K/D/A) и на ком он играл
//! чаще всего. `o` — профиль на op.gg, `r` — загрузить заново, `esc` — закрыть.

use eframe::egui::{self, Align, Color32, Layout, Margin, Rect, RichText, Sense, pos2, vec2};

use crate::clash::{self, ClashHistory};
use crate::error::AppError;
use crate::lcu::client::LCU_CLASH_DEPTH;
use crate::lobby::{self, Position};
use crate::riot::models::MatchSummary;
use crate::riot::{Region, RiotId};
use crate::ui::format;
use crate::ui::match_view::{champion_name, duration};
use crate::ui::terminal::{self, FONT, job};
use crate::ui::theme::{self, Weight};

const WIDTH: f32 = 600.0;
const ROW: f32 = 22.0;
/// Колонки строки игры: результат, чемпион, роль, K/D/A; остальное — длительность.
const COLUMNS: [f32; 4] = [80.0, 170.0, 110.0, 100.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAction {
    None,
    Close,
    /// Загрузить заново (после ошибки или по `r`).
    Retry,
}

/// Рисует окно. `state`: `None` — игры ещё загружаются.
pub fn show(
    ctx: &egui::Context,
    riot_id: &RiotId,
    state: Option<&Result<ClashHistory, AppError>>,
    region: Option<Region>,
) -> HistoryAction {
    let mut action = HistoryAction::None;
    let mut open_profile = false;
    let (key_o, key_r) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::O),
            i.consume_key(egui::Modifiers::NONE, egui::Key::R),
        )
    });
    if key_r && state.is_some() {
        action = HistoryAction::Retry;
    }
    open_profile |= key_o;

    let height = (ctx.content_rect().height() - 260.0).max(ROW * 4.0);
    let modal = egui::Modal::new(egui::Id::unique("clash_history"))
        .frame(theme::dialog_frame().inner_margin(Margin::symmetric(20, 16)))
        .show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = 4.0;
            let font = theme::mono(FONT);
            ui.horizontal(|ui| {
                ui.label(RichText::new("прошлые clash").font(theme::mono(15.0)).color(theme::TEXT).semibold());
                let tag = format!("#{}", riot_id.tag_line);
                ui.label(job(&[(&riot_id.game_name, theme::TEXT), (&tag, theme::TEXT_MUTED)], &font));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if terminal::key_button(ui, "esc", "закрыть", false).clicked() {
                        action = HistoryAction::Close;
                    }
                    if region.is_some() && terminal::key_button(ui, "o", "op.gg", false).clicked() {
                        open_profile = true;
                    }
                });
            });
            ui.add_space(6.0);
            match state {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(job(&[("загружаю игры clash…", theme::TEXT_MUTED)], &font));
                    });
                }
                Some(Err(error)) => {
                    let text = format!("! {error}");
                    ui.label(job(&[(&text, theme::AMBER_TEXT)], &font));
                    ui.add_space(4.0);
                    if terminal::key_button(ui, "r", "ещё раз", false).clicked() {
                        action = HistoryAction::Retry;
                    }
                }
                Some(Ok(history)) => body(ui, history, height),
            }
        });
    if modal.should_close() {
        action = HistoryAction::Close;
    }
    if open_profile && let Some(region) = region {
        ctx.open_url(egui::OpenUrl::new_tab(lobby::opgg_profile_url(region, riot_id)));
    }
    action
}

fn body(ui: &mut egui::Ui, history: &ClashHistory, height: f32) {
    let font = theme::mono(FONT);
    if history.games.is_empty() {
        let text = if history.from_api {
            "игр в clash не нашлось".to_string()
        } else {
            format!("в последних {LCU_CLASH_DEPTH} играх из клиента League игр clash нет")
        };
        ui.label(job(&[(&text, theme::TEXT_MUTED)], &font));
    } else {
        favorites(ui, &history.games);
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("clash_history_games")
            .max_height(height)
            // Без этого область в окне берёт высоту прошлого кадра (пока шла загрузка)
            // и сжимается — см. то же в match_view.
            .min_scrolled_height(height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for day in clash::history_days(&history.games) {
                    let when = format::local_datetime(day.start());
                    let score = format!("  {}–{}", day.wins, day.losses);
                    ui.label(job(&[(&when, theme::TEXT_MUTED), (&score, theme::TEXT)], &font));
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for game in &day.games {
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
                        game_row(ui, rect, game);
                    }
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.add_space(8.0);
                }
            });
    }
    ui.add_space(4.0);
    footer(ui, history);
}

/// «чаще всего: дариус ×3 (67%), кайл ×1 (100%)».
fn favorites(ui: &mut egui::Ui, games: &[MatchSummary]) {
    let list: Vec<String> = clash::history_champions(games)
        .iter()
        .take(5)
        .map(|(name, games, wins)| format!("{} ×{games} ({}%)", name.to_lowercase(), wins * 100 / games))
        .collect();
    if list.is_empty() {
        return;
    }
    let list = list.join(", ");
    let mut text = job(&[("чаще всего: ", theme::TEXT_HINT), (&list, theme::TEXT)], &theme::mono(FONT));
    text.wrap.max_width = ui.available_width();
    ui.label(text);
}

/// «победа  дариус  топ  8/2/5  32:10».
fn game_row(ui: &egui::Ui, rect: Rect, game: &MatchSummary) {
    let painter = ui.painter_at(rect);
    let font = theme::mono(FONT);
    let (result, color) = match (game.remake, game.win) {
        (true, _) => ("ремейк", theme::TEXT_HINT),
        (false, true) => ("победа", theme::GREEN_TEXT),
        (false, false) => ("поражение", theme::RED_TEXT),
    };
    let champion = champion_name(&game.me).to_lowercase();
    let role = Position::from_lcu(&game.me.position).map_or_else(|| "—".to_string(), |p| p.label().to_lowercase());
    // Сегодняшний раунд из итогов команды: K/D/A там нет.
    let kda = if ClashHistory::is_brief(game) { "—".to_string() } else { game.kda() };
    let length = duration(game.duration_secs);
    let cells: [(&str, Color32); 5] = [
        (result, color),
        (&champion, theme::TEXT),
        (&role, theme::TEXT_MUTED),
        (&kda, theme::TEXT),
        (&length, theme::TEXT_HINT),
    ];
    let mut x = rect.left();
    for (index, (text, color)) in cells.into_iter().enumerate() {
        let width = COLUMNS.get(index).copied().unwrap_or(rect.right() - x) - 8.0;
        let mut cell = job(&[(text, color)], &font);
        cell.wrap = egui::text::TextWrapping::truncate_at_width(width.max(8.0));
        let galley = painter.layout_job(cell);
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
        x += COLUMNS.get(index).copied().unwrap_or(0.0);
    }
}

/// Откуда игры и чего в них может не хватать.
fn footer(ui: &mut egui::Ui, history: &ClashHistory) {
    let font = theme::mono(11.5);
    let note = if history.from_api {
        "старые игры — из riot api, свежие — из клиента League (у чужих игроков его история \
         неполная), сегодняшние раунды — из итогов команды (без k/d/a)"
    } else {
        "из клиента League: у чужих игроков его история неполная, сегодняшние раунды — из итогов \
         команды (без k/d/a). Старые игры Clash видны с ключом Riot API (настройки)."
    };
    let mut text = job(&[(note, theme::TEXT_HINT)], &font);
    text.wrap.max_width = ui.available_width();
    ui.label(text);
    if let Some(problem) = &history.api_problem {
        let text = format!("riot api не ответил: {problem}");
        let mut text = job(&[(&text, theme::AMBER_TEXT)], &font);
        text.wrap.max_width = ui.available_width();
        ui.label(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::models::PlayerStats;

    fn game(hour: i64, champion: &str, win: bool) -> MatchSummary {
        MatchSummary {
            id: format!("RU_{hour}"),
            win,
            remake: false,
            queue_id: 700,
            played_at: 1_795_000_000 + hour * 3600,
            duration_secs: 1900,
            stats_version: 0,
            me: PlayerStats {
                champion_id: 122,
                champion: champion.into(),
                kills: 8,
                deaths: 2,
                assists: 5,
                position: "top".into(),
                ..PlayerStats::default()
            },
        }
    }

    /// Окно рисуется во всех состояниях без паники; `r` после загрузки — «ещё раз».
    #[test]
    fn dialog_draws_in_every_state() {
        let ctx = egui::Context::default();
        let id = RiotId::parse("Игрок#RU1").unwrap();
        let loaded: Result<ClashHistory, AppError> = Ok(ClashHistory {
            games: vec![game(0, "Дариус", true), game(1, "Кайл", false), game(200, "Дариус", true)],
            from_api: false,
            api_problem: Some(AppError::NoDataSource),
        });
        let empty: Result<ClashHistory, AppError> = Ok(ClashHistory {
            games: Vec::new(),
            from_api: true,
            api_problem: None,
        });
        let failed: Result<ClashHistory, AppError> = Err(AppError::NoDataSource);
        for state in [None, Some(&loaded), Some(&empty), Some(&failed)] {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                let action = show(ui.ctx(), &id, state, Some(Region::Ru));
                assert_eq!(action, HistoryAction::None);
            })
            .drop_without_applying_deltas();
        }
        let press = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::R,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        ctx.run_ui(press, |ui| {
            assert_eq!(show(ui.ctx(), &id, Some(&failed), None), HistoryAction::Retry);
        })
        .drop_without_applying_deltas();
    }
}
