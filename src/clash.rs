//! Clash на вкладке «Лобби»: ваша команда и соперник в текущем раунде турнира.
//! Пока турнир идёт (см. [`ClashState::is_shown`]), вкладка показывает его вместо лобби.
//!
//! Откуда данные (клиент League, только GET, проверено на живом клиенте):
//! - `/lol-clash/v1/tournament-summary` — турнир, ваша команда (rosterId) и сетка (bracketId);
//! - `/lol-clash/v1/bracket/{id}` — все матчи сетки: по ним находим текущий раунд и соперника;
//! - `/lol-clash/v1/roster/{id}` — состав команды (своей и соперника): summonerId и позиции;
//! - `/lol-summoner/v1/summoners/{summonerId}` — Riot ID, `/lol-ranked/...` — ранги;
//! - `/lol-clash/v1/scouting/champions` — та же «Разведка», что в клиенте.
//!
//! Соперник известен только когда сетка сформирована (после закрытия регистрации), и только
//! на ближайший раунд: следующий зависит от результатов текущего.
//!
//! Здесь только типы и чистые функции; опрос клиента — в backend.rs.

use std::collections::HashMap;

use crate::error::AppError;
use crate::lcu::models::{LcuClashBracket, LcuClashTournament, LcuScoutingChampions};
use crate::lobby::{PlayerProfile, Position};
use crate::riot::RiotId;

/// Этап турнира (поле `state` из `/lol-clash/v1/tournament-summary`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClashPhase {
    /// Клиент League не запущен.
    #[default]
    NoClient,
    /// Вы не зарегистрированы ни в одном турнире.
    NotRegistered,
    /// Регистрация открыта, турнир ещё не начался.
    Upcoming,
    /// Подтверждение участия перед турниром.
    LockIn,
    /// Сетка сформирована, можно смотреть соперника.
    Scouting,
    /// Идёт игра раунда.
    InGame,
    /// Турнир закончился.
    Results,
    Idle,
}

impl ClashPhase {
    pub fn from_lcu(state: &str) -> Self {
        match state {
            "UPCOMING" => Self::Upcoming,
            "LOCK_IN" => Self::LockIn,
            "SCOUTING" => Self::Scouting,
            "IN_GAME" => Self::InGame,
            "RESULTS" => Self::Results,
            _ => Self::Idle,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NoClient => "клиент не запущен",
            Self::NotRegistered => "нет регистрации",
            Self::Upcoming => "регистрация",
            Self::LockIn => "подтверждение",
            Self::Scouting => "разведка",
            Self::InGame => "идёт игра",
            Self::Results => "итоги",
            Self::Idle => "ожидание",
        }
    }

    /// Есть что показать (вкладка помечается точкой).
    pub fn is_active(self) -> bool {
        matches!(self, Self::LockIn | Self::Scouting | Self::InGame)
    }
}

