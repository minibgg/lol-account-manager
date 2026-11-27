//! Статистика аккаунта по архиву игр — для окна «Профиль»: общий итог, чемпионы,
//! серия побед или поражений.
//!
//! Чистые функции над списком игр (без базы и окна) — их легко проверить тестами.

use std::collections::HashMap;

use crate::riot::{MatchSummary, RiotId};
use crate::storage::ArchivedPlayer;

/// Сумма показателей по нескольким играм. Ремейки считаются отдельно и в остальные
/// числа не входят: это не победа, не поражение и не настоящая игра.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Totals {
    /// Игры без ремейков.
    pub games: u32,
    pub wins: u32,
    pub remakes: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    /// Общая длительность игр, секунды.
    pub secs: u64,
    /// Игры, у которых есть полные показатели (CS, урон, золото): у старых записей
    /// архива их может не быть, и в средние по этим показателям они не входят.
    pub full_games: u32,
    pub full_secs: u64,
    pub cs: u64,
    pub damage: u64,
    pub gold: u64,
    pub vision: u64,
}

impl Totals {
    pub fn add(&mut self, game: &MatchSummary) {
        if game.remake {
            self.remakes += 1;
            return;
        }
        let me = &game.me;
        self.games += 1;
        self.wins += u32::from(game.win);
        self.kills += me.kills;
        self.deaths += me.deaths;
        self.assists += me.assists;
        self.secs += u64::from(game.duration_secs);
        if game.has_full_stats() {
            self.full_games += 1;
            self.full_secs += u64::from(game.duration_secs);
            self.cs += u64::from(me.cs);
            self.damage += u64::from(me.damage);
            self.gold += u64::from(me.gold);
            self.vision += u64::from(me.vision);
        }
    }

    pub fn of<'a>(games: impl IntoIterator<Item = &'a MatchSummary>) -> Self {
        let mut totals = Self::default();
        for game in games {
            totals.add(game);
        }
        totals
    }

    pub fn losses(&self) -> u32 {
        self.games - self.wins
    }

    /// Процент побед; `None` — игр нет.
    pub fn winrate(&self) -> Option<f32> {
        (self.games > 0).then(|| self.wins as f32 * 100.0 / self.games as f32)
    }

    /// KDA-коэффициент по всем играм сразу: (убийства + помощь) / смерти.
    pub fn kda_ratio(&self) -> f32 {
        (self.kills + self.assists) as f32 / self.deaths.max(1) as f32
    }

    /// Средние за игру «7.1/4.2/7.3».
    pub fn average_kda(&self) -> String {
        let n = self.games.max(1) as f32;
        format!(
            "{:.1}/{:.1}/{:.1}",
            self.kills as f32 / n,
            self.deaths as f32 / n,
            self.assists as f32 / n
        )
    }

    /// Средняя длительность игры, секунды.
    pub fn average_secs(&self) -> u32 {
        (self.secs / u64::from(self.games.max(1))) as u32
    }

    /// Значение в минуту по играм с полными показателями; `None` — таких игр нет.
    fn per_minute(&self, total: u64) -> Option<f32> {
        (self.full_secs > 0).then(|| total as f32 * 60.0 / self.full_secs as f32)
    }

    pub fn cs_per_minute(&self) -> Option<f32> {
        self.per_minute(self.cs)
    }

    pub fn damage_per_minute(&self) -> Option<f32> {
        self.per_minute(self.damage)
    }

    pub fn gold_per_minute(&self) -> Option<f32> {
        self.per_minute(self.gold)
    }

    /// Очки обзора за игру.
    pub fn vision_per_game(&self) -> Option<f32> {
        (self.full_games > 0).then(|| self.vision as f32 / self.full_games as f32)
    }
}

/// Итог по одному чемпиону.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChampionStats {
    pub champion_id: i32,
    /// Русское название; пустое — неизвестно (в окне покажем номер).
    pub name: String,
    pub totals: Totals,
    /// Когда на нём играли в последний раз (unix-время).
    pub last_played: i64,
}

/// Как упорядочить чемпионов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChampionSort {
    Games,
    Winrate,
    Kda,
    Recent,
}

impl ChampionSort {
    pub const ALL: [Self; 4] = [Self::Games, Self::Winrate, Self::Kda, Self::Recent];

    pub fn label(self) -> &'static str {
        match self {
            Self::Games => "игры",
            Self::Winrate => "винрейт",
            Self::Kda => "kda",
            Self::Recent => "недавние",
        }
    }
}

