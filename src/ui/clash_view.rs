//! Clash на вкладке «Лобби» (пока турнир идёт): соперник в текущем раунде и ваша команда —
//! таблицами с позициями, рангами и чемпионами из разведки. Щелчок по строке — окно
//! «прошлые Clash» игрока (ui/clash_history_view.rs, там же — его профиль на op.gg);
//! `c` — скопировать ники соперника, `o` — мультипоиск op.gg по нему
//! (если соперника ещё нет — по своей команде). Над таблицей соперника — блок «кого
//! банить»: пять чемпионов, на которых у них больше всего игр и побед в этом сезоне,
//! и запасные.

use eframe::egui::{self, Color32, CursorIcon, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::clash::{self, ClashPhase, ClashPlayer, ClashState, ClashTeam, HistoryRequest};
use crate::lobby;
use crate::riot::{Region, RiotId};
use crate::storage::unix_now;
use crate::ui::lobby_view::rank_cell;
use crate::ui::terminal::{FONT, ROW_HEIGHT, job};
use crate::ui::theme::{self, white};

/// Ширины колонок: позиция, игрок, solo/duo, flex; остальное — чемпионы.
const COLUMNS: [f32; 4] = [86.0, 250.0, 180.0, 170.0];
const HEADERS: [&str; 5] = ["поз", "игрок", "solo/duo 5×5", "flex 5×5", "чемпионы сезона"];

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

/// Рисует вкладку. Возвращает, чьи прошлые Clash открыть (по нику щёлкнули).
pub fn show(ui: &mut egui::Ui, state: &ClashState) -> Option<HistoryRequest> {
    header(ui, state);
    if let Some(problem) = &state.problem {
        ui.label(RichText::new(format!("! {problem}")).font(theme::mono(12.0)).color(theme::AMBER_TEXT));
    }
    ui.add_space(10.0);

    let Some(team) = &state.team else {
        empty_state(ui, state.phase);
        return None;
    };
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("clash_page")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            match &state.opponent {
                Some(opponent) => {
                    bans(ui, opponent);
                    role_bans(ui, opponent);
                    section_title(ui, "соперник", opponent, theme::RED_TEXT);
                    clicked = clicked.take().or(table(ui, opponent, "clash_opponent"));
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
            clicked = clicked.take().or(table(ui, team, "clash_team"));
        });
    clicked
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

/// Блок «кого банить»: пять основных банов строками (номер, чемпион, игрок, игры и
/// процент побед, kda) и запасные одной строкой ниже.
fn bans(ui: &mut egui::Ui, team: &ClashTeam) {
    let suggestions = clash::ban_suggestions(team);
    if suggestions.is_empty() {
        return;
    }
    let (main, reserve) = suggestions.split_at(suggestions.len().min(clash::BAN_SUGGESTIONS));
    ui.label(job(
        &[
            ("кого банить", theme::AMBER_TEXT),
            ("  больше всего игр и побед у соперника в этом сезоне", theme::TEXT_HINT),
        ],
        &theme::mono(12.5),
    ))
    .on_hover_text(
        "По разведке Clash: чем больше игр на чемпионе в этом сезоне и выше процент побед, \
         тем он выше в списке. Один игрок — не больше трёх банов.",
    );
    ui.add_space(4.0);
    ui.push_id("clash_bans", |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (index, ban) in main.iter().enumerate() {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
            ban_row(ui, rect, index + 1, ban);
        }
    });
    if !reserve.is_empty() {
        let list: Vec<String> = reserve.iter().map(|b| b.label()).collect();
        let list = list.join(" · ");
        let mut text = job(&[("запасные: ", theme::TEXT_HINT), (&list, theme::TEXT_MUTED)], &theme::mono(12.0));
        text.wrap.max_width = ui.available_width();
        ui.add_space(4.0);
        ui.label(text);
    }
    ui.add_space(14.0);
}

/// Колонки блока «по ролям»: роль, игрок; остальное — баны.
const ROLE_COLUMNS: [f32; 2] = [86.0, 220.0];

/// Блок «по ролям»: на каждую роль соперника — кто, скорее всего, её играет и до трёх
/// его чемпионов для бана. Роль угадана по чемпионам («?» — неуверенно).
fn role_bans(ui: &mut egui::Ui, team: &ClashTeam) {
    let roles = clash::role_bans(team);
    if roles.iter().all(|r| r.bans.is_empty()) {
        return;
    }
    ui.label(job(
        &[
            ("по ролям", theme::AMBER_TEXT),
            ("  роль угадана по чемпионам игрока · ? — не уверен", theme::TEXT_HINT),
        ],
        &theme::mono(12.5),
    ))
    .on_hover_text(
        "У соперника в Clash позиции видны как «любая», поэтому роль каждого игрока \
         угадывается по линиям его чемпионов. Если игрок сам выбрал позицию — берётся она.",
    );
    ui.add_space(4.0);
    ui.push_id("clash_role_bans", |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for role in &roles {
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
            role_row(ui, rect, role);
            if role.player.is_some() {
                response.on_hover_text(if role.known {
                    "Позицию игрок выбрал сам в Clash"
                } else if role.unsure {
                    "Угадано: чемпионы игрока почти не подходят на эту роль — других кандидатов не осталось"
                } else {
                    "Угадано по линиям чемпионов игрока"
                });
            }
        }
    });
    ui.add_space(14.0);
}

