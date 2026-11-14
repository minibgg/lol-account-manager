//! Сводка по всем аккаунтам за период — для окна «Сводка» (клавиша d): сколько LP
//! набрано и потеряно, побед и поражений, у кого лучший ранг, чей ранг сгорает.
//!
//! Чистые функции над списком аккаунтов (без базы и окна) — их легко проверить тестами.

use crate::decay::{self, Decay};
use crate::history::{self, Queue};
use crate::riot::RankedStats;
use crate::storage::{Account, AccountId};

/// Строка сводки: один аккаунт.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: AccountId,
    /// «Имя#TAG».
    pub name: String,
    /// Ранг для показа: Solo/Duo, а если его нет — Flex.
    pub rank: Option<(Queue, RankedStats)>,
    /// Изменение LP за период по обеим очередям. `None` — за период ничего не менялось.
    pub lp: Option<i32>,
    pub wins: u32,
    /// `None` — у какой-то из очередей поражения неизвестны (клиент League их скрывает).
    pub losses: Option<u32>,
    /// Ранг скоро начнёт сгорать (или уже сгорает).
    pub decay: Option<(Queue, Decay)>,
}

/// Вся сводка.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Overview {
    /// Аккаунты: сначала те, у кого за период что-то менялось (по убыванию LP), потом
    /// остальные по рангу.
    pub rows: Vec<Row>,
    /// Сумма изменений LP по всем аккаунтам.
    pub lp: i32,
    pub wins: u32,
    /// Известные поражения (у аккаунтов, где клиент их скрывает, не считаются).
    pub losses: u32,
    /// У скольких аккаунтов за период что-то менялось.
    pub active: usize,
    /// Сколько игр всего в архиве.
    pub archived_games: usize,
}

impl Overview {
    /// Аккаунт с самым высоким рангом.
    pub fn best(&self) -> Option<&Row> {
        self.rows
            .iter()
            .filter(|row| row.rank.is_some())
            .max_by_key(|row| row.rank.as_ref().map(|(_, stats)| history::ladder(stats)))
    }

    /// У скольких аккаунтов ранг скоро начнёт сгорать.
    pub fn decaying(&self) -> usize {
        self.rows.iter().filter(|row| row.decay.is_some()).count()
    }
}

/// Сводка за время с `since` до `now` (unix-время).
pub fn build(accounts: &[Account], since: i64, now: i64) -> Overview {
    let mut overview = Overview::default();
    for account in accounts {
        let mut row = Row {
            id: account.id,
            name: account.riot_id.to_string(),
            rank: account.cache.as_ref().and_then(|cache| {
                Queue::ALL
                    .into_iter()
                    .find_map(|queue| Some((queue, queue.stats(&cache.ranked)?.clone())))
            }),
            lp: None,
            wins: 0,
            losses: Some(0),
            decay: decay::warning(account, now),
        };
        for queue in Queue::ALL {
            let Some(progress) = history::progress_since(&account.history, queue, since) else {
                continue;
            };
            row.lp = Some(row.lp.unwrap_or(0) + progress.lp);
            row.wins += progress.wins;
            // Поражения известны, только если известны в каждой из очередей.
            row.losses = match (row.losses, progress.losses) {
                (Some(a), Some(b)) => Some(a + b),
                _ => None,
            };
        }
        if let Some(lp) = row.lp {
            overview.active += 1;
            overview.lp += lp;
            overview.wins += row.wins;
            overview.losses += row.losses.unwrap_or(0);
        }
        overview.archived_games += account.archived_games;
        overview.rows.push(row);
    }
    let ladder = |row: &Row| row.rank.as_ref().map(|(_, stats)| history::ladder(stats));
    overview.rows.sort_by(|a, b| {
        // Some(lp) больше None, поэтому «с изменениями» окажутся выше; внутри — по LP.
        b.lp.cmp(&a.lp).then_with(|| ladder(b).cmp(&ladder(a))).then_with(|| a.name.cmp(&b.name))
    });
    overview
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{DAY, RankPoint};
    use crate::riot::{Division, RankedQueues, Region, RiotId, Tier};
    use crate::storage::CachedProfile;

    fn gold(division: Division, lp: i32, wins: u32, losses: Option<u32>) -> RankedStats {
        RankedStats {
            tier: Tier::Gold,
            division,
            lp,
            wins,
            losses,
        }
    }

    fn account(id: AccountId, name: &str, history: Vec<RankPoint>) -> Account {
        // Текущий ранг — последняя точка истории Solo.
        let solo = history.iter().rev().find(|p| p.queue == Queue::Solo).map(|p| p.stats.clone());
        Account {
            id,
            riot_id: RiotId::parse(name).unwrap(),
            region: Region::Ru,
            puuid: None,
            lcu_puuid: None,
            note: String::new(),
            tags: vec![],
            secret_ref: String::new(),
            has_login: false,
            has_password: false,
            created_at: 0,
            cache: Some(CachedProfile {
                profile_icon_id: 1,
                summoner_level: 30,
                ranked: RankedQueues { solo, flex: None },
                matches: None,
                fetched_at: 0,
            }),
            history,
            archived_games: 10,
            ranked_games: vec![],
        }
    }

    fn point(taken_at: i64, stats: RankedStats) -> RankPoint {
        RankPoint {
            queue: Queue::Solo,
            taken_at,
            stats,
        }
    }

    #[test]
    fn sums_changes_and_sorts_active_accounts_first() {
        let now = 100 * DAY;
        let since = now - 7 * DAY;
        let accounts = [
            // Поднялся: Золото II 40 → Золото I 10 = +70, 4 победы и 1 поражение.
            account(
                1,
                "Up#RU1",
                vec![
                    point(now - 10 * DAY, gold(Division::Two, 40, 10, Some(10))),
                    point(now - DAY, gold(Division::One, 10, 14, Some(11))),
                ],
            ),
            // Не играл за период (последнее изменение — 20 дней назад).
            account(2, "Idle#RU1", vec![point(now - 20 * DAY, gold(Division::One, 90, 5, Some(5)))]),
            // Упал на 30 LP; поражения скрыты.
            account(
                3,
                "Down#RU1",
                vec![
                    point(now - 8 * DAY, gold(Division::Three, 60, 3, None)),
                    point(now - 2 * DAY, gold(Division::Three, 30, 4, None)),
                ],
            ),
        ];
        let overview = build(&accounts, since, now);
        let names: Vec<&str> = overview.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Up#RU1", "Down#RU1", "Idle#RU1"]);
        assert_eq!((overview.lp, overview.wins, overview.losses, overview.active), (40, 5, 1, 2));
        assert_eq!(overview.archived_games, 30);
        assert_eq!((overview.rows[0].lp, overview.rows[0].losses), (Some(70), Some(1)));
        assert_eq!((overview.rows[1].lp, overview.rows[1].losses), (Some(-30), None));
        assert_eq!(overview.rows[2].lp, None);
        // Лучший ранг — у того, кто не играл: Золото I 90.
        assert_eq!(overview.best().map(|r| r.name.as_str()), Some("Idle#RU1"));
        assert_eq!(overview.decaying(), 0);
    }
}