/// Итоги по чемпионам. Чемпионы, на которых были только ремейки, не попадают.
pub fn champions<'a>(games: impl IntoIterator<Item = &'a MatchSummary>, sort: ChampionSort) -> Vec<ChampionStats> {
    let mut by_champion: HashMap<i32, ChampionStats> = HashMap::new();
    for game in games {
        let entry = by_champion.entry(game.me.champion_id).or_insert_with(|| ChampionStats {
            champion_id: game.me.champion_id,
            name: String::new(),
            totals: Totals::default(),
            last_played: 0,
        });
        // Название могло появиться не у всех записей — берём первое непустое.
        if entry.name.is_empty() {
            entry.name.clone_from(&game.me.champion);
        }
        entry.totals.add(game);
        entry.last_played = entry.last_played.max(game.played_at);
    }
    let mut list: Vec<ChampionStats> = by_champion.into_values().filter(|c| c.totals.games > 0).collect();
    // total_cmp — сравнение дробных чисел, которое умеет и NaN (обычное `cmp` у f32 нет).
    list.sort_by(|a, b| {
        let main = match sort {
            ChampionSort::Games => b.totals.games.cmp(&a.totals.games),
            ChampionSort::Winrate => b.totals.winrate().unwrap_or(0.0).total_cmp(&a.totals.winrate().unwrap_or(0.0)),
            ChampionSort::Kda => b.totals.kda_ratio().total_cmp(&a.totals.kda_ratio()),
            ChampionSort::Recent => b.last_played.cmp(&a.last_played),
        };
        // При равенстве — по числу игр и номеру чемпиона, чтобы порядок не прыгал.
        main.then_with(|| b.totals.games.cmp(&a.totals.games))
            .then_with(|| a.champion_id.cmp(&b.champion_id))
    });
    list
}

/// С кем аккаунт сыграл вместе хотя бы столько игр — тот попадает в «напарники».
/// Случайный союзник почти никогда не встречается дважды, а друг — постоянно.
pub const PARTNER_MIN_GAMES: u32 = 2;

/// Итог игр вместе с одним напарником: показатели — самого аккаунта в этих играх.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartnerStats {
    pub riot_id: RiotId,
    pub totals: Totals,
    /// Когда играли вместе в последний раз (unix-время).
    pub last_played: i64,
}

/// Из игроков архива (см. `Db::archived_players`) — союзники с известным именем:
/// номер игры → их Riot ID. Это вход для [`partners`].
pub fn teammates(players: &HashMap<String, Vec<ArchivedPlayer>>) -> HashMap<String, Vec<RiotId>> {
    players
        .iter()
        .map(|(match_id, list)| {
            let allies = list.iter().filter(|p| p.ally).filter_map(|p| p.riot_id.clone()).collect();
            (match_id.clone(), allies)
        })
        .collect()
}

/// Напарники: союзники, с которыми аккаунт сыграл вместе не меньше [`PARTNER_MIN_GAMES`]
/// игр (без ремейков). `teammates` — номер игры → союзники в ней (см.
/// `Db::archived_teammates`); игры без подробностей просто не учитываются. Riot ID
/// сравниваются без учёта регистра. Сортировка — как у чемпионов.
pub fn partners<'a>(
    games: impl IntoIterator<Item = &'a MatchSummary>,
    teammates: &HashMap<String, Vec<RiotId>>,
    sort: ChampionSort,
) -> Vec<PartnerStats> {
    let mut by_player: HashMap<String, PartnerStats> = HashMap::new();
    for game in games {
        let Some(players) = teammates.get(&game.id) else {
            continue;
        };
        for riot_id in players {
            let entry = by_player.entry(riot_id.to_string().to_lowercase()).or_insert_with(|| PartnerStats {
                riot_id: riot_id.clone(),
                totals: Totals::default(),
                last_played: 0,
            });
            entry.totals.add(game);
            // Игры идут новыми первыми — имя берём из самой свежей (его могли сменить).
            if game.played_at > entry.last_played {
                entry.last_played = game.played_at;
                entry.riot_id.clone_from(riot_id);
            }
        }
    }
    let mut list: Vec<PartnerStats> = by_player
        .into_values()
        .filter(|p| p.totals.games >= PARTNER_MIN_GAMES)
        .collect();
    list.sort_by(|a, b| {
        let main = match sort {
            ChampionSort::Games => b.totals.games.cmp(&a.totals.games),
            ChampionSort::Winrate => b.totals.winrate().unwrap_or(0.0).total_cmp(&a.totals.winrate().unwrap_or(0.0)),
            ChampionSort::Kda => b.totals.kda_ratio().total_cmp(&a.totals.kda_ratio()),
            ChampionSort::Recent => b.last_played.cmp(&a.last_played),
        };
        main.then_with(|| b.totals.games.cmp(&a.totals.games))
            .then_with(|| a.riot_id.to_string().to_lowercase().cmp(&b.riot_id.to_string().to_lowercase()))
    });
    list
}

