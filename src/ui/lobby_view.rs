//! Вкладка «Лобби» в стиле терминала: ваша группа, команда в выборе чемпионов и в игре —
//! таблицей с рангами. Щелчок по строке — профиль игрока на op.gg; клавиши `c` —
//! скопировать ники, `o` — мультипоиск op.gg (они же — подсказки в нижней строке окна).

use eframe::egui::{self, Align, Color32, CursorIcon, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::lobby::{self, EnemyPlayer, LobbyPhase, LobbyPlayer, LobbyState, RecentForm};
use crate::riot::{RankedStats, Region, RiotId};
use crate::ui::terminal::{self, FONT, ROW_HEIGHT, job};
use crate::ui::theme::{self, white};

/// Ширины колонок: позиция, игрок, чемпион, solo/duo, flex; дальше — уровень.
const COLUMNS: [f32; 5] = [96.0, 260.0, 150.0, 190.0, 190.0];
const HEADERS: [&str; 6] = ["поз", "игрок", "чемпион", "solo/duo", "flex", "ур."];
/// Противники: позиция, игрок, чемпион, solo/duo, игры на этом чемпионе; дальше — форма.
const ENEMY_COLUMNS: [f32; 5] = [96.0, 240.0, 150.0, 180.0, 150.0];
const ENEMY_HEADERS: [&str; 6] = ["поз", "игрок", "чемпион", "solo/duo", "на чемпионе", "последние игры"];

/// Riot ID для `c` / `o`: противники, если идёт игра и они известны, иначе игроки лобби.
pub fn riot_ids(state: &LobbyState) -> Vec<RiotId> {
    if !state.enemies.is_empty() {
        return state.enemies.iter().filter_map(|p| p.riot_id.clone()).collect();
    }
    state.players.iter().filter_map(|p| p.riot_id.clone()).collect()
}

/// Скопировать ники через запятую. Возвращает сообщение для нижней строки.
pub fn copy_names(ctx: &egui::Context, state: &LobbyState) -> Result<&'static str, &'static str> {
    let ids = riot_ids(state);
    if ids.is_empty() {
        return Err("В лобби пока нет игроков с известными никами");
    }
    let names: Vec<String> = ids.iter().map(ToString::to_string).collect();
    ctx.copy_text(names.join(", "));
    Ok("Ники скопированы")
}

/// Открыть всех игроков на op.gg одной ссылкой.
pub fn open_multisearch(ctx: &egui::Context, state: &LobbyState, region: Option<Region>) -> Result<&'static str, &'static str> {
    let ids = riot_ids(state);
    match region {
        _ if ids.is_empty() => Err("В лобби пока нет игроков с известными никами"),
        None => Err("Сервер неизвестен: клиент League не подключён"),
        Some(region) => {
            ctx.open_url(egui::OpenUrl::new_tab(lobby::opgg_multisearch_url(region, &ids)));
            Ok("Мультипоиск op.gg открыт в браузере")
        }
    }
}

/// Рисует вкладку. `region` — сервер клиента League (для ссылок на op.gg).
/// Возвращает true, если щелчком по игроку открыли его профиль.
pub fn show(ui: &mut egui::Ui, state: &LobbyState, region: Option<Region>) -> bool {
    let found = state.players.iter().filter(|p| p.riot_id.is_some()).count();
    header(ui, state, found);
    if let Some(problem) = &state.problem {
        ui.label(RichText::new(format!("! {problem}")).font(theme::mono(12.0)).color(theme::AMBER_TEXT));
    }
    ui.add_space(10.0);

    if state.players.is_empty() && state.enemies.is_empty() {
        empty_state(ui, state.phase);
        return false;
    }
    let mut opened = false;
    egui::ScrollArea::vertical()
        .id_salt("lobby_page")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let both = !state.players.is_empty() && !state.enemies.is_empty();
            if !state.enemies.is_empty() {
                section_title(ui, "противники", theme::RED_TEXT);
                opened |= enemy_table(ui, &state.enemies, region);
                ui.add_space(14.0);
            }
            if !state.players.is_empty() {
                if both {
                    section_title(ui, "ваша команда", theme::ACCENT_TEXT);
                }
                opened |= table(ui, state, region);
            }
        });
    opened
}

fn section_title(ui: &mut egui::Ui, title: &str, color: Color32) {
    ui.label(RichText::new(title).font(theme::mono(12.5)).color(color));
    ui.add_space(4.0);
}

