//! История рангов: как менялись LP, что изменилось после игры («Победа · +19 LP»)
//! и сводка игровой сессии.
//!
//! Точки истории записывает база (storage/db.rs) при каждом обновлении аккаунта,
//! если ранг изменился. Здесь — только чистые функции над ними: без базы и egui,
//! поэтому их легко проверить тестами.

use crate::riot::{Division, RankedQueues, RankedStats, Tier};

/// Секунд в сутках.
pub const DAY: i64 = 86_400;

/// Сколько дней истории загружаем вместе со списком аккаунтов (и одну точку до этого —
/// с какого ранга начинался период).
pub const HISTORY_DAYS: i64 = 120;

/// Перерыв дольше этого — новая игровая сессия.
pub const SESSION_GAP: i64 = 3 * 3600;

/// «Лестница» (см. [`ladder`]): с этой отметки начинается Мастер.
pub const APEX_BASE: i32 = Tier::Master as i32 * 400;

/// Тиры с дивизионами — по порядку, индекс = номер в [`ladder`].
const DIVISION_TIERS: [Tier; 7] = [
    Tier::Iron,
    Tier::Bronze,
    Tier::Silver,
    Tier::Gold,
    Tier::Platinum,
    Tier::Emerald,
    Tier::Diamond,
];
const DIVISIONS: [Division; 4] = [Division::Four, Division::Three, Division::Two, Division::One];

/// Рейтинговая очередь.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Queue {
    Solo,
    Flex,
}

impl Queue {
    pub const ALL: [Self; 2] = [Self::Solo, Self::Flex];

    /// Как очередь записана в базе.
    pub fn key(self) -> &'static str {
        match self {
            Self::Solo => "solo",
            Self::Flex => "flex",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Solo => "Solo/Duo",
            Self::Flex => "Flex",
        }
    }

    /// Очередь, в которой сыграна игра (по `queue_id` Riot): 420 — Solo/Duo, 440 — Flex.
    pub fn from_queue_id(queue_id: i32) -> Option<Self> {
        match queue_id {
            420 => Some(Self::Solo),
            440 => Some(Self::Flex),
            _ => None,
        }
    }

    /// Ранг этой очереди из пары Solo/Flex.
    pub fn stats(self, ranked: &RankedQueues) -> Option<&RankedStats> {
        match self {
            Self::Solo => ranked.solo.as_ref(),
            Self::Flex => ranked.flex.as_ref(),
        }
    }
}

/// Ранг в одной очереди в момент обновления.
#[derive(Debug, Clone, PartialEq)]
pub struct RankPoint {
    pub queue: Queue,
    /// Unix-время, секунды.
    pub taken_at: i64,
    pub stats: RankedStats,
}

/// Ранг одним числом — чтобы рисовать график и считать разницу LP через границы
/// дивизионов. Каждый дивизион — 100 LP: Железо IV 0 LP = 0, Железо III 0 LP = 100, …,
/// Мастер 0 LP = 2800. У Мастера, Грандмастера и Претендента общая шкала LP.
pub fn ladder(stats: &RankedStats) -> i32 {
    if stats.tier.is_apex() {
        APEX_BASE + stats.lp
    } else {
        // `as i32` у enum без полей — номер варианта по порядку объявления
        // (Iron = 0 … Diamond = 6; Four = 0 … One = 3).
        stats.tier as i32 * 400 + stats.division as i32 * 100 + stats.lp
    }
}

/// Какой дивизион на этой высоте «лестницы» (для подписей графика): 1350 → Золото III.
/// Мастер и выше — без дивизиона.
pub fn division_at(value: i32) -> (Tier, Option<Division>) {
    if value >= APEX_BASE {
        return (Tier::Master, None);
    }
    let value = value.max(0);
    let tier = DIVISION_TIERS[(value / 400) as usize];
    let division = DIVISIONS[((value % 400) / 100) as usize];
    (tier, Some(division))
}

/// «Золото III» / «Мастер» — подпись деления графика.
pub fn division_label(value: i32) -> String {
    match division_at(value) {
        (tier, Some(division)) => format!("{} {}", tier.label_ru(), division.roman()),
        (tier, None) => tier.label_ru().to_string(),
    }
}

/// Ранг изменился (стоит записать новую точку истории)? Поражения сравниваем, только
/// если они известны в обеих записях: клиент League скрывает их у чужих аккаунтов.
pub fn has_progress(old: &RankedStats, new: &RankedStats) -> bool {
    old.tier != new.tier
        || old.division != new.division
        || old.lp != new.lp
        || old.wins != new.wins
        || matches!((old.losses, new.losses), (Some(a), Some(b)) if a != b)
}

