//! Сгорание ранга: когда аккаунт начнёт терять LP из-за того, что на нём не играют.
//!
//! Правила Riot (одинаковые для Solo/Duo и Flex, считаются по каждой очереди отдельно):
//! - Алмаз: каждая ранговая игра добавляет 7 «запасных» дней, запас — не больше 30.
//!   Запас кончился — минус 50 LP в день.
//! - Мастер, Грандмастер, Претендент: игра добавляет 1 день, запас — не больше 14.
//!   Запас кончился — минус 75 LP в день.
//! - Ниже Алмаза ранг не сгорает.
//!
//! Сам запас Riot отдаёт только в клиенте игры и только для своего аккаунта, поэтому мы
//! его **оцениваем** по архиву игр: идём по ранговым играм очереди от старых к новым,
//! прибавляем дни за игру и вычитаем прошедшее время. Считаем, что перед самой старой
//! известной игрой запас был полным, — поэтому оценка может быть завышена, и в окне она
//! показана со знаком «≈».
//!
//! Чистые функции без базы и окна — их легко проверить тестами.

use crate::history::{DAY, Queue};
use crate::riot::{RankedStats, Tier};
use crate::storage::Account;

/// За сколько дней до сгорания начинаем предупреждать.
pub const WARN_DAYS: i64 = 7;
/// За сколько дней до сгорания показываем уведомление Windows при запуске.
pub const URGENT_DAYS: i64 = 3;

/// Правила сгорания для ранга.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// Сколько дней можно накопить.
    pub max_days: i64,
    /// Сколько дней добавляет одна игра.
    pub days_per_game: i64,
    /// Сколько LP в день теряется, когда запас кончился.
    pub lp_per_day: i32,
}

/// Правила для ранга; `None` — этот ранг не сгорает.
pub fn rules(tier: Tier) -> Option<Rules> {
    match tier {
        Tier::Diamond => Some(Rules {
            max_days: 30,
            days_per_game: 7,
            lp_per_day: 50,
        }),
        t if t.is_apex() => Some(Rules {
            max_days: 14,
            days_per_game: 1,
            lp_per_day: 75,
        }),
        _ => None,
    }
}

/// Оценка сгорания для одной очереди.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decay {
    /// Сколько полных дней осталось до сгорания. Ноль и меньше — ранг уже сгорает.
    pub days_left: i64,
    pub rules: Rules,
}

impl Decay {
    /// Запас кончился — LP уже уходят.
    pub fn is_active(&self) -> bool {
        self.days_left <= 0
    }

    /// Пора предупреждать (осталась неделя или меньше).
    pub fn is_warning(&self) -> bool {
        self.days_left <= WARN_DAYS
    }

    /// Совсем скоро (или уже) — повод для уведомления.
    pub fn is_urgent(&self) -> bool {
        self.days_left <= URGENT_DAYS
    }

    /// Коротко, для ячейки таблицы: «↓5д» или «↓сгорает».
    pub fn short_label(&self) -> String {
        if self.is_active() {
            "↓сгорает".into()
        } else {
            format!("↓{}д", self.days_left)
        }
    }

    /// Целой фразой: «сгорание ≈ через 5 дн. (−50 LP в день)».
    pub fn label(&self) -> String {
        let lp = self.rules.lp_per_day;
        if self.is_active() {
            format!("ранг сгорает: ≈ −{lp} LP в день, пока не сыграна ранговая игра")
        } else {
            format!("сгорание ≈ через {} дн. (потом −{lp} LP в день)", self.days_left)
        }
    }
}

/// Оценка по времени ранговых игр очереди. `games` — unix-время игр, от старых к новым.
/// `None` — ранг не сгорает или игр этой очереди в архиве нет (оценить не по чему).
pub fn estimate(stats: &RankedStats, games: &[i64], now: i64) -> Option<Decay> {
    let rules = rules(stats.tier)?;
    let first = *games.first()?;
    let max = rules.max_days * DAY;
    // Запас в секундах; перед первой известной игрой считаем его полным.
    let mut bank = max;
    let mut at = first;
    for &game in games {
        // С прошлой игры запас убывал (но не ниже нуля), игра его пополнила.
        bank = (bank - (game - at).max(0)).max(0);
        bank = (bank + rules.days_per_game * DAY).min(max);
        at = at.max(game);
    }
    let left = bank - (now - at).max(0);
    Some(Decay {
        // div_euclid — деление с округлением вниз и для отрицательных чисел.
        days_left: left.div_euclid(DAY),
        rules,
    })
}

