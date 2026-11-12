//! Строка аккаунта в списке: иконка с уровнем, Riot ID, регион и теги, ранги Solo/Duo
//! и Flex с эмблемами, последние игры, кнопки и статус обновления.
//!
//! Карточка ничего не меняет сама: она возвращает нажатую кнопку ([`CardAction`]),
//! а что делать — решает окно (app.rs).

use eframe::egui::{
    self, Align, Color32, Layout, Margin, Rect, RichText, Sense, Shadow, Stroke, StrokeKind, Vec2,
    pos2, vec2,
};

use crate::error::AppError;
use crate::history::{Queue, RankPoint};
use crate::images::ImageKey;
use crate::messages::CredentialKind;
use crate::riot::models::RECENT_MATCHES;
use crate::riot::{MatchSummary, RankedStats, Tier};
use crate::storage::Account;
use crate::ui::image_store::ImageStore;
use crate::ui::theme::{self, Weight, alpha, white};
use crate::ui::widgets::{self, Button, Icon, IconButton, Tone};
use crate::ui::{format, match_view, rank_history};

/// Высота содержимого строки (без отступов рамки).
const ROW_HEIGHT: f32 = 48.0;
const AVATAR_SIZE: f32 = 40.0;
const EMBLEM_SIZE: f32 = 40.0;
/// Фиксированная ширина колонок рангов — чтобы они стояли ровно друг под другом.
const RANK_WIDTH: f32 = 168.0;
/// Место под кнопки справа («Войти» и пять значков).
const ACTIONS_WIDTH: f32 = 262.0;
/// Последние игры: иконка чемпиона и полоска результата.
const MATCH_ICON: f32 = 20.0;
const MATCH_GAP: f32 = 2.0;
const MATCHES_WIDTH: f32 = MATCH_ICON * RECENT_MATCHES as f32 + MATCH_GAP * (RECENT_MATCHES as f32 - 1.0);
/// Уже этого колонка с именем не сжимается — тогда прячем последние игры.
const MIN_IDENTITY_WIDTH: f32 = 170.0;

/// Что карточке нужно знать, кроме самого аккаунта.
pub struct CardState<'a> {
    /// В этот аккаунт сейчас выполнен вход в клиенте League.
    pub in_client: bool,
    pub refreshing: bool,
    /// Ошибка последнего обновления.
    pub error: Option<&'a AppError>,
    /// Нажата «Войти»: логин уже в буфере, следующая кнопка копирует пароль.
    pub login_pending: bool,
    pub images: &'a ImageStore,
}

/// Какую кнопку нажали на карточке.
pub enum CardAction {
    Refresh,
    Copy(CredentialKind),
    /// «Войти»: логин в буфер + запустить Riot Client.
    StartLogin,
    /// Вторая половина «Войти»: пароль в буфер.
    FinishLogin,
    Edit,
    Delete,
    /// Щелчок по игре в полоске «последние игры»: открыть подробности (id игры).
    OpenMatch(String),
    /// Щелчок по рангу: окно «История ранга» этой очереди.
    OpenHistory(Queue),
}