/// Чемпион из разведки: «ривен 5 игр · 60% · kda 3.4».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoutChampion {
    pub name: String,
    /// 0 — игр в этом сезоне нет, чемпион взят из мастерства.
    pub games: u32,
    pub wins: u32,
    pub kda: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClashPlayer {
    pub summoner_id: u64,
    pub puuid: String,
    pub riot_id: Option<RiotId>,
    /// `None` — «любая» (FILL) или не выбрана.
    pub position: Option<Position>,
    pub is_self: bool,
    pub is_captain: bool,
    /// `None` — ещё не загружен или не удалось.
    pub profile: Option<PlayerProfile>,
    pub champions: Vec<ScoutChampion>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClashTeam {
    pub roster_id: i64,
    pub name: String,
    pub short_name: String,
    pub players: Vec<ClashPlayer>,
}

/// Что показывает вкладка «Клеш».
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClashState {
    pub phase: ClashPhase,
    /// Ближайший (или идущий) раунд; `None` — сетки нет или вы уже сыграли все матчи.
    pub round: Option<i32>,
    /// Когда начнётся раунд (unix-время, секунды).
    pub round_start: Option<i64>,
    /// Победы и поражения в этой сетке.
    pub wins: u32,
    pub losses: u32,
    /// Размер сетки (8 или 16 команд); 0 — сетки нет.
    pub bracket_size: i32,
    pub team: Option<ClashTeam>,
    pub opponent: Option<ClashTeam>,
    /// Соперник ещё не определился: ждём результата другого матча.
    pub opponent_pending: bool,
    pub problem: Option<AppError>,
    /// Ближайшие дни турниров (показываются на вкладке «Лобби», пока турнира нет).
    pub schedule: Vec<ClashDay>,
}

impl ClashState {
    /// Показывать ли турнир на вкладке «Лобби» вместо обычного лобби: клиент ответил,
    /// что вы в команде и турнир идёт (подтверждение, разведка или игра раунда).
    pub fn is_shown(&self) -> bool {
        self.team.is_some() && self.phase.is_active()
    }

    /// К какой команде относятся `c` / `o` (ники, op.gg): к сопернику, если он
    /// известен, иначе к своей.
    pub fn target_team(&self) -> Option<&ClashTeam> {
        self.opponent.as_ref().or(self.team.as_ref())
    }
}

/// Ближайший матч вашей команды в сетке.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NextMatch {
    pub round: Option<i32>,
    /// `None` — соперник ещё не определился.
    pub opponent: Option<i64>,
    /// Секунды unix.
    pub start: Option<i64>,
    pub wins: u32,
    pub losses: u32,
}

/// По сетке: счёт вашей команды и ближайший несыгранный матч.
///
/// В будущих матчах вместо id команд стоят отрицательные числа (ссылка на матч, из которого
/// придёт команда) или 0 — такие «соперники» ещё не известны.
pub fn next_match(bracket: &LcuClashBracket, my_roster: i64) -> NextMatch {
    let mut result = NextMatch::default();
    let mine = bracket
        .matches
        .iter()
        .filter(|m| m.roster_id1 == my_roster || m.roster_id2 == my_roster);
    let mut upcoming = None;
    for m in mine {
        if m.status == "COMPLETED" {
            if m.winner_id == my_roster {
                result.wins += 1;
            } else {
                result.losses += 1;
            }
        } else if upcoming.is_none_or(|u: &crate::lcu::models::LcuBracketMatch| m.round < u.round) {
            upcoming = Some(m);
        }
    }
    if let Some(m) = upcoming {
        let other = if m.roster_id1 == my_roster { m.roster_id2 } else { m.roster_id1 };
        result.round = Some(m.round);
        result.opponent = (other > 0).then_some(other);
        result.start = (m.round_start_time > 0).then_some(m.round_start_time / 1000);
    }
    result
}

/// Позиция в Clash: "TOP" … "UTILITY"; "FILL" и "UNSELECTED" — `None`.
pub fn position(lcu: &str) -> Option<Position> {
    Position::from_lcu(&lcu.to_lowercase())
}

/// Порядок строк: топ, лес, мид, бот, поддержка, потом «любая».
pub fn position_order(position: Option<Position>) -> u8 {
    match position {
        Some(Position::Top) => 0,
        Some(Position::Jungle) => 1,
        Some(Position::Middle) => 2,
        Some(Position::Bottom) => 3,
        Some(Position::Utility) => 4,
        None => 5,
    }
}

// ----- Расписание турниров ------------------------------------------------------

/// День турнира Clash из расписания (`/lol-clash/v1/all-tournaments`).
///
/// Проверено на живом клиенте (RU, 2026-11-22): каждый день Clash — отдельный «турнир»
/// с одной фазой; номер дня — во втором ключе названия («day_2»). У фазы
/// `registrationTime` — начало подтверждения участия (lock-in, за 2 часа до игр),
/// `startTime` — начало игр.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClashDay {
    pub tournament_id: i64,
    /// Название из ключа перевода («worlds 2024»); пустое — понять не удалось.
    pub name: String,
    /// Какой это день турнира (0 — неизвестно) и сколько их всего (0 — неизвестно).
    pub day: usize,
    pub days: usize,
    /// Когда начинается подтверждение участия и когда — игры (unix-время, секунды).
    pub lock_in: i64,
    pub start: i64,
    /// Вы (аккаунт из клиента) зарегистрированы в этом турнире.
    pub registered: bool,
}

