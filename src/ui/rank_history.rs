//! История ранга в интерфейсе: «+34» и мини-график на строке аккаунта, график при
//! наведении на ранг и окно «История ранга» (щелчок по рангу).

use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};

use crate::history::{self, APEX_BASE, DAY, Progress, Queue, RankPoint};
use crate::riot::RankedStats;
use crate::storage::{Account, AccountId, unix_now};
use crate::ui::account_card;
use crate::ui::format;
use crate::ui::image_store::ImageStore;
use crate::ui::theme::{self, Weight, alpha, white};
use crate::ui::widgets::{self, Icon, IconButton, Segment, Tone};

/// За сколько дней «+34» на строке аккаунта.
pub const TREND_DAYS: i64 = 7;

const DIALOG_WIDTH: f32 = 660.0;

/// Период графика в окне истории.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Week,
    Month,
    All,
}

impl Period {
    pub fn days(self) -> i64 {
        match self {
            Self::Week => 7,
            Self::Month => 30,
            Self::All => history::HISTORY_DAYS,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Week => "7 дней",
            Self::Month => "30 дней",
            Self::All => "120 дней",
        }
    }
}

/// Цвет изменения LP: рост — зелёный, падение — красный.
pub fn lp_color(lp: i32) -> Color32 {
    match lp {
        0 => theme::TEXT_MUTED,
        lp if lp > 0 => theme::GREEN_TEXT,
        _ => theme::RED_TEXT,
    }
}

/// «+34», «−7», «±0» — коротко, для строки аккаунта.
fn short_lp(lp: i32) -> String {
    history::signed_lp(lp).trim_end_matches(" LP").to_string()
}

// ----- На строке аккаунта ------------------------------------------------------

/// Мини-график и «+34» за [`TREND_DAYS`] дней — справа от заголовка ранга.
/// Рисуется в раскладке справа налево: сначала число (оно правее), потом график.
/// Ничего, если за этот срок ранг не менялся.
pub fn trend(ui: &mut egui::Ui, history: &[RankPoint], queue: Queue) {
    let now = unix_now();
    let since = now - TREND_DAYS * DAY;
    let Some(progress) = history::progress_since(history, queue, since) else {
        return;
    };
    let color = lp_color(progress.lp);
    ui.label(RichText::new(short_lp(progress.lp)).size(10.5).semibold().color(color));
    let (rect, _) = ui.allocate_exact_size(vec2(28.0, 11.0), Sense::hover());
    let series = history::series(history, queue, since, now);
    sparkline(ui.painter(), rect, &series, (since, now), color);
}

/// Мини-график без подписей.
fn sparkline(painter: &egui::Painter, rect: Rect, series: &[(i64, i32)], time: (i64, i64), color: Color32) {
    let Some(points) = project(rect, series, time, value_range(series)) else {
        return;
    };
    let last = points.last().copied();
    painter.line(points, Stroke::new(1.4, color));
    if let Some(last) = last {
        painter.circle_filled(last, 1.8, color);
    }
}

/// Подсказка при наведении на ранг: график за 30 дней и изменения за неделю и месяц.
pub fn preview(ui: &mut egui::Ui, history: &[RankPoint], queue: Queue, stats: Option<&RankedStats>) {
    ui.set_max_width(320.0);
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.label(theme::caps(&format!("{} · история за 30 дней", queue.label())));
    let now = unix_now();
    if history.iter().any(|p| p.queue == queue) {
        chart(ui, history, queue, now - 30 * DAY, now, vec2(310.0, 110.0), false);
        for (days, title) in [(TREND_DAYS, "За 7 дней"), (30, "За 30 дней")] {
            let text = match history::progress_since(history, queue, now - days * DAY) {
                Some(p) => format!("{title}: {} · {}", history::signed_lp(p.lp), p.record()),
                None => format!("{title}: без изменений"),
            };
            ui.label(RichText::new(text).size(11.5).color(theme::TEXT));
        }
    } else {
        ui.add(
            egui::Label::new(
                RichText::new(
                    "История начнёт копиться с этого обновления: приложение запоминает \
                     ранг каждый раз, когда он меняется.",
                )
                .size(11.5)
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );
    }
    if stats.is_some_and(|s| s.losses.is_none()) {
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Поражения скрыты: клиент League показывает их только у аккаунта, в который \
                     выполнен вход. С API-ключом (Настройки) они загрузятся из Riot API.",
                )
                .size(11.0)
                .color(theme::AMBER_TEXT),
            )
            .wrap(),
        );
    }
    ui.label(RichText::new("Щёлкните — подробная история").size(10.5).color(theme::TEXT_HINT));
}