pub fn show(ui: &mut egui::Ui, account: &Account, state: &CardState<'_>) -> Option<CardAction> {
    let mut action = None;
    // Frame::begin → содержимое → paint: так можно выбрать цвет рамки ПОСЛЕ того, как
    // узнали, наведён ли курсор на строку (фон всё равно рисуется под содержимым).
    let mut prepared = theme::panel().inner_margin(Margin::symmetric(14, 10)).begin(ui);
    {
        let ui = &mut prepared.content_ui;
        ui.set_width(ui.available_width());
        // Строки внутри колонок — по высоте текста (иначе каждая `ui.horizontal`
        // занимает не меньше interact_size.y).
        ui.spacing_mut().interact_size.y = 0.0;
        // Высота строки задана заранее: egui раскладывает за один проход, и только так
        // все колонки (иконка, имя, ранги, кнопки) встают ровно по центру.
        let size = vec2(ui.available_width(), ROW_HEIGHT);
        ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_height(ROW_HEIGHT);
            let cache = account.cache.as_ref();
            let level = cache.map(|c| c.summoner_level.to_string());
            avatar(ui, cache.map(|c| c.profile_icon_id), level.as_deref(), AVATAR_SIZE, state.images);
            ui.add_space(4.0);

            // Колонка с именем занимает то, что осталось от рангов, игр и кнопок.
            // Последние игры показываем, только если строке хватает ширины.
            let gap = ui.spacing().item_spacing.x;
            let fixed = RANK_WIDTH * 2.0 + ACTIONS_WIDTH + gap * 3.0;
            let show_matches = ui.available_width() - fixed - MATCHES_WIDTH - gap >= MIN_IDENTITY_WIDTH;
            let reserved = if show_matches { fixed + MATCHES_WIDTH + gap } else { fixed };
            let free = ui.available_width() - reserved;
            identity(ui, account, state.in_client, free.clamp(140.0, 360.0));

            // Option<&RankedQueues>: None — данных ещё нет (аккаунт не обновлялся).
            let ranked = cache.map(|c| &c.ranked);
            for queue in Queue::ALL {
                let stats = ranked.map(|r| queue.stats(r));
                if rank_block(ui, account.id, queue, stats, &account.history, state.images) {
                    action = Some(CardAction::OpenHistory(queue));
                }
            }
            if show_matches {
                let matches = cache.map(|c| c.matches.as_deref());
                if let Some(match_id) = recent_matches(ui, account.id, matches, state.images) {
                    action = Some(CardAction::OpenMatch(match_id));
                }
            }

            let width = ui.available_width();
            ui.allocate_ui_with_layout(vec2(width, ROW_HEIGHT), Layout::top_down(Align::Max), |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.allocate_ui_with_layout(vec2(width, 28.0), Layout::right_to_left(Align::Center), |ui| {
                    // Только если кнопку нажали: `None` не должен затирать щелчок по игре выше.
                    if let Some(pressed) = buttons(ui, account, state) {
                        action = Some(pressed);
                    }
                });
                ui.allocate_ui_with_layout(vec2(width, 16.0), Layout::right_to_left(Align::Center), |ui| {
                    status(ui, account, state);
                });
            });
        });
    }
    let response = prepared.allocate_space(ui);
    let hovered = response.contains_pointer();
    let (fill, border) = if state.login_pending {
        (Color32::from_rgba_unmultiplied(0x13, 0x1E, 0x36, 220), alpha(theme::ACCENT_LIGHT, 170))
    } else if hovered {
        (Color32::from_rgba_unmultiplied(0x12, 0x1B, 0x2E, 200), alpha(theme::ACCENT_LIGHT, 80))
    } else if state.in_client {
        (alpha(theme::CARD, 180), alpha(theme::ACCENT_LIGHT, 60))
    } else {
        (alpha(theme::CARD, 180), theme::BORDER)
    };
    prepared.frame.fill = fill;
    prepared.frame.stroke = Stroke::new(1.0, border);
    if state.login_pending {
        prepared.frame.shadow = Shadow {
            offset: [0, 0],
            blur: 24,
            spread: 0,
            color: alpha(theme::ACCENT, 50),
        };
    }
    prepared.paint(ui);
    action
}

/// Иконка профиля с плашкой снизу (уровень или позиция) — как у игроков в reveal.
/// Используется и в списке аккаунтов, и на вкладке «Лобби».
pub fn avatar(ui: &mut egui::Ui, icon_id: Option<i32>, badge: Option<&str>, size: f32, images: &ImageStore) {
    let (rect, _) = ui.allocate_exact_size(vec2(size + 8.0, size + 6.0), Sense::hover());
    let icon = Rect::from_min_size(pos2(rect.min.x, rect.min.y), Vec2::splat(size));
    let radius = (size / 5.0) as u8;

    // and_then — «если Some, вызвать функцию, которая тоже возвращает Option».
    match icon_id.and_then(|id| images.get(ImageKey::ProfileIcon(id))) {
        Some(image) => image.corner_radius(radius).paint_at(ui, icon),
        None => {
            ui.painter().rect_filled(icon, radius, white(13)); // заглушка, пока грузится
        }
    }
    let painter = ui.painter();
    painter.rect_stroke(icon, radius, Stroke::new(1.0, white(26)), StrokeKind::Inside);

    if let Some(text) = badge {
        let galley = widgets::text_galley(ui, RichText::new(text).size(9.0).semibold());
        let pill = Rect::from_center_size(
            pos2(icon.center().x, icon.bottom() + 1.0),
            vec2(galley.size().x + 8.0, 13.0),
        );
        painter.rect(pill, 4, theme::BADGE_BG, Stroke::new(1.0, alpha(theme::ACCENT_LIGHT, 64)), StrokeKind::Inside);
        painter.galley(pill.center() - galley.size() / 2.0, galley, theme::ACCENT_TEXT);
    }
}