/// Оценка для очереди аккаунта (по его рангу из кэша и ранговым играм из архива).
pub fn of_account(account: &Account, queue: Queue, now: i64) -> Option<Decay> {
    let stats = queue.stats(&account.cache.as_ref()?.ranked)?;
    let games: Vec<i64> = account
        .ranked_games
        .iter()
        .filter(|(q, _)| *q == queue)
        .map(|(_, played_at)| *played_at)
        .collect();
    estimate(stats, &games, now)
}

/// Самая срочная из очередей аккаунта, о которой уже пора предупреждать.
pub fn warning(account: &Account, now: i64) -> Option<(Queue, Decay)> {
    Queue::ALL
        .into_iter()
        .filter_map(|queue| Some((queue, of_account(account, queue, now)?)))
        .filter(|(_, decay)| decay.is_warning())
        .min_by_key(|(_, decay)| decay.days_left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::Division;

    fn stats(tier: Tier) -> RankedStats {
        RankedStats {
            tier,
            division: Division::Two,
            lp: 40,
            wins: 10,
            losses: None,
        }
    }

    const NOW: i64 = 1_000 * DAY;

    #[test]
    fn low_ranks_and_unknown_games_give_nothing() {
        assert_eq!(estimate(&stats(Tier::Emerald), &[NOW - DAY], NOW), None, "ниже алмаза не сгорает");
        assert_eq!(estimate(&stats(Tier::Diamond), &[], NOW), None, "нет игр — оценить не по чему");
    }

    #[test]
    fn diamond_bank_is_capped_at_30_days() {
        // Десять игр подряд — запас всё равно 30 дней, не 70.
        let games: Vec<i64> = (0..10).map(|i| NOW - 5 * DAY + i * 3600).collect();
        let decay = estimate(&stats(Tier::Diamond), &games, NOW).unwrap();
        // Последняя игра была 5 дней без 9 часов назад: осталось 25 дней с хвостиком.
        assert_eq!(decay.days_left, 25);
        assert!(!decay.is_warning());
    }

    #[test]
    fn diamond_runs_out_after_a_long_break() {
        // Одна игра 26 дней назад: 30 − 26 = 4 дня.
        let decay = estimate(&stats(Tier::Diamond), &[NOW - 26 * DAY], NOW).unwrap();
        assert_eq!(decay.days_left, 4);
        assert!(decay.is_warning() && !decay.is_urgent() && !decay.is_active());
        assert_eq!(decay.short_label(), "↓4д");
        // 40 дней назад: запас кончился 10 дней назад.
        let decay = estimate(&stats(Tier::Diamond), &[NOW - 40 * DAY], NOW).unwrap();
        assert!(decay.is_active() && decay.is_urgent());
        assert_eq!(decay.short_label(), "↓сгорает");
    }

    #[test]
    fn bank_drains_between_games() {
        // Игра 60 дней назад (запас 30 → за 50 дней до нуля), потом игра 10 дней назад:
        // запас 0 + 7 = 7 дней, из них прошло 10 → уже сгорает.
        let games = [NOW - 60 * DAY, NOW - 10 * DAY];
        assert!(estimate(&stats(Tier::Diamond), &games, NOW).unwrap().is_active());
        // А если вторая игра была 3 дня назад — осталось 4 дня.
        let games = [NOW - 60 * DAY, NOW - 3 * DAY];
        assert_eq!(estimate(&stats(Tier::Diamond), &games, NOW).unwrap().days_left, 4);
    }

    #[test]
    fn apex_gets_one_day_per_game() {
        // Мастер: 20 дней назад запас был полным (14), с тех пор две игры 2 дня назад:
        // 14 − 18 → 0, +1, +1 = 2 дня; прошло 2 → сгорает сегодня.
        let games = [NOW - 20 * DAY, NOW - 2 * DAY, NOW - 2 * DAY];
        let decay = estimate(&stats(Tier::Master), &games, NOW).unwrap();
        assert_eq!(decay.days_left, 0);
        assert!(decay.is_active());
        assert_eq!(decay.rules.lp_per_day, 75);
    }
}