/// «+19 LP», «−7 LP», «±0 LP».
pub fn signed_lp(lp: i32) -> String {
    match lp {
        0 => "±0 LP".to_string(),
        lp if lp > 0 => format!("+{lp} LP"),
        lp => format!("−{} LP", -lp),
    }
}

// ----- Изменение за период --------------------------------------------------

/// Сколько LP, побед и поражений добавилось между двумя рангами.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub lp: i32,
    pub wins: u32,
    /// `None` — поражения неизвестны (см. [`RankedStats::losses`]).
    pub losses: Option<u32>,
}

impl Progress {
    pub fn between(from: &RankedStats, to: &RankedStats) -> Self {
        Self {
            lp: ladder(to) - ladder(from),
            // saturating_sub — не уходить ниже нуля (после сброса сезона победы обнуляются).
            wins: to.wins.saturating_sub(from.wins),
            losses: match (from.losses, to.losses) {
                (Some(from), Some(to)) => Some(to.saturating_sub(from)),
                _ => None,
            },
        }
    }

    /// Сколько игр; `None`, если поражения неизвестны.
    pub fn games(&self) -> Option<u32> {
        Some(self.wins + self.losses?)
    }

    /// «3W 2L» или «3 победы», если поражения неизвестны.
    pub fn record(&self) -> String {
        match self.losses {
            Some(losses) => format!("{}W {losses}L", self.wins),
            None => format!("{}W", self.wins),
        }
    }
}

/// Точки одной очереди по порядку времени.
fn of_queue(history: &[RankPoint], queue: Queue) -> impl DoubleEndedIterator<Item = &RankPoint> {
    history.iter().filter(move |p| p.queue == queue)
}

/// Что изменилось в очереди с момента `since`: последняя точка против ранга на момент
/// `since` (последней точки до него, а если истории тогда ещё не было — первой после).
/// `None` — за период ничего не менялось или сравнивать не с чем.
pub fn progress_since(history: &[RankPoint], queue: Queue, since: i64) -> Option<Progress> {
    let last = of_queue(history, queue).next_back()?;
    if last.taken_at < since {
        return None; // за период изменений не было
    }
    let base = of_queue(history, queue)
        .rev()
        .find(|p| p.taken_at < since)
        .or_else(|| of_queue(history, queue).next())?;
    // ptr::eq — это одна и та же точка (единственная): сравнивать не с чем.
    (!std::ptr::eq(base, last)).then(|| Progress::between(&base.stats, &last.stats))
}

/// Точки графика очереди за период: (время, «лестница»). Начинается с ранга на момент
/// `since` (если он известен), заканчивается текущим рангом в момент `now`.
pub fn series(history: &[RankPoint], queue: Queue, since: i64, now: i64) -> Vec<(i64, i32)> {
    let mut out = Vec::new();
    if let Some(base) = of_queue(history, queue).rev().find(|p| p.taken_at < since) {
        out.push((since, ladder(&base.stats)));
    }
    out.extend(
        of_queue(history, queue)
            .filter(|p| p.taken_at >= since)
            .map(|p| (p.taken_at, ladder(&p.stats))),
    );
    if let Some(&(time, value)) = out.last()
        && time < now
    {
        out.push((now, value));
    }
    out
}

/// Изменения по очереди — пары (предыдущая точка, точка), новые первыми.
pub fn steps(history: &[RankPoint], queue: Queue) -> Vec<(&RankPoint, &RankPoint)> {
    let points: Vec<&RankPoint> = of_queue(history, queue).collect();
    points.windows(2).rev().map(|pair| (pair[0], pair[1])).collect()
}

// ----- Что изменилось после обновления --------------------------------------

/// Изменение ранга в одной очереди (для карточки «после игры»).
#[derive(Debug, Clone, PartialEq)]
pub struct RankChange {
    pub queue: Queue,
    /// `None` — до этого ранга не было (закончилась калибровка).
    pub before: Option<RankedStats>,
    pub after: RankedStats,
}

impl RankChange {
    /// `None` — сравнивать не с чем (калибровка).
    pub fn progress(&self) -> Option<Progress> {
        self.before.as_ref().map(|before| Progress::between(before, &self.after))
    }

    /// Сменился дивизион или тир: `Some(true)` — повышение, `Some(false)` — понижение.
    pub fn promotion(&self) -> Option<bool> {
        let before = self.before.as_ref()?;
        let old = (before.tier, before.division);
        let new = (self.after.tier, self.after.division);
        (old != new).then_some(new > old)
    }
}