/// Riot ID, «в клиенте», регион, теги, заметка.
fn identity(ui: &mut egui::Ui, account: &Account, in_client: bool, width: f32) {
    ui.allocate_ui_with_layout(vec2(width, ROW_HEIGHT), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.add_space(4.0); // две строки по ~20 px — по центру строки высотой 48
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add(
                egui::Label::new(
                    RichText::new(&account.riot_id.game_name).size(14.0).semibold().color(theme::TEXT),
                )
                .truncate(),
            );
            ui.label(
                RichText::new(format!(" #{}", account.riot_id.tag_line))
                    .size(12.5)
                    .color(theme::TEXT_MUTED),
            );
            if in_client {
                ui.add_space(8.0);
                widgets::pill(ui, "в клиенте", Tone::Accent)
                    .on_hover_text("В этот аккаунт сейчас выполнен вход в клиенте League");
            }
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            widgets::tag(ui, account.region.label(), Tone::Accent).on_hover_text(account.region.full_name());
            for tag in &account.tags {
                widgets::tag(ui, tag, Tone::Neutral);
            }
            // Заметка — первая строка, обрезанная по ширине; целиком — при наведении.
            if let Some(first_line) = account.note.lines().next().filter(|l| !l.trim().is_empty()) {
                ui.add_space(4.0);
                let more = account.note.lines().count() > 1;
                let text = if more { format!("{first_line} …") } else { first_line.to_string() };
                ui.add(egui::Label::new(RichText::new(text).size(11.0).color(theme::TEXT_MUTED)).truncate())
                    .on_hover_text(&account.note);
            }
        });
    });
}

/// Эмблема, название ранга, LP, победы/поражения и винрейт; справа от заголовка —
/// «+34» и мини-график за неделю. Наведение — график за месяц, щелчок — окно истории
/// (возвращает true). `stats`: `None` — данных ещё нет; `Some(None)` — без ранга.
fn rank_block(
    ui: &mut egui::Ui,
    account_id: i64,
    queue: Queue,
    stats: Option<Option<&RankedStats>>,
    history: &[RankPoint],
    images: &ImageStore,
) -> bool {
    let block = ui.allocate_ui_with_layout(vec2(RANK_WIDTH, EMBLEM_SIZE), Layout::left_to_right(Align::Center), |ui| {
        ui.set_width(RANK_WIDTH);
        ui.spacing_mut().item_spacing.x = 8.0;
        // flatten: Option<Option<T>> → Option<T>.
        let tier = stats.flatten().map(|s| s.tier);
        emblem(ui, images, tier, stats.is_some(), EMBLEM_SIZE);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.horizontal(|ui| {
                ui.label(theme::caps(queue.label()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    rank_history::trend(ui, history, queue);
                });
            });
            match stats {
                None => {
                    ui.label(RichText::new("нет данных").size(12.0).color(theme::TEXT_HINT));
                }
                Some(None) => {
                    ui.label(RichText::new("Без ранга").size(12.0).color(theme::TEXT_MUTED));
                }
                Some(Some(s)) => {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        ui.label(
                            RichText::new(s.rank_label())
                                .size(12.5)
                                .semibold()
                                .color(theme::tier_color(Some(s.tier))),
                        );
                        ui.label(RichText::new(format!("{} LP", s.lp)).size(11.5).color(theme::TEXT));
                    });
                    record(ui, s);
                }
            }
        });
    });

    // Без ранга и без истории показывать нечего.
    if !matches!(stats, Some(Some(_))) && !history.iter().any(|p| p.queue == queue) {
        return false;
    }
    // Щелчок — по всему блоку (поверх эмблемы и подписей, поэтому подсказка про скрытые
    // поражения — внутри `preview`).
    let id = ui.scope_id().with(("rank", account_id, queue.key()));
    let response = ui
        .interact(block.response.rect, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter()
            .rect_filled(block.response.rect.expand2(vec2(4.0, 2.0)), 6, white(6));
    }
    let clicked = response.clicked();
    response.on_hover_ui(|ui| rank_history::preview(ui, history, queue, stats.flatten()));
    clicked
}