/// Итоги по позициям («top» … «utility»), чаще сыгранные — первыми. Игры, где позиция
/// неизвестна (ARAM, старые записи архива), не учитываются.
pub fn positions<'a>(games: impl IntoIterator<Item = &'a MatchSummary>) -> Vec<(String, Totals)> {
    let mut by_position: HashMap<&str, Totals> = HashMap::new();
    for game in games {
        if !game.me.position.is_empty() {
            by_position.entry(game.me.position.as_str()).or_default().add(game);
        }
    }
    let mut list: Vec<(String, Totals)> = by_position
        .into_iter()
        .filter(|(_, totals)| totals.games > 0)
        .map(|(position, totals)| (position.to_string(), totals))
        .collect();
    list.sort_by(|a, b| b.1.games.cmp(&a.1.games).then_with(|| a.0.cmp(&b.0)));
    list
}

/// Текущая серия: (победы это или поражения, сколько подряд). `games` — новые первыми;
/// ремейки серию не прерывают. `None` — игр нет.
pub fn streak<'a>(games: impl IntoIterator<Item = &'a MatchSummary>) -> Option<(bool, usize)> {
    let mut played = games.into_iter().filter(|g| !g.remake);
    let win = played.next()?.win;
    Some((win, 1 + played.take_while(|g| g.win == win).count()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::PlayerStats;
    use crate::riot::models::MATCH_STATS_VERSION;

    fn game(champion: i32, win: bool, kda: (u32, u32, u32), played_at: i64) -> MatchSummary {
        MatchSummary {
            id: format!("RU_{played_at}"),
            win,
            remake: false,
            queue_id: 420,
            played_at,
            duration_secs: 1800,
            stats_version: MATCH_STATS_VERSION,
            me: PlayerStats {
                champion_id: champion,
                champion: format!("Чемпион {champion}"),
                kills: kda.0,
                deaths: kda.1,
                assists: kda.2,
                cs: 180,
                damage: 21_000,
                ..Default::default()
            },
        }
    }

    #[test]
    fn totals_skip_remakes_and_old_records() {
        let mut remake = game(1, false, (0, 0, 0), 3);
        remake.remake = true;
        // Старая запись архива: только K/D/A, без CS и урона.
        let mut old = game(1, false, (2, 6, 4), 2);
        old.stats_version = 0;
        old.me.cs = 0;
        old.me.damage = 0;
        let games = [game(1, true, (10, 2, 8), 4), remake, old];
        let totals = Totals::of(&games);
        assert_eq!((totals.games, totals.wins, totals.losses(), totals.remakes), (2, 1, 1, 1));
        assert_eq!(totals.winrate(), Some(50.0));
        assert_eq!(totals.average_kda(), "6.0/4.0/6.0");
        assert_eq!(totals.kda_ratio(), 3.0);
        assert_eq!(totals.average_secs(), 1800);
        // CS в минуту — только по игре с полными показателями: 180 за 30 минут.
        assert_eq!(totals.cs_per_minute(), Some(6.0));
        assert_eq!(totals.damage_per_minute(), Some(700.0));
        assert_eq!(Totals::default().winrate(), None);
        assert_eq!(Totals::default().cs_per_minute(), None);
    }

    #[test]
    fn champions_are_grouped_and_sorted() {
        let games = [
            game(245, true, (5, 5, 5), 10),
            game(245, false, (5, 5, 5), 9),
            game(245, false, (5, 5, 5), 8),
            game(92, true, (9, 1, 1), 20),
            game(517, true, (1, 1, 1), 5),
            game(517, true, (1, 1, 1), 4),
        ];
        let ids = |sort| champions(&games, sort).iter().map(|c| c.champion_id).collect::<Vec<_>>();
        assert_eq!(ids(ChampionSort::Games), [245, 517, 92]);
        // Винрейт: 517 и 92 — по 100 %, выше тот, у кого больше игр.
        assert_eq!(ids(ChampionSort::Winrate), [517, 92, 245]);
        assert_eq!(ids(ChampionSort::Kda), [92, 245, 517]);
        assert_eq!(ids(ChampionSort::Recent), [92, 245, 517]);
        let ekko = &champions(&games, ChampionSort::Games)[0];
        assert_eq!((ekko.totals.games, ekko.totals.wins, ekko.last_played), (3, 1, 10));
        assert_eq!(ekko.name, "Чемпион 245");
    }

    #[test]
    fn partners_need_two_games_together() {
        let id = |s: &str| RiotId::parse(s).unwrap();
        let mut remake = game(1, true, (0, 0, 0), 5);
        remake.remake = true;
        let games = [
            game(1, true, (10, 2, 5), 40),
            game(1, true, (4, 4, 4), 30),
            game(1, false, (1, 5, 1), 20),
            game(1, true, (1, 1, 1), 10),
            remake,
            game(1, false, (1, 1, 1), 1), // без подробностей
        ];
        let teammates: HashMap<String, Vec<RiotId>> = [
            ("RU_40", vec![id("Pal#RU1"), id("Random#RU1")]),
            ("RU_30", vec![id("pal#ru1"), id("Duo#RU1")]),
            ("RU_20", vec![id("Duo#RU1")]),
            ("RU_10", vec![id("Duo#RU1")]),
            ("RU_5", vec![id("Random#RU1")]), // ремейк — не в счёт
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        let list = partners(&games, &teammates, ChampionSort::Games);
        let short: Vec<(String, u32, u32, i64)> = list
            .iter()
            .map(|p| (p.riot_id.to_string(), p.totals.games, p.totals.wins, p.last_played))
            .collect();
        // Random — одна настоящая игра: не напарник. Регистр Pal не важен, имя — из свежей игры.
        assert_eq!(short, [("Duo#RU1".into(), 3, 2, 30), ("Pal#RU1".into(), 2, 2, 40)]);
        let by_winrate: Vec<String> = partners(&games, &teammates, ChampionSort::Winrate)
            .iter()
            .map(|p| p.riot_id.game_name.clone())
            .collect();
        assert_eq!(by_winrate, ["Pal", "Duo"]);
        assert_eq!(list[1].totals.average_kda(), "7.0/3.0/4.5");
        assert!(partners(&games, &HashMap::new(), ChampionSort::Games).is_empty());
    }

    #[test]
    fn teammates_are_named_allies() {
        let player = |name: Option<&str>, ally: bool| ArchivedPlayer {
            riot_id: name.map(|n| RiotId::parse(n).unwrap()),
            champion: "Ари".into(),
            ally,
        };
        let players: HashMap<String, Vec<ArchivedPlayer>> =
            [("RU_1".to_string(), vec![player(Some("Pal#RU1"), true), player(None, true), player(Some("Foe#RU1"), false)])]
                .into_iter()
                .collect();
        assert_eq!(teammates(&players)["RU_1"], [RiotId::parse("Pal#RU1").unwrap()]);
    }

    #[test]
    fn positions_count_only_known_ones() {
        let at = |position: &str, win: bool, played_at: i64| {
            let mut game = game(1, win, (1, 1, 1), played_at);
            game.me.position = position.into();
            game
        };
        let games = [at("middle", true, 1), at("middle", false, 2), at("top", true, 3), at("", true, 4), at("middle", true, 5)];
        let list = positions(&games);
        let short: Vec<(&str, u32, u32)> = list.iter().map(|(p, t)| (p.as_str(), t.games, t.wins)).collect();
        assert_eq!(short, [("middle", 3, 2), ("top", 1, 1)]);
    }

    #[test]
    fn streak_counts_from_the_newest_game() {
        let mut remake = game(1, false, (0, 0, 0), 9);
        remake.remake = true;
        let games = [
            game(1, true, (1, 1, 1), 10),
            remake,
            game(1, true, (1, 1, 1), 8),
            game(1, false, (1, 1, 1), 7),
            game(1, true, (1, 1, 1), 6),
        ];
        assert_eq!(streak(&games), Some((true, 2)), "ремейк серию не прерывает");
        assert_eq!(streak(&games[3..]), Some((false, 1)));
        assert_eq!(streak(&[] as &[MatchSummary]), None);
    }
}