/// «топ  YANDERETTA  darius 35 · 60%, kayle 19 · 52%, smolder 18 · 72%».
fn role_row(ui: &egui::Ui, rect: Rect, role: &clash::RoleBans) {
    let painter = ui.painter_at(rect);
    let font = theme::mono(FONT);
    let label = role.role.label().to_lowercase();
    let mut player: Vec<(String, Color32)> = match &role.player {
        Some(name) if !name.is_empty() => vec![(name.clone(), theme::TEXT_MUTED)],
        Some(_) => vec![("загружаю…".into(), theme::TEXT_HINT)],
        None => vec![("—".into(), theme::TEXT_HINT)],
    };
    if role.unsure {
        player.push((" ?".into(), theme::AMBER_TEXT));
    }
    let player: Vec<(&str, Color32)> = player.iter().map(|(t, c)| (t.as_str(), *c)).collect();
    let bans: Vec<String> = role
        .bans
        .iter()
        .map(|b| {
            let name = b.champion.to_lowercase();
            if b.games == 0 { name } else { format!("{name} {} · {}%", b.games, b.wins * 100 / b.games) }
        })
        .collect();
    let (bans, bans_color) = if bans.is_empty() { ("—".to_string(), theme::TEXT_HINT) } else { (bans.join(", "), theme::TEXT) };
    let cells = [
        job(&[(&label, theme::TEXT_MUTED)], &font),
        job(&player, &font),
        job(&[(&bans, bans_color)], &font),
    ];
    let mut x = rect.left();
    for (index, mut cell) in cells.into_iter().enumerate() {
        let width = ROLE_COLUMNS.get(index).copied().unwrap_or(rect.right() - x) - 8.0;
        cell.wrap = egui::text::TextWrapping::truncate_at_width(width.max(8.0));
        let galley = painter.layout_job(cell);
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
        x += ROLE_COLUMNS.get(index).copied().unwrap_or(0.0);
    }
}

/// Колонки строки бана: номер, чемпион, игрок, игры и победы; остальное — kda.
const BAN_COLUMNS: [f32; 4] = [30.0, 160.0, 250.0, 150.0];