/// Ранг одной строкой для вкладки «Лобби»: маленькая эмблема, «Алмаз II», LP и винрейт.
pub fn rank_line(ui: &mut egui::Ui, stats: Option<Option<&RankedStats>>, images: &ImageStore) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let tier = stats.flatten().map(|s| s.tier);
        emblem(ui, images, tier, stats.is_some(), 18.0);
        match stats {
            None => {
                ui.label(RichText::new("нет данных").size(11.0).color(theme::TEXT_HINT));
            }
            Some(None) => {
                ui.label(RichText::new("Без ранга").size(11.0).color(theme::TEXT_MUTED));
            }
            Some(Some(s)) => {
                ui.label(
                    RichText::new(s.rank_label())
                        .size(11.5)
                        .semibold()
                        .color(theme::tier_color(Some(s.tier))),
                );
                let mut details = format!("· {} LP", s.lp);
                // Поражения других игроков клиент League не показывает — тогда только победы.
                match (s.winrate(), s.games()) {
                    (Some(winrate), Some(games)) => details.push_str(&format!(" · {winrate:.0}% из {games}")),
                    _ => details.push_str(&format!(" · {} побед", s.wins)),
                }
                ui.label(RichText::new(details).size(11.0).color(theme::TEXT_MUTED));
            }
        }
    });
}