// ----- График ---------------------------------------------------------------------

/// Диапазон значений ряда (с запасом, если ранг не менялся — иначе линия легла бы на край).
fn value_range(series: &[(i64, i32)]) -> (f32, f32) {
    let min = series.iter().map(|p| p.1).min().unwrap_or(0) as f32;
    let max = series.iter().map(|p| p.1).max().unwrap_or(0) as f32;
    if max - min < 1.0 { (min - 1.0, max + 1.0) } else { (min, max) }
}

/// Точки ряда → координаты в прямоугольнике. `None` — меньше двух точек.
fn project(rect: Rect, series: &[(i64, i32)], time: (i64, i64), values: (f32, f32)) -> Option<Vec<Pos2>> {
    if series.len() < 2 {
        return None;
    }
    let span_t = (time.1 - time.0).max(1) as f32;
    let span_v = (values.1 - values.0).max(1.0);
    Some(
        series
            .iter()
            .map(|&(t, v)| {
                pos2(
                    rect.left() + (t - time.0) as f32 / span_t * rect.width(),
                    rect.bottom() - (v as f32 - values.0) / span_v * rect.height(),
                )
            })
            .collect(),
    )
}

/// Подпись полосы графика, которая начинается на высоте `value`.
fn band_label(value: i32, step: i32) -> String {
    if value >= APEX_BASE {
        format!("{} LP", value - APEX_BASE)
    } else if step >= 400 {
        history::division_at(value).0.label_ru().to_string()
    } else {
        history::division_label(value)
    }
}