/// Шапка таблицы: названия колонок и черта под ними.
fn table_head(ui: &mut egui::Ui, headers: &[&str], columns: &[f32]) {
    let font = theme::mono(FONT);
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let painter = ui.painter();
    let mut x = head.left();
    for (index, title) in headers.iter().enumerate() {
        painter.text(pos2(x, head.center().y), egui::Align2::LEFT_CENTER, *title, font.clone(), theme::TEXT_HINT);
        x += columns.get(index).copied().unwrap_or(0.0);
    }
    painter.hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::BORDER));
    ui.add_space(4.0);
}

/// Строка таблицы, по которой можно щёлкнуть (профиль на op.gg). Возвращает
/// (место строки, наведена ли, щёлкнули ли).
fn link_row(ui: &mut egui::Ui, region: Option<Region>, riot_id: Option<&RiotId>) -> (Rect, bool, bool) {
    let link = match (region, riot_id) {
        (Some(region), Some(id)) => Some(lobby::opgg_profile_url(region, id)),
        _ => None,
    };
    let sense = if link.is_some() { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), sense);
    let Some(url) = link else {
        return (rect, false, false);
    };
    let response = response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Открыть профиль на op.gg");
    let clicked = response.clicked();
    if clicked {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
    (rect, response.hovered(), clicked)
}

/// Таблица противников. Возвращает true, если по щелчку открыли профиль.
fn enemy_table(ui: &mut egui::Ui, enemies: &[EnemyPlayer], region: Option<Region>) -> bool {
    table_head(ui, &ENEMY_HEADERS, &ENEMY_COLUMNS);
    let mut opened = false;
    ui.push_id("lobby_enemies", |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for enemy in enemies {
            let (rect, hovered, clicked) = link_row(ui, region, enemy.riot_id.as_ref());
            enemy_row(ui, rect, enemy, hovered);
            opened |= clicked;
        }
    });
    opened
}

/// «3 игры · 67%»; ни одной игры на нём среди последних — «0 из 20» (возможно, первая).
pub(crate) fn champion_form(form: Option<&RecentForm>) -> (String, Color32) {
    match form {
        None => ("…".into(), theme::TEXT_HINT),
        Some(f) if f.games == 0 => ("нет игр".into(), theme::TEXT_HINT),
        Some(f) if f.champion_games == 0 => (format!("0 из {}", f.games), theme::AMBER_TEXT),
        Some(f) => {
            let wr = f.champion_wins * 100 / f.champion_games;
            let color = if f.champion_games * 4 >= f.games { theme::ACCENT_TEXT } else { theme::TEXT };
            (format!("{} · {wr}%", games_word(f.champion_games)), color)
        }
    }
}

/// «55% из 20 · kda 3.1 · ▲3».
pub(crate) fn recent_form(form: Option<&RecentForm>) -> Vec<(String, Color32)> {
    let Some(f) = form else {
        return vec![("загружаю…".into(), theme::TEXT_HINT)];
    };
    let Some(wr) = f.winrate() else {
        return vec![("—".into(), theme::TEXT_HINT)];
    };
    let wr_color = match wr {
        60.. => theme::GREEN_TEXT,
        0..=40 => theme::RED_TEXT,
        _ => theme::TEXT,
    };
    let mut parts = vec![
        (format!("{wr}%"), wr_color),
        (format!(" из {} · kda {:.1}", f.games, f.kda()), theme::TEXT_MUTED),
    ];
    match f.streak {
        3.. => parts.push((format!(" · ▲{}", f.streak), theme::GREEN_TEXT)),
        ..=-3 => parts.push((format!(" · ▼{}", -f.streak), theme::RED_TEXT)),
        _ => {}
    }
    parts
}

/// «1 игра», «3 игры», «5 игр».
fn games_word(n: u32) -> String {
    let word = match (n % 10, n % 100) {
        (1, 11) => "игр",
        (1, _) => "игра",
        (2..=4, 12..=14) => "игр",
        (2..=4, _) => "игры",
        _ => "игр",
    };
    format!("{n} {word}")
}