/// «ИГРЫ · 3W 2L» и пять иконок чемпионов с полоской под каждой: зелёная — победа,
/// красная — поражение, серая — ремейк. Наведение — сводка игры, щелчок — подробности.
/// `matches`: `None` — аккаунт ещё не обновлялся (колонка пустая, но место занято),
/// `Some(None)` — игры не загружались, `Some(Some([]))` — игр нет.
/// Возвращает id игры, по которой щёлкнули.
fn recent_matches(
    ui: &mut egui::Ui,
    account_id: i64,
    matches: Option<Option<&[MatchSummary]>>,
    images: &ImageStore,
) -> Option<String> {
    let mut clicked = None;
    ui.allocate_ui_with_layout(vec2(MATCHES_WIDTH, ROW_HEIGHT), Layout::top_down(Align::Min), |ui| {
        ui.set_width(MATCHES_WIDTH);
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.add_space(4.0);
        let Some(matches) = matches else {
            return;
        };
        let empty_text = match matches {
            None => Some("нет данных"),
            Some([]) => Some("нет недавних игр"),
            Some(_) => None,
        };
        if let Some(text) = empty_text {
            ui.label(theme::caps("Игры"));
            ui.label(RichText::new(text).size(11.0).color(theme::TEXT_HINT));
            return;
        }
        let matches = matches.unwrap_or_default();
        // Ремейки не считаем ни победой, ни поражением.
        let counted = matches.iter().filter(|m| !m.remake);
        let wins = counted.clone().filter(|m| m.win).count();
        let losses = counted.count() - wins;
        ui.label(theme::caps(&format!("Игры · {wins}W {losses}L")));

        let (strip, _) = ui.allocate_exact_size(vec2(MATCHES_WIDTH, MATCH_ICON + 5.0), Sense::hover());
        for (index, game) in matches.iter().take(RECENT_MATCHES).enumerate() {
            let x = strip.left() + index as f32 * (MATCH_ICON + MATCH_GAP);
            let icon = Rect::from_min_size(pos2(x, strip.top()), Vec2::splat(MATCH_ICON));
            let bar = Rect::from_min_size(pos2(x, icon.bottom() + 2.0), vec2(MATCH_ICON, 3.0));

            let response = ui
                .interact(icon.union(bar), ui.scope_id().with(("match", account_id, index)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            // Ремейк — иконка бледнее: игра не считается.
            match_view::champion_icon(ui, images, game.me.champion_id, icon, game.remake);
            let painter = ui.painter();
            let outline = if response.hovered() { white(90) } else { white(26) };
            painter.rect_stroke(icon, 4, Stroke::new(1.0, outline), StrokeKind::Inside);
            painter.rect_filled(bar, 1, match_view::result_color(game.win, game.remake));

            if response.clicked() {
                clicked = Some(game.id.clone());
            }
            response.on_hover_ui(|ui| match_tooltip(ui, game, images));
        }
    });
    clicked
}

/// Подсказка над иконкой игры — коротко, что в ней было.
fn match_tooltip(ui: &mut egui::Ui, game: &MatchSummary, images: &ImageStore) {
    let me = &game.me;
    ui.set_max_width(300.0);
    ui.spacing_mut().item_spacing.y = 3.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (result, color) = match_view::result_label(game.win, game.remake);
        ui.label(RichText::new(result).size(13.0).semibold().color(color));
        ui.label(RichText::new(match_view::champion_name(me)).size(13.0).semibold().color(theme::TEXT));
        if me.champ_level > 0 {
            ui.label(RichText::new(format!("ур. {}", me.champ_level)).size(11.5).color(theme::TEXT_MUTED));
        }
    });
    ui.label(
        RichText::new(format!(
            "{} · {} · {}",
            game.queue_label(),
            match_view::duration(game.duration_secs),
            format::relative_time(game.played_at)
        ))
        .size(11.0)
        .color(theme::TEXT_MUTED),
    );
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(game.kda()).size(13.0).semibold().color(theme::TEXT));
        ui.label(
            RichText::new(format!("KDA {:.2}", me.kda_ratio()))
                .size(11.5)
                .color(match_view::kda_color(me.kda_ratio())),
        );
    });
    if game.has_full_stats() {
        ui.label(
            RichText::new(format!(
                "CS {} ({:.1}/мин) · урон {} · золото {}",
                me.cs,
                me.cs_per_minute(game.duration_secs),
                match_view::number(me.damage),
                match_view::number(me.gold)
            ))
            .size(11.5)
            .color(theme::TEXT),
        );
        ui.add_space(2.0);
        match_view::item_row(ui, images, &me.items, 20.0);
        if let Some(label) = me.multikill_label() {
            ui.label(RichText::new(label).size(11.5).semibold().color(theme::AMBER_TEXT));
        }
    }
    ui.add_space(2.0);
    ui.label(RichText::new("Щёлкните — подробности игры").size(10.5).color(theme::TEXT_HINT));
}

/// Эмблема ранга. `has_data: false` — данных ещё нет (эмблема бледная).
pub fn emblem(ui: &mut egui::Ui, images: &ImageStore, tier: Option<Tier>, has_data: bool, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if let Some(image) = images.get(ImageKey::Emblem(tier)) {
        // Без ранга и без данных — эмблема бледная.
        let image = match (has_data, tier) {
            (false, _) => image.tint(Color32::from_white_alpha(50)),
            (true, None) => image.tint(Color32::from_white_alpha(110)),
            (true, Some(_)) => image,
        };
        image.paint_at(ui, rect);
    }
}

/// «20W 15L · 57%»: винрейт от 50% — зелёный, ниже — красный.
fn record(ui: &mut egui::Ui, s: &RankedStats) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let text = match s.losses {
            Some(losses) => format!("{}W {losses}L", s.wins),
            None => format!("{}W · L скрыты", s.wins),
        };
        let label = ui.label(RichText::new(text).size(10.5).color(theme::TEXT_MUTED));
        if s.losses.is_none() {
            label.on_hover_text(
                "Клиент League показывает поражения только аккаунта, в который в нём выполнен вход. \
                 С API-ключом (Настройки) они загрузятся из Riot API.",
            );
        }
        if let Some(winrate) = s.winrate() {
            let color = if winrate >= 50.0 { theme::GREEN_TEXT } else { theme::RED_TEXT };
            ui.label(RichText::new(format!("{winrate:.0}%")).size(10.5).medium().color(color));
        }
    });
}