/// График ранга: полосы дивизионов с подписями слева, линия с заливкой, точки изменений.
/// `detailed` — подписи времени снизу и ранг в точке при наведении.
pub fn chart(
    ui: &mut egui::Ui,
    history: &[RankPoint],
    queue: Queue,
    since: i64,
    now: i64,
    size: Vec2,
    detailed: bool,
) {
    const LABELS: f32 = 82.0;
    let axis = if detailed { 18.0 } else { 2.0 };
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let plot = Rect::from_min_max(
        pos2(rect.left() + LABELS, rect.top() + 4.0),
        pos2(rect.right() - 6.0, rect.bottom() - axis),
    );
    painter.rect_filled(plot, 6, white(4));

    let series = history::series(history, queue, since, now);
    if series.len() < 2 {
        painter.text(
            plot.center(),
            Align2::CENTER_CENTER,
            "Пока нет изменений за этот период",
            FontId::proportional(12.0),
            theme::TEXT_HINT,
        );
        return;
    }

    // Диапазон по вертикали — целыми дивизионами (или тирами, если разброс большой).
    let min = series.iter().map(|p| p.1).min().unwrap_or(0);
    let max = series.iter().map(|p| p.1).max().unwrap_or(0);
    let step = if max - min > 600 { 400 } else { 100 };
    let lo = (min - 8).max(0).div_euclid(step) * step;
    let hi = (max + 8 + step - 1).div_euclid(step) * step;
    let y = |v: i32| plot.bottom() - (v - lo) as f32 / (hi - lo) as f32 * plot.height();

    // Полосы дивизионов: чуть подкрашены цветом тира, подпись — по центру полосы.
    let mut band = lo;
    while band < hi {
        let top = y(band + step);
        let bottom = y(band);
        let tier = history::division_at(band).0;
        let color = theme::tier_color(Some(tier));
        let band_rect = Rect::from_x_y_ranges(plot.x_range(), top..=bottom);
        painter.rect_filled(band_rect, 0, alpha(color, 8));
        painter.hline(plot.x_range(), bottom, Stroke::new(1.0, white(12)));
        if bottom - top >= 12.0 {
            painter.text(
                pos2(rect.left(), (top + bottom) / 2.0),
                Align2::LEFT_CENTER,
                band_label(band, step),
                FontId::proportional(10.5),
                alpha(color, 200),
            );
        }
        band += step;
    }

    let Some(points) = project(plot, &series, (since, now), (lo as f32, hi as f32)) else {
        return;
    };
    // Заливка под линией: от цвета линии к прозрачному у нижнего края.
    let line_color = theme::ACCENT_LIGHT;
    let mut mesh = egui::Mesh::default();
    for (index, point) in points.iter().enumerate() {
        mesh.colored_vertex(*point, alpha(line_color, 46));
        mesh.colored_vertex(pos2(point.x, plot.bottom()), alpha(line_color, 0));
        if index > 0 {
            let i = index as u32 * 2;
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i + 1, i);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.line(points, Stroke::new(2.0, line_color));

    // Точки изменений: рост — зелёная, падение — красная.
    let changes: Vec<(&RankPoint, Option<&RankPoint>, Pos2)> = {
        let in_queue: Vec<&RankPoint> = history.iter().filter(|p| p.queue == queue).collect();
        in_queue
            .iter()
            .enumerate()
            .filter(|(_, p)| p.taken_at >= since)
            .map(|(i, p)| {
                let previous = i.checked_sub(1).map(|j| in_queue[j]);
                let x = plot.left() + (p.taken_at - since) as f32 / (now - since).max(1) as f32 * plot.width();
                (*p, previous, pos2(x, y(history::ladder(&p.stats))))
            })
            .collect()
    };
    for (point, previous, pos) in &changes {
        let delta = previous.map_or(0, |prev| history::ladder(&point.stats) - history::ladder(&prev.stats));
        let color = if delta > 0 { theme::GREEN } else if delta < 0 { theme::RED } else { theme::ACCENT_LIGHT };
        painter.circle_filled(*pos, 3.2, theme::CARD);
        painter.circle_stroke(*pos, 3.2, Stroke::new(1.6, color));
    }

    if !detailed {
        return;
    }
    // Подписи времени снизу.
    let days = (now - since) / DAY;
    for (x, text) in [
        (plot.left(), format!("{days} дн назад")),
        (plot.center().x, format!("{} дн назад", days / 2)),
        (plot.right(), "сейчас".to_string()),
    ] {
        let anchor = if x == plot.left() {
            Align2::LEFT_TOP
        } else if x == plot.right() {
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        painter.text(pos2(x, plot.bottom() + 4.0), anchor, text, FontId::proportional(10.0), theme::TEXT_HINT);
    }

    // Наведение: ближайшая точка изменения по горизонтали.
    let Some(pointer) = response.hover_pos() else {
        return;
    };
    let nearest = changes
        .iter()
        .min_by(|a, b| (a.2.x - pointer.x).abs().total_cmp(&(b.2.x - pointer.x).abs()))
        .filter(|c| (c.2.x - pointer.x).abs() < 24.0);
    if let Some((point, previous, pos)) = nearest {
        painter.vline(pos.x, plot.y_range(), Stroke::new(1.0, white(40)));
        painter.circle_filled(*pos, 4.5, theme::ACCENT_LIGHT);
        let stats = &point.stats;
        let delta = previous.map(|prev| Progress::between(&prev.stats, stats));
        response.on_hover_ui_at_pointer(|ui| {
            ui.label(
                RichText::new(format!("{} · {} LP", stats.rank_label(), stats.lp))
                    .size(12.0)
                    .semibold()
                    .color(theme::tier_color(Some(stats.tier))),
            );
            if let Some(delta) = delta {
                ui.label(
                    RichText::new(format!("{} · {}", history::signed_lp(delta.lp), delta.record()))
                        .size(11.5)
                        .color(lp_color(delta.lp)),
                );
            }
            ui.label(RichText::new(format::relative_time(point.taken_at)).size(11.0).color(theme::TEXT_MUTED));
        });
    }
}

// ----- Окно «История ранга» ---------------------------------------------------------

/// Открытое окно истории.
pub struct HistoryDialog {
    pub account: AccountId,
    pub queue: Queue,
    pub period: Period,
}

impl HistoryDialog {
    pub fn new(account: AccountId, queue: Queue) -> Self {
        Self {
            account,
            queue,
            period: Period::Month,
        }
    }

    /// Возвращает true, если окно закрыли.
    pub fn show(&mut self, ctx: &egui::Context, account: &Account, images: &ImageStore) -> bool {
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("rank_history"))
            .frame(theme::dialog_frame())
            .show(ctx, |ui| {
                ui.set_width(DIALOG_WIDTH);
                close = self.header(ui, account, images);
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let queues = Queue::ALL.map(|q| Segment::new(q, q.label()));
                    widgets::segmented(ui, "history_queue", &mut self.queue, &queues, 30.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let periods = [Period::Week, Period::Month, Period::All].map(|p| Segment::new(p, p.label()));
                        widgets::segmented(ui, "history_period", &mut self.period, &periods, 30.0);
                    });
                });
                ui.add_space(12.0);

                let now = unix_now();
                let since = now - self.period.days() * DAY;
                let history = &account.history;
                let current = account.cache.as_ref().and_then(|c| self.queue.stats(&c.ranked));
                stat_tiles(ui, history, self.queue, since, current);
                ui.add_space(12.0);
                // Окно приложения может быть низким: график и список делят то, что осталось
                // после заголовка, переключателей и плиток (~300 px).
                let room = (ctx.content_rect().height() - 300.0).max(180.0);
                let chart_height = (room * 0.6).clamp(120.0, 230.0);
                let list_height = (room - chart_height - 40.0).clamp(60.0, 170.0);
                chart(ui, history, self.queue, since, now, vec2(DIALOG_WIDTH, chart_height), true);
                ui.add_space(14.0);
                changes_list(ui, history, self.queue, since, list_height);
            });
        close || modal.should_close()
    }

    /// Эмблема, «История ранга», Riot ID и текущий ранг. Возвращает true — «закрыть».
    fn header(&self, ui: &mut egui::Ui, account: &Account, images: &ImageStore) -> bool {
        let mut close = false;
        let current = account.cache.as_ref().map(|c| self.queue.stats(&c.ranked));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let tier = current.flatten().map(|s| s.tier);
            account_card::emblem(ui, images, tier, current.is_some(), 46.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(RichText::new("История ранга").size(16.0).semibold().color(theme::TEXT));
                    widgets::tag(ui, &account.riot_id.to_string(), Tone::Neutral);
                });
                let text = match current {
                    Some(Some(s)) => format!("{} · {} · {} LP", self.queue.label(), s.rank_label(), s.lp),
                    Some(None) => format!("{} · без ранга", self.queue.label()),
                    None => format!("{} · нет данных", self.queue.label()),
                };
                ui.label(RichText::new(text).size(12.0).color(theme::TEXT_MUTED));
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                close = ui.add(IconButton::new(Icon::Close)).on_hover_text("Закрыть (Esc)").clicked();
            });
        });
        close
    }
}