impl ClashDay {
    /// Идёт подтверждение участия (до начала игр).
    pub fn lock_in_open(&self, now: i64) -> bool {
        self.lock_in > 0 && self.lock_in <= now && now < self.start
    }

    /// «день 2» / «день 1 из 2»; пусто — номер неизвестен.
    pub fn day_label(&self) -> String {
        match (self.day, self.days) {
            (0, _) => String::new(),
            (day, 0 | 1) => format!("день {day}"),
            (day, days) => format!("день {day} из {days}"),
        }
    }
}

/// Сколько дней турнира показывать.
pub const SCHEDULE_DAYS: usize = 4;
/// Сколько после начала день турнира ещё считается идущим (игры идут несколько часов).
const DAY_LENGTH: i64 = 6 * 3600;

/// Ближайшие дни турниров: без отменённых и прошедших, по порядку.
/// `registered` — турниры, где у вас есть команда (из `tournament-summary`).
pub fn schedule(tournaments: &[LcuClashTournament], registered: &[i64], now: i64) -> Vec<ClashDay> {
    let mut days = Vec::new();
    for t in tournaments.iter().filter(|t| t.status != "CANCELLED") {
        let mut phases: Vec<_> = t.phases.iter().filter(|p| !p.cancelled && p.start_time > 0).collect();
        phases.sort_by_key(|p| p.start_time);
        let name = tournament_name(&t.name_loc_key);
        // «day_2» → второй день; если ключа нет, а фаз несколько — по порядку фаз.
        let day_from_key = t
            .name_loc_key_secondary
            .to_lowercase()
            .strip_prefix("day_")
            .and_then(|n| n.parse::<usize>().ok());
        let total = if day_from_key.is_some() { 0 } else { phases.len() };
        let starts: Vec<(i64, i64)> = if phases.is_empty() {
            // Дней нет в ответе — берём время всего турнира.
            (t.schedule_time > 0)
                .then_some((t.schedule_time / 1000, 0))
                .into_iter()
                .collect()
        } else {
            phases.iter().map(|p| (p.start_time / 1000, p.registration_time / 1000)).collect()
        };
        for (index, (start, lock_in)) in starts.into_iter().enumerate() {
            if start + DAY_LENGTH < now {
                continue;
            }
            let day = match day_from_key {
                Some(day) => day + index,
                None if total > 1 => index + 1,
                None => 0,
            };
            days.push(ClashDay {
                tournament_id: t.id,
                name: name.clone(),
                day,
                days: total,
                lock_in,
                start,
                registered: registered.contains(&t.id),
            });
        }
    }
    days.sort_by_key(|d| d.start);
    days.truncate(SCHEDULE_DAYS);
    days
}

/// «tournament_name_bilgewater» → «bilgewater», «worlds2024» → «worlds 2024»;
/// непонятный ключ — пустая строка.
fn tournament_name(key: &str) -> String {
    let key = key.to_lowercase();
    let mut rest = key.as_str();
    for prefix in ["clash_", "tournament_", "name_", "theme_"] {
        rest = rest.strip_prefix(prefix).unwrap_or(rest);
    }
    // Пробел между буквами и цифрами.
    let mut spaced = String::new();
    let mut previous: Option<char> = None;
    for c in rest.chars() {
        if previous.is_some_and(|p| p.is_ascii_alphabetic()) && c.is_ascii_digit() {
            spaced.push(' ');
        }
        spaced.push(c);
        previous = Some(c);
    }
    let name = spaced.replace('_', " ").trim().to_string();
    if name.is_empty() || name.chars().all(|c| c.is_ascii_digit() || c == ' ') {
        String::new()
    } else {
        name
    }
}

// ----- Кого банить ----------------------------------------------------------------

/// Подсказка бана против соперника: чемпион, кто на нём играет и насколько успешно.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BanSuggestion {
    pub champion: String,
    /// «Имя» игрока соперника (без тега); пустое — имя не загрузилось.
    pub player: String,
    pub games: u32,
    pub wins: u32,
}