/// Кнопки справа. Раскладка справа налево: первая добавленная — самая правая.
fn buttons(ui: &mut egui::Ui, account: &Account, state: &CardState<'_>) -> Option<CardAction> {
    let mut action = None;
    ui.spacing_mut().item_spacing.x = 2.0;

    if ui.add(IconButton::new(Icon::Trash).danger()).on_hover_text("Удалить").clicked() {
        action = Some(CardAction::Delete);
    }
    if ui
        .add(IconButton::new(Icon::Edit))
        .on_hover_text("Изменить: Riot ID, регион, теги, заметка, логин и пароль")
        .clicked()
    {
        action = Some(CardAction::Edit);
    }
    if state.refreshing {
        ui.add_space(6.0);
        widgets::spinner(ui, 14.0);
        ui.add_space(6.0);
    } else if ui.add(IconButton::new(Icon::Refresh)).on_hover_text("Обновить ранги").clicked() {
        action = Some(CardAction::Refresh);
    }

    if account.has_login || account.has_password {
        ui.add_space(8.0);
    }
    if account.has_password
        && ui.add(IconButton::new(Icon::Key)).on_hover_text("Скопировать пароль").clicked()
    {
        action = Some(CardAction::Copy(CredentialKind::Password));
    }
    if account.has_login {
        if ui.add(IconButton::new(Icon::User)).on_hover_text("Скопировать логин").clicked() {
            action = Some(CardAction::Copy(CredentialKind::Login));
        }
        ui.add_space(4.0);
        if state.login_pending {
            // Вторая половина «Войти»: та же синяя кнопка, теперь копирует пароль.
            let button = Button::primary("Пароль").icon(Icon::Key).small().min_width(84.0);
            if ui
                .add(button)
                .on_hover_text("Логин уже в буфере: вставьте его в Riot Client (Ctrl+V), затем нажмите — скопируется пароль")
                .clicked()
            {
                action = Some(CardAction::FinishLogin);
            }
        } else {
            let button = Button::primary("Войти").icon(Icon::Login).small().min_width(84.0);
            let login = ui
                .add_enabled(!state.in_client, button)
                .on_hover_text(
                    "Скопирует логин и откроет Riot Client. Вставьте логин (Ctrl+V), \
                     потом нажмите «Пароль» и вставьте пароль. Сама программа в клиент ничего не вводит.",
                )
                .on_disabled_hover_text("Вы уже вошли в этот аккаунт");
            if login.clicked() {
                action = Some(CardAction::StartLogin);
            }
        }
    }
    action
}