/// Четыре плитки: изменение LP, игры, пик и текущий ранг.
fn stat_tiles(ui: &mut egui::Ui, history: &[RankPoint], queue: Queue, since: i64, current: Option<&RankedStats>) {
    const GAP: f32 = 10.0;
    let width = (DIALOG_WIDTH - GAP * 3.0) / 4.0;
    let progress = history::progress_since(history, queue, since);
    // Пик: самая высокая точка за период (вместе с рангом на его начало).
    let peak = history
        .iter()
        .filter(|p| p.queue == queue)
        .filter(|p| p.taken_at >= since)
        .chain(history.iter().rev().find(|p| p.queue == queue && p.taken_at < since))
        .max_by_key(|p| history::ladder(&p.stats));

    let tiles: [(&str, String, Color32, String); 4] = [
        match &progress {
            Some(p) => ("Изменение", history::signed_lp(p.lp), lp_color(p.lp), "за период".into()),
            None => ("Изменение", "±0 LP".into(), theme::TEXT_MUTED, "без изменений".into()),
        },
        match &progress {
            Some(p) => match p.games() {
                Some(games) => ("Игры", games.to_string(), theme::TEXT, p.record()),
                None => ("Игры", format!("{}W", p.wins), theme::TEXT, "поражения скрыты".into()),
            },
            None => ("Игры", "0".into(), theme::TEXT_MUTED, "за период".into()),
        },
        match peak {
            Some(p) => (
                "Пик",
                p.stats.rank_label(),
                theme::tier_color(Some(p.stats.tier)),
                format!("{} LP", p.stats.lp),
            ),
            None => ("Пик", "—".into(), theme::TEXT_MUTED, "нет данных".into()),
        },
        match current {
            Some(s) => (
                "Сейчас",
                s.rank_label(),
                theme::tier_color(Some(s.tier)),
                match (s.winrate(), s.games()) {
                    (Some(wr), Some(games)) => format!("{} LP · {wr:.0}% из {games}", s.lp),
                    _ => format!("{} LP · {} побед", s.lp, s.wins),
                },
            ),
            None => ("Сейчас", "Без ранга".into(), theme::TEXT_MUTED, String::new()),
        },
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for (title, value, color, sub) in tiles {
            egui::Frame::new()
                .fill(white(5))
                .stroke(Stroke::new(1.0, white(14)))
                .corner_radius(theme::RADIUS)
                .inner_margin(Margin::symmetric(12, 9))
                .show(ui, |ui| {
                    // Внутри `horizontal` рамка тоже раскладывает в строку — нужен столбец.
                    ui.vertical(|ui| {
                        ui.set_width(width - 24.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(theme::caps(title));
                        ui.add(egui::Label::new(RichText::new(value).size(16.0).semibold().color(color)).truncate());
                        ui.add(egui::Label::new(RichText::new(sub).size(10.5).color(theme::TEXT_MUTED)).truncate());
                    });
                });
        }
    });
}