impl BanSuggestion {
    /// «ривен — ник: 9 игр, 67%».
    pub fn label(&self) -> String {
        let who = if self.player.is_empty() { String::new() } else { format!(" — {}", self.player) };
        if self.games == 0 {
            format!("{}{who}: мастерство", self.champion.to_lowercase())
        } else {
            format!(
                "{}{who}: {}, {}%",
                self.champion.to_lowercase(),
                crate::lobby::games_count(self.games),
                self.wins * 100 / self.games
            )
        }
    }
}

/// Сколько банов предлагать (в Clash у команды 5 банов).
pub const BAN_SUGGESTIONS: usize = 5;

/// Кого банить против команды: чемпионы сезона из разведки, у кого больше игр и выше
/// процент побед — тот и опаснее. Один чемпион — один раз (у самого опасного игрока).
/// Если ни у кого нет игр в сезоне — по мастерству (оно стоит в разведке первым).
pub fn ban_suggestions(team: &ClashTeam) -> Vec<BanSuggestion> {
    let mut all: Vec<(u32, BanSuggestion)> = Vec::new();
    for player in &team.players {
        let name = player.riot_id.as_ref().map(|id| id.game_name.clone()).unwrap_or_default();
        for (rank, champion) in player.champions.iter().enumerate() {
            // Очки: игры × (1 + доля побед). Мастерство без игр — совсем немного,
            // чтобы попасть в список, только если больше ничего нет.
            let score = if champion.games > 0 {
                champion.games * 100 + champion.wins * 100
            } else {
                3 - rank.min(2) as u32
            };
            all.push((
                score,
                BanSuggestion {
                    champion: champion.name.clone(),
                    player: name.clone(),
                    games: champion.games,
                    wins: champion.wins,
                },
            ));
        }
    }
    all.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let has_games = all.iter().any(|(_, s)| s.games > 0);
    let mut seen = std::collections::HashSet::new();
    all.into_iter()
        .map(|(_, s)| s)
        // Есть игры в сезоне — одиночные «по мастерству» уже не нужны.
        .filter(|s| !has_games || s.games >= 2)
        .filter(|s| seen.insert(s.champion.clone()))
        .take(BAN_SUGGESTIONS)
        .collect()
}

