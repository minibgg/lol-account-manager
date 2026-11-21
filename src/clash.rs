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
use crate::lcu::models::{LcuClashBracket, LcuScoutingChampions};
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