/// «1  darius  YANDERETTA  35 игр, 60%  kda 3.1».
fn ban_row(ui: &egui::Ui, rect: Rect, number: usize, ban: &clash::BanSuggestion) {
    let painter = ui.painter_at(rect);
    let font = theme::mono(FONT);
    let number = number.to_string();
    let champion = ban.champion.to_lowercase();
    let player = if ban.player.is_empty() { "—".to_string() } else { ban.player.clone() };
    let stats = ban.stats();
    let stats_color = if ban.games == 0 { theme::TEXT_MUTED } else { theme::TEXT };
    let kda = if ban.kda.is_empty() { String::new() } else { format!("kda {}", ban.kda) };
    let cells = [
        job(&[(&number, theme::AMBER_TEXT)], &font),
        job(&[(&champion, theme::TEXT)], &font),
        job(&[(&player, theme::TEXT_MUTED)], &font),
        job(&[(&stats, stats_color)], &font),
        job(&[(&kda, theme::TEXT_HINT)], &font),
    ];
    let mut x = rect.left();
    for (index, mut cell) in cells.into_iter().enumerate() {
        let width = BAN_COLUMNS.get(index).copied().unwrap_or(rect.right() - x) - 8.0;
        cell.wrap = egui::text::TextWrapping::truncate_at_width(width.max(8.0));
        let galley = painter.layout_job(cell);
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
        x += BAN_COLUMNS.get(index).copied().unwrap_or(0.0);
    }
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

/// Таблица игроков команды. Возвращает, по чьему нику щёлкнули.
fn table(ui: &mut egui::Ui, team: &ClashTeam, id: &str) -> Option<HistoryRequest> {
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

    let mut clicked = None;
    ui.push_id(id, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for player in &team.players {
            // Пока Riot ID не загрузился, открывать нечего.
            let request = history_request(player, team.roster_id);
            let clickable = request.is_some();
            let sense = if clickable { Sense::click() } else { Sense::hover() };
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), sense);
            row(ui, rect, player, clickable && response.hovered());
            if clickable {
                let response = response
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text("На ком играл в прошлых Clash");
                if response.clicked() {
                    clicked = request;
                }
            }
        }
    });
    clicked
}

/// Что нужно окну «прошлые Clash» про игрока; `None` — его Riot ID ещё не загрузился.
fn history_request(player: &ClashPlayer, roster_id: i64) -> Option<HistoryRequest> {
    Some(HistoryRequest {
        riot_id: player.riot_id.clone()?,
        puuid: player.puuid.clone(),
        roster_id,
        summoner_id: player.summoner_id,
    })
}

/// «ривен 9 · 56%, физз 4 · 75%»; без игр в сезоне — по мастерству: «кассадин, катарина».
fn champions_text(player: &ClashPlayer) -> (String, Color32) {
    if player.champions.is_empty() {
        return ("—".into(), theme::TEXT_HINT);
    }
    let by_mastery = player.champions.iter().all(|c| c.games == 0);
    // Разведка хранит до пяти чемпионов (для запасных банов) — в таблице хватит трёх.
    let list: Vec<String> = player
        .champions
        .iter()
        .take(3)
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
            champion_id: 0,
            name: name.into(),
            games,
            wins,
            kda: String::new(),
            positions: Vec::new(),
            main_position: None,
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
    fn page_with_bans_renders() {
        let player = |name: &str, champions: Vec<ScoutChampion>| ClashPlayer {
            riot_id: RiotId::parse(&format!("{name}#RU1")).ok(),
            champions,
            ..ClashPlayer::default()
        };
        let opponent = ClashTeam {
            name: "Соперник".into(),
            players: vec![
                player("a", vec![champion("Ривен", 9, 5), champion("Джакс", 4, 3)]),
                player("b", vec![champion("Ари", 12, 7)]),
                ClashPlayer::default(), // ещё не загрузился
            ],
            ..ClashTeam::default()
        };
        let state = ClashState {
            phase: ClashPhase::Scouting,
            team: Some(ClashTeam::default()),
            opponent: Some(opponent),
            ..ClashState::default()
        };
        let ctx = egui::Context::default();
        let mut sizes = Vec::new();
        // Узкое окно и широкое: ничего не должно падать.
        for width in [320.0, 1400.0] {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 900.0))),
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                show(ui, &state);
                sizes.push(ui.min_rect().height());
            })
            .drop_without_applying_deltas();
        }
        assert!(sizes.iter().all(|h| *h > ROW_HEIGHT * 8.0), "блоки банов нарисованы: {sizes:?}");
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