/// Под кнопками: «обновляется», ошибка или время последнего обновления.
fn status(ui: &mut egui::Ui, account: &Account, state: &CardState<'_>) {
    let small = |text: String, color: Color32| RichText::new(text).size(10.5).color(color);
    if state.refreshing {
        ui.label(small("обновляется…".into(), theme::TEXT_MUTED));
    } else if let Some(error) = state.error {
        // Ранги на карточке остаются с прошлого обновления — говорим, насколько они старые.
        let age = account
            .cache
            .as_ref()
            .map(|cache| format!("обновлено {}", format::relative_time(cache.fetched_at)));
        // Без интернета полный текст ошибки длинный и одинаковый у всех карточек —
        // показываем коротко, подробности во всплывающей подсказке.
        let text = match (error.is_offline(), &age) {
            (true, Some(age)) => format!("нет связи · {age}"),
            (true, None) => "нет связи с сервером".to_string(),
            (false, _) => error.to_string(),
        };
        let hint = match &age {
            Some(age) => format!("{error}\nПоказаны сохранённые данные ({age})"),
            None => error.to_string(),
        };
        // Справа налево: сначала текст (он правее), потом значок слева от него.
        let label = ui.add(egui::Label::new(small(text, theme::RED_TEXT)).truncate());
        label.on_hover_text(hint);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
        widgets::paint_icon(ui.painter(), rect, Icon::Warning, theme::RED_TEXT);
    } else if let Some(cache) = &account.cache {
        ui.label(small(
            format!("обновлено {}", format::relative_time(cache.fetched_at)),
            theme::TEXT_HINT,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::models::MATCH_STATS_VERSION;
    use crate::riot::{PlayerStats, RankedQueues, Region, RiotId};
    use crate::storage::CachedProfile;

    fn account_with_games() -> Account {
        let game = |id: &str| MatchSummary {
            id: id.into(),
            win: true,
            remake: false,
            queue_id: 420,
            played_at: 0,
            duration_secs: 1800,
            stats_version: MATCH_STATS_VERSION,
            me: PlayerStats {
                champion_id: 42,
                ..Default::default()
            },
        };
        Account {
            id: 7,
            riot_id: RiotId::parse("Тест#RU1").unwrap(),
            region: Region::Ru,
            puuid: None,
            lcu_puuid: None,
            note: String::new(),
            tags: vec![],
            secret_ref: String::new(),
            has_login: true,
            has_password: true,
            created_at: 0,
            cache: Some(CachedProfile {
                profile_icon_id: 1,
                summoner_level: 30,
                ranked: RankedQueues::default(),
                matches: Some(vec![game("RU_1"), game("RU_2")]),
                fetched_at: 0,
            }),
            history: vec![],
            archived_games: 0,
        }
    }

    /// Один кадр egui без окна: рисуем строку и возвращаем нажатое действие.
    fn frame(ctx: &egui::Context, account: &Account, events: Vec<egui::Event>) -> Option<CardAction> {
        let images = ImageStore::default();
        let state = CardState {
            in_client: false,
            refreshing: false,
            error: None,
            login_pending: false,
            images: &images,
        };
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 200.0))),
            events,
            ..Default::default()
        };
        let mut action = None;
        ctx.run_ui(input, |ui| action = show(ui, account, &state))
            .drop_without_applying_deltas();
        action
    }

    /// «Нажимает» по сетке точек строки (где именно что лежит, зависит от ширин колонок)
    /// и возвращает первое действие, для которого `wanted` вернул true.
    fn click_until(account: &Account, wanted: impl Fn(&CardAction) -> bool) -> Option<CardAction> {
        let ctx = egui::Context::default();
        frame(&ctx, account, vec![]);
        for y in [30.0, 36.0, 42.0] {
            for x in (60..1100).step_by(4) {
                let pos = pos2(x as f32, y);
                let button = |pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                };
                let on_press = frame(&ctx, account, vec![egui::Event::PointerMoved(pos), button(true)]);
                let on_release = frame(&ctx, account, vec![button(false)]);
                if let Some(action) = on_press.or(on_release).filter(&wanted) {
                    return Some(action);
                }
            }
        }
        None
    }

    /// Щелчок по игре должен дойти до окна (раньше его затирали кнопки справа).
    #[test]
    fn clicking_a_game_opens_details() {
        let action = click_until(&account_with_games(), |a| matches!(a, CardAction::OpenMatch(_)));
        match action {
            Some(CardAction::OpenMatch(id)) => assert!(id == "RU_1" || id == "RU_2", "{id}"),
            _ => panic!("щелчок по игре не открыл подробности"),
        }
    }

    /// Щелчок по рангу открывает историю именно этой очереди.
    #[test]
    fn clicking_a_rank_opens_history() {
        use crate::riot::Division;
        let mut account = account_with_games();
        let solo = RankedStats {
            tier: Tier::Gold,
            division: Division::Two,
            lp: 54,
            wins: 10,
            losses: Some(8),
        };
        if let Some(cache) = account.cache.as_mut() {
            cache.ranked.solo = Some(solo.clone());
        }
        account.history = vec![RankPoint {
            queue: Queue::Solo,
            taken_at: crate::storage::unix_now() - 3600,
            stats: solo,
        }];
        let action = click_until(&account, |a| matches!(a, CardAction::OpenHistory(_)));
        assert!(
            matches!(action, Some(CardAction::OpenHistory(Queue::Solo))),
            "щелчок по рангу Solo/Duo не открыл историю (у Flex нет ни ранга, ни истории — он не нажимается)"
        );
    }
}