/// Список изменений за период, новые сверху; не выше `max_height`, дальше прокрутка.
fn changes_list(ui: &mut egui::Ui, history: &[RankPoint], queue: Queue, since: i64, max_height: f32) {
    let steps: Vec<_> = history::steps(history, queue)
        .into_iter()
        .filter(|(_, point)| point.taken_at >= since)
        .collect();
    ui.label(theme::caps(&format!("Изменения · {}", steps.len())));
    ui.add_space(4.0);
    if steps.is_empty() {
        let text = if history.iter().any(|p| p.queue == queue) {
            "За этот период ранг не менялся."
        } else {
            "История пока пуста. Приложение запоминает ранг при каждом обновлении — \
             после следующих игр здесь появятся изменения и график."
        };
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(theme::TEXT_MUTED)).wrap());
        return;
    }
    // Высота задана явно, как в окне игры: иначе в модальном окне область прокрутки
    // берёт место, оставшееся с прошлого кадра, и сжимается до пары строк.
    let height = max_height.min(steps.len() as f32 * 30.0);
    egui::ScrollArea::vertical()
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (previous, point) in steps {
                change_row(ui, previous, point);
            }
        });
}

/// «3 ч назад · Золото II · 54 LP · Повышение · +19 LP · 1W».
fn change_row(ui: &mut egui::Ui, previous: &RankPoint, point: &RankPoint) {
    let delta = Progress::between(&previous.stats, &point.stats);
    let stats = &point.stats;
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 26.0), Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.allocate_ui_with_layout(vec2(96.0, 20.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_width(96.0);
            ui.label(RichText::new(format::relative_time(point.taken_at)).size(11.5).color(theme::TEXT_MUTED));
        });
        ui.label(
            RichText::new(stats.rank_label())
                .size(12.5)
                .semibold()
                .color(theme::tier_color(Some(stats.tier))),
        );
        ui.label(RichText::new(format!("{} LP", stats.lp)).size(12.0).color(theme::TEXT));
        let change = history::RankChange {
            queue: point.queue,
            before: Some(previous.stats.clone()),
            after: stats.clone(),
        };
        match change.promotion() {
            Some(true) => {
                widgets::tag(ui, "Повышение", Tone::Green);
            }
            Some(false) => {
                widgets::tag(ui, "Понижение", Tone::Red);
            }
            None => {}
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(delta.record()).size(11.5).color(theme::TEXT_MUTED));
            ui.label(
                RichText::new(history::signed_lp(delta.lp))
                    .size(12.5)
                    .semibold()
                    .color(lp_color(delta.lp)),
            );
        });
    });
}
