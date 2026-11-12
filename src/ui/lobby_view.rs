//! Вкладка «Лобби» в стиле терминала: ваша группа, команда в выборе чемпионов и в игре —
//! таблицей с рангами. Щелчок по строке — профиль игрока на op.gg; клавиши `c` —
//! скопировать ники, `o` — мультипоиск op.gg (они же — подсказки в нижней строке окна).

use eframe::egui::{self, Align, Color32, CursorIcon, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::lobby::{self, LobbyPhase, LobbyPlayer, LobbyState};
use crate::riot::{RankedStats, Region, RiotId};
use crate::ui::terminal::{self, FONT, ROW_HEIGHT, job};
use crate::ui::theme::{self, white};

/// Ширины колонок: позиция, игрок, чемпион, solo/duo, flex; дальше — уровень.
const COLUMNS: [f32; 5] = [96.0, 260.0, 150.0, 190.0, 190.0];
const HEADERS: [&str; 6] = ["поз", "игрок", "чемпион", "solo/duo", "flex", "ур."];

/// Все известные Riot ID игроков лобби.
pub fn riot_ids(state: &LobbyState) -> Vec<RiotId> {
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

    if state.players.is_empty() {
        empty_state(ui, state.phase);
        return false;
    }
    table(ui, state, region, (ui.available_height() - 30.0).max(ROW_HEIGHT * 3.0))
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
            if state.players.is_empty() { "клиент League в игре" } else { "ваша команда" },
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
fn rank_cell(stats: Option<Option<&RankedStats>>) -> (String, Color32) {
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
fn table(ui: &mut egui::Ui, state: &LobbyState, region: Option<Region>, max_height: f32) -> bool {
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
    egui::ScrollArea::vertical()
        .id_salt("lobby_table")
        .max_height(max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for player in &state.players {
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
