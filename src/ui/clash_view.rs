//! Clash на вкладке «Лобби» (пока турнир идёт): соперник в текущем раунде и ваша команда —
//! таблицами с позициями, рангами и чемпионами из разведки. Щелчок по строке — профиль
//! игрока на op.gg; `c` — скопировать ники соперника, `o` — мультипоиск op.gg по нему
//! (если соперника ещё нет — по своей команде).

use eframe::egui::{self, Color32, CursorIcon, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::clash::{ClashPhase, ClashPlayer, ClashState, ClashTeam};
use crate::lobby;
use crate::riot::{Region, RiotId};
use crate::storage::unix_now;
use crate::ui::lobby_view::rank_cell;
use crate::ui::terminal::{FONT, ROW_HEIGHT, job};
use crate::ui::theme::{self, white};

/// Ширины колонок: позиция, игрок, solo/duo, flex; остальное — чемпионы.
const COLUMNS: [f32; 4] = [86.0, 250.0, 180.0, 170.0];
const HEADERS: [&str; 5] = ["поз", "игрок", "solo/duo", "flex", "чемпионы сезона"];

/// Riot ID команды, к которой относятся `c` / `o`.
pub fn riot_ids(state: &ClashState) -> Vec<RiotId> {
    state
        .target_team()
        .map(|team| team.players.iter().filter_map(|p| p.riot_id.clone()).collect())
        .unwrap_or_default()
}

/// Скопировать ники через запятую. Возвращает сообщение для нижней строки.
pub fn copy_names(ctx: &egui::Context, state: &ClashState) -> Result<&'static str, &'static str> {
    let ids = riot_ids(state);
    if ids.is_empty() {
        return Err("Ников пока нет: команда Clash не найдена");
    }
    let names: Vec<String> = ids.iter().map(ToString::to_string).collect();
    ctx.copy_text(names.join(", "));
    Ok(if state.opponent.is_some() { "Ники соперника скопированы" } else { "Ники команды скопированы" })
}

/// Открыть всю команду на op.gg одной ссылкой.
pub fn open_multisearch(ctx: &egui::Context, state: &ClashState, region: Option<Region>) -> Result<&'static str, &'static str> {
    let ids = riot_ids(state);
    match region {
        _ if ids.is_empty() => Err("Ников пока нет: команда Clash не найдена"),
        None => Err("Сервер неизвестен: клиент League не подключён"),
        Some(region) => {
            ctx.open_url(egui::OpenUrl::new_tab(lobby::opgg_multisearch_url(region, &ids)));
            Ok("Мультипоиск op.gg открыт в браузере")
        }
    }
}

/// Рисует вкладку. Возвращает true, если щелчком по игроку открыли его профиль.
pub fn show(ui: &mut egui::Ui, state: &ClashState, region: Option<Region>) -> bool {
    header(ui, state);
    if let Some(problem) = &state.problem {
        ui.label(RichText::new(format!("! {problem}")).font(theme::mono(12.0)).color(theme::AMBER_TEXT));
    }
    ui.add_space(10.0);

    let Some(team) = &state.team else {
        empty_state(ui, state.phase);
        return false;
    };
    let mut opened = false;
    egui::ScrollArea::vertical()
        .id_salt("clash_page")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            match &state.opponent {
                Some(opponent) => {
                    section_title(ui, "соперник", opponent, theme::RED_TEXT);
                    opened |= table(ui, opponent, region, "clash_opponent");
                    ui.add_space(14.0);
                }
                None if state.opponent_pending => {
                    note(ui, "соперник ещё не определился — ждём результата другого матча");
                    ui.add_space(10.0);
                }
                None if state.bracket_size == 0 => {
                    note(ui, "сетки ещё нет: соперник появится, когда закроется регистрация и начнётся турнир");
                    ui.add_space(10.0);
                }
                None => {}
            }
            section_title(ui, "ваша команда", team, theme::ACCENT_TEXT);
            opened |= table(ui, team, region, "clash_team");
        });
    opened
}