/// Что изменилось в рангах после обновления. Пустой список — ничего.
pub fn rank_changes(before: &RankedQueues, after: &RankedQueues) -> Vec<RankChange> {
    Queue::ALL
        .into_iter()
        .filter_map(|queue| {
            let new = queue.stats(after)?;
            let old = queue.stats(before);
            // is_none_or: ранга не было (калибровка) или он изменился.
            old.is_none_or(|old| has_progress(old, new)).then(|| RankChange {
                queue,
                before: old.cloned(),
                after: new.clone(),
            })
        })
        .collect()
}

// ----- Игровая сессия --------------------------------------------------------

/// Игры без долгих перерывов: сколько сыграно и сколько LP получено.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// Первое изменение сессии (unix-время).
    pub started_at: i64,
    /// Очереди, в которых за сессию что-то изменилось.
    pub queues: Vec<(Queue, Progress)>,
}

impl Session {
    pub fn wins(&self) -> u32 {
        self.queues.iter().map(|(_, p)| p.wins).sum()
    }

    /// `None` — где-то поражения неизвестны.
    pub fn losses(&self) -> Option<u32> {
        self.queues.iter().map(|(_, p)| p.losses).sum()
    }
}

/// Текущая сессия: изменения ранга, между которыми не было перерывов дольше
/// [`SESSION_GAP`], и последнее — не раньше `SESSION_GAP` назад. После каждой игры
/// аккаунт обновляется, поэтому точка истории ≈ игра. `history` — по порядку времени.
pub fn session(history: &[RankPoint], now: i64) -> Option<Session> {
    let last = history.last()?;
    if now - last.taken_at > SESSION_GAP {
        return None;
    }
    // Идём назад, пока перерывы между изменениями короче SESSION_GAP.
    let mut start = history.len() - 1;
    while start > 0 && history[start].taken_at - history[start - 1].taken_at <= SESSION_GAP {
        start -= 1;
    }
    let (before, during) = history.split_at(start);
    let queues: Vec<(Queue, Progress)> = Queue::ALL
        .into_iter()
        .filter_map(|queue| {
            let end = of_queue(during, queue).next_back()?;
            // Ранг перед сессией; если истории до неё нет — первая точка сессии.
            let base = of_queue(before, queue)
                .next_back()
                .or_else(|| of_queue(during, queue).next())?;
            (!std::ptr::eq(base, end)).then(|| (queue, Progress::between(&base.stats, &end.stats)))
        })
        .collect();
    (!queues.is_empty()).then(|| Session {
        started_at: during[0].taken_at,
        queues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(tier: Tier, division: Division, lp: i32, wins: u32, losses: Option<u32>) -> RankedStats {
        RankedStats {
            tier,
            division,
            lp,
            wins,
            losses,
        }
    }

    fn gold(division: Division, lp: i32, wins: u32, losses: u32) -> RankedStats {
        stats(Tier::Gold, division, lp, wins, Some(losses))
    }

    fn point(queue: Queue, taken_at: i64, stats: RankedStats) -> RankPoint {
        RankPoint { queue, taken_at, stats }
    }

    #[test]
    fn ladder_is_continuous_across_divisions_and_apex() {
        assert_eq!(ladder(&stats(Tier::Iron, Division::Four, 0, 0, None)), 0);
        assert_eq!(ladder(&gold(Division::Three, 50, 0, 0)), 3 * 400 + 100 + 50);
        // Алмаз I 100 LP — то же, что Мастер 0 LP.
        assert_eq!(ladder(&stats(Tier::Diamond, Division::One, 100, 0, None)), APEX_BASE);
        assert_eq!(ladder(&stats(Tier::Grandmaster, Division::One, 350, 0, None)), APEX_BASE + 350);

        assert_eq!(division_label(1250), "Золото IV");
        assert_eq!(division_label(1350), "Золото III");
        assert_eq!(division_label(0), "Железо IV");
        assert_eq!(division_label(APEX_BASE + 10), "Мастер");
        assert_eq!(signed_lp(19), "+19 LP");
        assert_eq!(signed_lp(-7), "−7 LP");
        assert_eq!(signed_lp(0), "±0 LP");
    }

    #[test]
    fn progress_ignores_unknown_losses() {
        let old = gold(Division::Two, 40, 10, 8);
        assert!(!has_progress(&old, &old.clone()));
        assert!(has_progress(&old, &gold(Division::Two, 40, 10, 9)), "поражение на 0 LP");
        // Поражения скрыты в новой записи — по ним не сравниваем.
        let hidden = stats(Tier::Gold, Division::Two, 40, 10, None);
        assert!(!has_progress(&old, &hidden));

        // Повышение: Золото II 90 → Золото I 8 = +18 LP.
        let p = Progress::between(&gold(Division::Two, 90, 10, 8), &gold(Division::One, 8, 11, 8));
        assert_eq!((p.lp, p.wins, p.losses), (18, 1, Some(0)));
        assert_eq!(p.record(), "1W 0L");
        assert_eq!(Progress::between(&old, &hidden).losses, None);
    }

    #[test]
    fn rank_changes_and_promotions() {
        let before = RankedQueues {
            solo: Some(gold(Division::Two, 90, 10, 8)),
            flex: None,
        };
        let after = RankedQueues {
            solo: Some(gold(Division::One, 8, 11, 8)),
            flex: Some(stats(Tier::Silver, Division::One, 0, 6, Some(4))),
        };
        let changes = rank_changes(&before, &after);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].queue, Queue::Solo);
        assert_eq!(changes[0].promotion(), Some(true));
        assert_eq!(changes[0].progress().unwrap().lp, 18);
        // Flex: ранга не было — закончилась калибровка.
        assert_eq!(changes[1].before, None);
        assert_eq!(changes[1].promotion(), None);

        assert!(rank_changes(&after, &after).is_empty());
        let demoted = RankChange {
            queue: Queue::Solo,
            before: after.solo.clone(),
            after: gold(Division::Two, 75, 11, 9),
        };
        assert_eq!(demoted.promotion(), Some(false));
    }

    #[test]
    fn progress_and_series_over_a_period() {
        let now = 100 * DAY;
        let history = vec![
            point(Queue::Solo, now - 20 * DAY, gold(Division::Three, 20, 1, 1)),
            point(Queue::Flex, now - 6 * DAY, gold(Division::Four, 0, 1, 0)),
            point(Queue::Solo, now - 5 * DAY, gold(Division::Three, 40, 2, 1)),
            point(Queue::Solo, now - DAY, gold(Division::Two, 10, 4, 1)),
        ];
        // За неделю: от Золота III 20 LP (точка до периода) до Золота II 10 LP = +90.
        let week = progress_since(&history, Queue::Solo, now - 7 * DAY).unwrap();
        assert_eq!((week.lp, week.wins, week.losses), (90, 3, Some(0)));
        // За 2 дня: только последняя игра.
        assert_eq!(progress_since(&history, Queue::Solo, now - 2 * DAY).unwrap().lp, 70);
        // Flex: одна точка — сравнивать не с чем.
        assert_eq!(progress_since(&history, Queue::Flex, now - 7 * DAY), None);
        // Последнее изменение раньше периода — за период ничего.
        assert_eq!(progress_since(&history, Queue::Solo, now - DAY / 2), None);

        let line = series(&history, Queue::Solo, now - 7 * DAY, now);
        let base = ladder(&gold(Division::Three, 20, 0, 0));
        assert_eq!(
            line,
            vec![
                (now - 7 * DAY, base),
                (now - 5 * DAY, base + 20),
                (now - DAY, base + 90),
                (now, base + 90),
            ]
        );

        let steps = steps(&history, Queue::Solo);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].1.taken_at, now - DAY, "новые первыми");
    }

    #[test]
    fn session_stops_at_a_long_break() {
        let now = 50 * DAY;
        let hour = 3600;
        let history = vec![
            point(Queue::Solo, now - 2 * DAY, gold(Division::Two, 50, 10, 10)),
            // Сессия: три игры с перерывами меньше трёх часов.
            point(Queue::Solo, now - 3 * hour, gold(Division::Two, 70, 11, 10)),
            point(Queue::Flex, now - 2 * hour, gold(Division::Four, 10, 3, 3)),
            point(Queue::Solo, now - hour, gold(Division::Two, 52, 11, 11)),
        ];
        let s = session(&history, now).unwrap();
        assert_eq!(s.started_at, now - 3 * hour);
        // Solo: от точки до сессии (50 LP) до последней (52 LP).
        assert_eq!(s.queues[0], (Queue::Solo, Progress { lp: 2, wins: 1, losses: Some(1) }));
        // Flex: до сессии точек нет, а в сессии одна — сравнивать не с чем.
        assert_eq!(s.queues.len(), 1);
        assert_eq!((s.wins(), s.losses()), (1, Some(1)));

        // Последняя игра давно — сессии нет.
        assert_eq!(session(&history, now + 4 * hour), None);
        // Истории до сессии нет: считаем от первой точки сессии.
        let fresh = &history[1..];
        assert_eq!(session(fresh, now).unwrap().queues[0].1.lp, -18);
    }
}