/// До трёх чемпионов из разведки: сначала сыгранные в этом сезоне (по числу игр),
/// а если таких нет — с наибольшим мастерством.
pub fn scout_champions(scouting: &LcuScoutingChampions, names: &HashMap<i32, String>) -> Vec<ScoutChampion> {
    let name = |id: i32| names.get(&id).cloned().unwrap_or_else(|| format!("#{id}"));
    let mut season: Vec<_> = scouting
        .top_season_champions
        .iter()
        .filter(|c| c.game_count > 0 && c.champion_id > 0)
        .collect();
    // sort_by_key + Reverse — по убыванию игр.
    season.sort_by_key(|c| std::cmp::Reverse(c.game_count));
    let picked: Vec<ScoutChampion> = season
        .into_iter()
        .take(3)
        .map(|c| ScoutChampion {
            name: name(c.champion_id),
            games: c.game_count,
            wins: c.win_count,
            kda: c.kda.clone(),
        })
        .collect();
    if !picked.is_empty() {
        return picked;
    }
    let mut masteries: Vec<_> = scouting.top_masteries.iter().filter(|m| m.champion_id > 0).collect();
    masteries.sort_by_key(|m| std::cmp::Reverse(m.champion_points));
    masteries
        .into_iter()
        .take(3)
        .map(|m| ScoutChampion {
            name: name(m.champion_id),
            games: 0,
            wins: 0,
            kda: String::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcu::models::{LcuBracketMatch, LcuScoutingMastery, LcuScoutingSeasonChampion};

    fn game(round: i32, a: i64, b: i64, status: &str, winner: i64) -> LcuBracketMatch {
        LcuBracketMatch {
            round,
            roster_id1: a,
            roster_id2: b,
            status: status.into(),
            winner_id: winner,
            round_start_time: 1_795_282_629_620,
            ..Default::default()
        }
    }

    #[test]
    fn finds_current_opponent_and_record() {
        // Как в живой сетке: 1-й раунд выиграли, во 2-м известен соперник,
        // в 3-м вместо команд — ссылки на матчи (отрицательные id).
        let bracket = LcuClashBracket {
            matches: vec![
                game(3, -1048373, -1048375, "UPCOMING", 0),
                game(2, 2141461, 2139072, "UPCOMING", 0),
                game(1, 2139645, 2139072, "COMPLETED", 2139072),
                game(1, 1, 2, "COMPLETED", 1),
            ],
            ..Default::default()
        };
        let next = next_match(&bracket, 2139072);
        assert_eq!(next.round, Some(2));
        assert_eq!(next.opponent, Some(2141461));
        assert_eq!(next.start, Some(1_795_282_629));
        assert_eq!((next.wins, next.losses), (1, 0));
    }

    #[test]
    fn opponent_unknown_until_other_match_ends() {
        let bracket = LcuClashBracket {
            matches: vec![
                game(1, 5, 7, "COMPLETED", 7),
                game(2, 7, -42, "UPCOMING", 0),
                game(3, 0, 7, "UPCOMING", 0),
            ],
            ..Default::default()
        };
        let next = next_match(&bracket, 7);
        assert_eq!(next.round, Some(2));
        assert_eq!(next.opponent, None);
    }

    #[test]
    fn no_upcoming_match_after_last_game() {
        let bracket = LcuClashBracket {
            matches: vec![game(1, 5, 7, "COMPLETED", 5), game(2, 7, 9, "COMPLETED", 9)],
            ..Default::default()
        };
        let next = next_match(&bracket, 7);
        assert_eq!(next.round, None);
        assert_eq!((next.wins, next.losses), (0, 2));
    }

    #[test]
    fn positions() {
        assert_eq!(position("UTILITY"), Some(Position::Utility));
        assert_eq!(position("FILL"), None);
        assert_eq!(position("UNSELECTED"), None);
        assert!(position_order(Some(Position::Top)) < position_order(None));
    }

    #[test]
    fn schedule_skips_cancelled_and_past_days() {
        use crate::lcu::models::LcuClashTournamentPhase;
        let now = 1_800_000_000;
        // Как в живом клиенте: отдельный турнир на день, номер — в «day_2».
        let live = [LcuClashTournament {
            id: 28542,
            name_loc_key: "worlds2024".into(),
            name_loc_key_secondary: "day_2".into(),
            status: "DEFAULT".into(),
            phases: vec![LcuClashTournamentPhase {
                id: 27422,
                registration_time: (now + 3600) * 1000,
                start_time: (now + 3 * 3600) * 1000,
                cancelled: false,
            }],
            ..Default::default()
        }];
        let today = schedule(&live, &[28542], now);
        assert_eq!(today.len(), 1);
        assert_eq!((today[0].name.as_str(), today[0].day_label()), ("worlds 2024", "день 2".to_string()));
        assert!(today[0].registered);
        assert!(!today[0].lock_in_open(now), "подтверждение через час");
        assert!(today[0].lock_in_open(now + 2 * 3600));

        let phase = |id, start: i64, cancelled| LcuClashTournamentPhase {
            id,
            registration_time: (start - 4 * 86_400) * 1000,
            start_time: start * 1000,
            cancelled,
        };
        let tournaments = [
            LcuClashTournament {
                id: 7,
                name_loc_key: "tournament_name_bilgewater".into(),
                status: "DEFAULT".into(),
                phases: vec![
                    phase(2, now + 86_400, false),
                    phase(1, now - 2 * 86_400, false), // уже прошёл
                    phase(3, now + 2 * 86_400, true),  // отменён
                ],
                ..Default::default()
            },
            LcuClashTournament {
                id: 8,
                status: "CANCELLED".into(),
                phases: vec![phase(9, now + 3600, false)],
                ..Default::default()
            },
            LcuClashTournament {
                id: 9,
                name_loc_key: "12345".into(),
                status: "DEFAULT".into(),
                schedule_time: (now + 7 * 86_400) * 1000,
                ..Default::default()
            },
        ];
        let days = schedule(&tournaments, &[7], now);
        let seen: Vec<_> = days.iter().map(|d| (d.tournament_id, d.name.as_str(), d.day_label(), d.registered)).collect();
        assert_eq!(
            seen,
            [(7, "bilgewater", "день 2 из 2".to_string(), true), (9, "", String::new(), false)]
        );
        assert!(days[0].lock_in_open(now), "подтверждение за 4 дня до начала");
        assert!(!days[1].lock_in_open(now), "без времени подтверждения — не знаем");
        // Идущий сейчас день (начался час назад) ещё показываем.
        let today = [LcuClashTournament {
            id: 1,
            phases: vec![phase(1, now - 3600, false)],
            ..Default::default()
        }];
        assert_eq!(schedule(&today, &[], now).len(), 1);
    }

    #[test]
    fn bans_pick_the_most_dangerous_champions() {
        let player = |name: &str, champions: Vec<ScoutChampion>| ClashPlayer {
            riot_id: RiotId::parse(&format!("{name}#RU1")).ok(),
            champions,
            ..ClashPlayer::default()
        };
        let champ = |name: &str, games, wins| ScoutChampion {
            name: name.into(),
            games,
            wins,
            kda: String::new(),
        };
        let team = ClashTeam {
            players: vec![
                player("top", vec![champ("Ривен", 9, 6), champ("Джакс", 1, 1)]),
                player("mid", vec![champ("Ари", 12, 5), champ("Ривен", 3, 3)]),
                player("sup", vec![champ("Тареш", 0, 0)]), // только мастерство
            ],
            ..ClashTeam::default()
        };
        let bans = ban_suggestions(&team);
        let seen: Vec<_> = bans.iter().map(|b| (b.champion.as_str(), b.player.as_str(), b.games)).collect();
        // Ари: 12 игр + 5 побед = 1700; Ривен у top: 9 + 6 = 1500; Ривен у mid повторно не берём;
        // Джакс с одной игрой и Тареш без игр — не нужны, когда есть сыгранные чемпионы.
        assert_eq!(seen, [("Ари", "mid", 12), ("Ривен", "top", 9)]);
        assert_eq!(bans[1].label(), "ривен — top: 9 игр, 66%");

        // Ни у кого нет игр в сезоне — подсказываем по мастерству.
        let quiet = ClashTeam {
            players: vec![player("a", vec![champ("Тареш", 0, 0), champ("Лулу", 0, 0)])],
            ..ClashTeam::default()
        };
        let bans = ban_suggestions(&quiet);
        assert_eq!(bans.len(), 2);
        assert_eq!(bans[0].label(), "тареш — a: мастерство");
    }

    #[test]
    fn scouting_prefers_season_games_then_mastery() {
        let names = HashMap::from([(38, "Кассадин".to_string()), (105, "Физз".to_string())]);
        let season = |id, games| LcuScoutingSeasonChampion {
            champion_id: id,
            game_count: games,
            win_count: games / 2,
            kda: "3.0".into(),
        };
        let mastery = |id, points| LcuScoutingMastery {
            champion_id: id,
            champion_points: points,
        };
        let scouting = LcuScoutingChampions {
            player_id: 1,
            top_masteries: vec![mastery(131, 274_893), mastery(38, 304_534)],
            top_season_champions: vec![season(26, 0), season(105, 4), season(38, 9)],
        };
        let picked = scout_champions(&scouting, &names);
        let list: Vec<_> = picked.iter().map(|c| (c.name.as_str(), c.games)).collect();
        assert_eq!(list, [("Кассадин", 9), ("Физз", 4)], "без игр — не показываем");

        let no_games = LcuScoutingChampions {
            top_season_champions: vec![season(26, 0)],
            ..scouting
        };
        let picked = scout_champions(&no_games, &names);
        let list: Vec<_> = picked.iter().map(|c| (c.name.as_str(), c.games)).collect();
        assert_eq!(list, [("Кассадин", 0), ("#131", 0)], "по мастерству");
    }
}