/// «клеш  разведка  · раунд 2 · через 12 мин · счёт 1–0».
fn header(ui: &mut egui::Ui, state: &ClashState) {
    let font = theme::mono(12.5);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.label(RichText::new("клеш").font(font.clone()).color(theme::TEXT));
        let color = if state.phase.is_active() { theme::ACCENT_TEXT } else { theme::TEXT_MUTED };
        ui.label(RichText::new(state.phase.label()).font(font.clone()).color(color));
        let mut extra = Vec::new();
        if let Some(round) = state.round {
            extra.push(format!("раунд {round}"));
        }
        if let Some(start) = state.round_start {
            let left = start - unix_now();
            if left > 60 {
                extra.push(format!("через {} мин", left / 60));
            }
        }
        if state.bracket_size > 0 {
            extra.push(format!("счёт {}–{}", state.wins, state.losses));
        }
        if !extra.is_empty() {
            ui.label(RichText::new(extra.join(" · ")).font(font.clone()).color(theme::TEXT_MUTED));
        }
    });
    let subtitle = match state.phase {
        ClashPhase::NoClient => "клиент League не запущен",
        ClashPhase::NotRegistered => "аккаунт из клиента не зарегистрирован на турнир",
        _ if state.opponent.is_some() => "соперник в ближайшем раунде и ваша команда",
        _ => "ваша команда",
    };
    ui.label(RichText::new(subtitle).font(theme::mono(11.5)).color(theme::TEXT_HINT));
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(format!("· {text}")).font(theme::mono(12.0)).color(theme::TEXT_MUTED));
}

/// Нет команды: клиент не запущен или регистрации нет.
fn empty_state(ui: &mut egui::Ui, phase: ClashPhase) {
    let (mark, color, title, text) = match phase {
        ClashPhase::NoClient => (
            "○",
            theme::AMBER_TEXT,
            "клиент League не запущен",
            "Запустите клиент — здесь появятся ваша команда Clash и соперник по сетке.",
        ),
        _ => (
            "·",
            theme::TEXT_MUTED,
            "нет команды Clash",
            "Зарегистрируйтесь на турнир в клиенте. Когда сформируется сетка, здесь появится \
             соперник ближайшего раунда: ники, позиции, ранги и чемпионы из разведки.",
        ),
    };
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let font = theme::mono(FONT);
            ui.label(job(&[(mark, color), (" ", color), (title, theme::TEXT)], &font));
            ui.add(egui::Label::new(RichText::new(text).font(theme::mono(12.0)).color(theme::TEXT_MUTED)).wrap());
        });
}

/// «соперник  big time rush [btr]».
fn section_title(ui: &mut egui::Ui, title: &str, team: &ClashTeam, color: Color32) {
    let font = theme::mono(12.5);
    let mut parts = vec![(title, color), ("  ", color), (team.name.as_str(), theme::TEXT)];
    let tag = format!(" [{}]", team.short_name);
    if !team.short_name.is_empty() {
        parts.push((tag.as_str(), theme::TEXT_MUTED));
    }
    ui.label(job(&parts, &font));
    ui.add_space(4.0);
}

/// Таблица игроков команды. Возвращает true, если по щелчку открыли профиль.
fn table(ui: &mut egui::Ui, team: &ClashTeam, region: Option<Region>, id: &str) -> bool {
    let font = theme::mono(FONT);
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let painter = ui.painter();
    let mut x = head.left();
    for (index, title) in HEADERS.iter().enumerate() {
        painter.text(pos2(x, head.center().y), egui::Align2::LEFT_CENTER, *title, font.clone(), theme::TEXT_HINT);
        x += COLUMNS.get(index).copied().unwrap_or(0.0);
    }
    painter.hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::BORDER));
    ui.add_space(4.0);

    let mut opened = false;
    ui.push_id(id, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for player in &team.players {
            let link = match (region, &player.riot_id) {
                (Some(region), Some(id)) => Some(lobby::opgg_profile_url(region, id)),
                _ => None,
            };
            let sense = if link.is_some() { Sense::click() } else { Sense::hover() };
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), sense);
            row(ui, rect, player, link.is_some() && response.hovered());
            if let Some(url) = link {
                let response = response
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text("Открыть профиль на op.gg");
                if response.clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    opened = true;
                }
            }
        }
    });
    opened
}