/// Строка противника: «бот  ник#тег  джинкс  алмаз II 64lp · 120W  3 игры · 67%  55% из 20 …».
fn enemy_row(ui: &egui::Ui, rect: Rect, enemy: &EnemyPlayer, hovered: bool) {
    let painter = ui.painter_at(rect);
    if hovered {
        painter.rect_filled(rect, 0, white(7));
    }
    let font = theme::mono(FONT);
    let ranked = enemy.profile.as_ref().map(|p| &p.ranked);
    let (solo, solo_color) = rank_cell(ranked.map(|r| r.solo.as_ref()));
    let position = enemy.position.map(|p| p.label().to_lowercase()).unwrap_or_else(|| "—".into());
    let champion = enemy.champion.as_deref().map(str::to_lowercase).unwrap_or_else(|| "—".into());
    let (on_champion, on_champion_color) = champion_form(enemy.form.as_ref());
    let form = recent_form(enemy.form.as_ref());
    let form: Vec<(&str, Color32)> = form.iter().map(|(t, c)| (t.as_str(), *c)).collect();
    let name = match &enemy.riot_id {
        Some(id) => vec![
            (id.game_name.clone(), theme::TEXT),
            (format!("#{}", id.tag_line), theme::TEXT_MUTED),
        ],
        None => vec![("загружаю…".into(), theme::TEXT_HINT)],
    };
    let name: Vec<(&str, Color32)> = name.iter().map(|(t, c)| (t.as_str(), *c)).collect();

    let cells = [
        job(&[(&position, theme::TEXT_MUTED)], &font),
        job(&name, &font),
        job(&[(&champion, theme::TEXT)], &font),
        job(&[(&solo, solo_color)], &font),
        job(&[(&on_champion, on_champion_color)], &font),
        job(&form, &font),
    ];
    let mut x = rect.left();
    for (index, mut cell) in cells.into_iter().enumerate() {
        let width = ENEMY_COLUMNS.get(index).copied().unwrap_or(rect.right() - x) - 8.0;
        cell.wrap = egui::text::TextWrapping::truncate_at_width(width.max(8.0));
        let galley = painter.layout_job(cell);
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
        x += ENEMY_COLUMNS.get(index).copied().unwrap_or(0.0);
    }
}

/// «лобби · выбор чемпионов    найдено 4/5» и строка пояснения под ним.
fn header(ui: &mut egui::Ui, state: &LobbyState, found: usize) {
    let (phase, subtitle) = match state.phase {
        LobbyPhase::NoClient => (None, "клиент League не запущен"),
        LobbyPhase::Idle => (None, "вы не в лобби"),
        LobbyPhase::Party => (Some("группа"), "игроки, которые сейчас с вами в группе"),
        LobbyPhase::Queue => (Some("поиск игры"), "ваша группа"),
        LobbyPhase::ChampSelect => (
            Some("выбор чемпионов"),
            if found >= state.players.len() && found > 0 {
                "союзники определены"
            } else {
                "ищу имена союзников…"
            },
        ),
        LobbyPhase::InGame => (
            Some("игра идёт"),
            if !state.enemies.is_empty() {
                "противники: ранг, игры на этом чемпионе и форма по последним 20 играм"
            } else if state.players.is_empty() {
                "клиент League в игре"
            } else {
                "ваша команда"
            },
        ),
    };
    let font = theme::mono(12.5);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.label(RichText::new("лобби").font(font.clone()).color(theme::TEXT));
        if let Some(phase) = phase {
            ui.label(RichText::new(phase).font(font.clone()).color(theme::ACCENT_TEXT));
        }
        if !state.players.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let all = found == state.players.len();
                let color = if all { theme::GREEN_TEXT } else { theme::ACCENT_TEXT };
                ui.label(
                    RichText::new(format!("найдено {found}/{}", state.players.len()))
                        .font(font.clone())
                        .color(color),
                );
            });
        }
    });
    ui.label(RichText::new(subtitle).font(theme::mono(11.5)).color(theme::TEXT_HINT));
}