/// «ривен 9 · 56%, физз 4 · 75%»; без игр в сезоне — по мастерству: «кассадин, катарина».
fn champions_text(player: &ClashPlayer) -> (String, Color32) {
    if player.champions.is_empty() {
        return ("—".into(), theme::TEXT_HINT);
    }
    let by_mastery = player.champions.iter().all(|c| c.games == 0);
    let list: Vec<String> = player
        .champions
        .iter()
        .map(|c| {
            let name = c.name.to_lowercase();
            if c.games == 0 {
                name
            } else {
                format!("{name} {} · {}%", c.games, c.wins * 100 / c.games)
            }
        })
        .collect();
    let text = list.join(", ");
    if by_mastery {
        (format!("{text} (мастерство)"), theme::TEXT_MUTED)
    } else {
        (text, theme::TEXT)
    }
}

/// Строка игрока: «мид  ник#тег ★  алмаз II 86lp · 120W  …  ривен 9 · 56%».
fn row(ui: &egui::Ui, rect: Rect, player: &ClashPlayer, hovered: bool) {
    let painter = ui.painter_at(rect);
    if hovered {
        painter.rect_filled(rect, 0, white(7));
    }
    let font = theme::mono(FONT);
    let ranked = player.profile.as_ref().map(|p| &p.ranked);
    let (solo, solo_color) = rank_cell(ranked.map(|r| r.solo.as_ref()));
    let (flex, flex_color) = rank_cell(ranked.map(|r| r.flex.as_ref()));
    let position = player.position.map(|p| p.label().to_lowercase()).unwrap_or_else(|| "любая".into());
    let (champions, champions_color) = champions_text(player);

    let mut name: Vec<(String, Color32)> = match &player.riot_id {
        Some(id) => vec![
            (id.game_name.clone(), theme::TEXT),
            (format!("#{}", id.tag_line), theme::TEXT_MUTED),
        ],
        None => vec![("загружаю…".into(), theme::TEXT_HINT)],
    };
    if player.is_captain {
        name.push((" ★".into(), theme::AMBER_TEXT));
    }
    if player.is_self {
        name.push((" (вы)".into(), theme::ACCENT_TEXT));
    }
    let name: Vec<(&str, Color32)> = name.iter().map(|(t, c)| (t.as_str(), *c)).collect();
    let position_color = if player.position.is_some() { theme::TEXT_MUTED } else { theme::TEXT_HINT };

    let cells = [
        job(&[(&position, position_color)], &font),
        job(&name, &font),
        job(&[(&solo, solo_color)], &font),
        job(&[(&flex, flex_color)], &font),
        job(&[(&champions, champions_color)], &font),
    ];
    let mut x = rect.left();
    for (index, mut cell) in cells.into_iter().enumerate() {
        let width = COLUMNS.get(index).copied().unwrap_or(rect.right() - x) - 8.0;
        cell.wrap = egui::text::TextWrapping::truncate_at_width(width.max(8.0));
        let galley = painter.layout_job(cell);
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
        x += COLUMNS.get(index).copied().unwrap_or(0.0);
    }
    if player.is_self {
        ui.painter()
            .vline(rect.left() - 6.0, rect.y_range().shrink(5.0), Stroke::new(2.0, theme::ACCENT));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clash::ScoutChampion;

    fn champion(name: &str, games: u32, wins: u32) -> ScoutChampion {
        ScoutChampion {
            name: name.into(),
            games,
            wins,
            kda: String::new(),
        }
    }

    #[test]
    fn champions_text_shows_games_or_mastery() {
        let mut player = ClashPlayer::default();
        assert_eq!(champions_text(&player).0, "—");
        player.champions = vec![champion("Ривен", 9, 5), champion("Физз", 4, 3)];
        assert_eq!(champions_text(&player).0, "ривен 9 · 55%, физз 4 · 75%");
        player.champions = vec![champion("Кассадин", 0, 0)];
        assert_eq!(champions_text(&player).0, "кассадин (мастерство)");
    }

    #[test]
    fn keys_target_opponent_first() {
        let team = |name: &str| ClashTeam {
            players: vec![ClashPlayer {
                riot_id: RiotId::parse(&format!("{name}#RU1")).ok(),
                ..ClashPlayer::default()
            }],
            ..ClashTeam::default()
        };
        let mut state = ClashState {
            team: Some(team("Свой")),
            ..ClashState::default()
        };
        assert_eq!(riot_ids(&state)[0].game_name, "Свой");
        state.opponent = Some(team("Чужой"));
        assert_eq!(riot_ids(&state)[0].game_name, "Чужой");
    }
}