/// Клиент не запущен или вы не в лобби: пара строк пояснения.
fn empty_state(ui: &mut egui::Ui, phase: LobbyPhase) {
    let (mark, color, title, text) = match phase {
        LobbyPhase::NoClient => (
            "○",
            theme::AMBER_TEXT,
            "клиент League не запущен",
            "Запустите клиент — здесь появятся ваша группа и союзники в выборе чемпионов.",
        ),
        LobbyPhase::InGame => (
            "●",
            theme::ACCENT_TEXT,
            "идёт игра",
            "Союзники с рангами показываются во время выбора чемпионов. \
             После игры здесь появится следующее лобби.",
        ),
        _ => (
            "·",
            theme::TEXT_MUTED,
            "ждём начала игры",
            "Когда начнётся выбор чемпионов, здесь появятся союзники с рангами — \
             и в ранговых играх, где клиент скрывает их имена.",
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

/// «алмаз II 64lp · 52%» / «алмаз II 64lp · 119 побед» (поражения чужих аккаунтов клиент скрывает).
pub(crate) fn rank_cell(stats: Option<Option<&RankedStats>>) -> (String, Color32) {
    match stats {
        None => ("—".into(), theme::TEXT_HINT),
        Some(None) => ("без ранга".into(), theme::TEXT_HINT),
        Some(Some(s)) => {
            let extra = match s.winrate() {
                Some(wr) => format!(" · {wr:.0}%"),
                None => format!(" · {}W", s.wins),
            };
            (format!("{}{extra}", terminal::rank_text(s)), theme::tier_color(Some(s.tier)))
        }
    }
}

/// Таблица игроков. Возвращает true, если по щелчку открыли профиль.
fn table(ui: &mut egui::Ui, state: &LobbyState, region: Option<Region>) -> bool {
    table_head(ui, &HEADERS, &COLUMNS);
    let mut opened = false;
    ui.push_id("lobby_table", |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for player in &state.players {
            let (rect, hovered, clicked) = link_row(ui, region, player.riot_id.as_ref());
            row(ui, rect, player, hovered);
            opened |= clicked;
        }
    });
    opened
}

/// Строка игрока: «мид  benjm netanyahu#01irl (вы)  экко  алмаз II 86lp · 52%  …  469».
fn row(ui: &egui::Ui, rect: Rect, player: &LobbyPlayer, hovered: bool) {
    let painter = ui.painter_at(rect);
    if hovered {
        painter.rect_filled(rect, 0, white(7));
    }
    let font = theme::mono(FONT);
    let profile = player.profile.as_ref();
    let ranked = profile.map(|p| &p.ranked);
    let (solo, solo_color) = rank_cell(ranked.map(|r| r.solo.as_ref()));
    let (flex, flex_color) = rank_cell(ranked.map(|r| r.flex.as_ref()));
    let position = player.position.map(|p| p.label().to_lowercase()).unwrap_or_else(|| "—".into());
    let champion = player
        .champion
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_else(|| "не выбран".into());
    let level = profile.map(|p| p.summoner_level.to_string()).unwrap_or_default();

    let name = match &player.riot_id {
        Some(id) => {
            let mut parts = vec![
                (id.game_name.clone(), theme::TEXT),
                (format!("#{}", id.tag_line), theme::TEXT_MUTED),
            ];
            if player.is_self {
                parts.push((" (вы)".into(), theme::ACCENT_TEXT));
            }
            parts
        }
        None => vec![("скрытый игрок".into(), theme::TEXT_HINT)],
    };
    let name: Vec<(&str, Color32)> = name.iter().map(|(t, c)| (t.as_str(), *c)).collect();
    let champion_color = if player.champion.is_some() { theme::TEXT } else { theme::TEXT_HINT };

    let cells = [
        job(&[(&position, theme::TEXT_MUTED)], &font),
        job(&name, &font),
        job(&[(&champion, champion_color)], &font),
        job(&[(&solo, solo_color)], &font),
        job(&[(&flex, flex_color)], &font),
        job(&[(&level, theme::TEXT_MUTED)], &font),
    ];
    let mut x = rect.left();
    for (index, mut cell) in cells.into_iter().enumerate() {
        let width = COLUMNS.get(index).copied().unwrap_or(rect.right() - x) - 8.0;
        cell.wrap = egui::text::TextWrapping::truncate_at_width(width.max(8.0));
        let galley = painter.layout_job(cell);
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
        x += COLUMNS.get(index).copied().unwrap_or(0.0);
    }
    // «Вы» — тонкая янтарная черта слева, как метка в терминале.
    if player.is_self {
        ui.painter()
            .vline(rect.left() - 6.0, rect.y_range().shrink(5.0), Stroke::new(2.0, theme::ACCENT));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn games_word_declension() {
        let words: Vec<String> = [1, 2, 5, 11, 12, 21, 22, 25].into_iter().map(games_word).collect();
        assert_eq!(words, ["1 игра", "2 игры", "5 игр", "11 игр", "12 игр", "21 игра", "22 игры", "25 игр"]);
    }

    #[test]
    fn champion_form_marks_first_time_and_main() {
        let form = |games, champion_games, champion_wins| RecentForm {
            games,
            wins: games / 2,
            champion_games,
            champion_wins,
            ..RecentForm::default()
        };
        assert_eq!(champion_form(None).0, "…");
        assert_eq!(champion_form(Some(&form(20, 0, 0))), ("0 из 20".into(), theme::AMBER_TEXT));
        assert_eq!(champion_form(Some(&form(20, 12, 9))), ("12 игр · 75%".into(), theme::ACCENT_TEXT));
        assert_eq!(champion_form(Some(&form(20, 2, 1))).0, "2 игры · 50%");
    }

    #[test]
    fn enemies_are_the_target_for_keys() {
        let id = |name: &str| RiotId::parse(&format!("{name}#RU1")).ok();
        let mut state = LobbyState {
            players: vec![LobbyPlayer {
                riot_id: id("Союзник"),
                ..LobbyPlayer::default()
            }],
            ..LobbyState::default()
        };
        assert_eq!(riot_ids(&state)[0].game_name, "Союзник");
        state.enemies = vec![EnemyPlayer {
            riot_id: id("Враг"),
            ..EnemyPlayer::default()
        }];
        assert_eq!(riot_ids(&state)[0].game_name, "Враг");
    }
}
