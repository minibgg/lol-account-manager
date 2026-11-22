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
use crate::lcu::models::{
    LcuClashBracket, LcuClashRosterStats, LcuClashTournament, LcuRecommendedPositions, LcuScoutingChampions,
};
use crate::lobby::{PlayerProfile, Position};
use crate::riot::models::{MatchSummary, PlayerStats};
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
    pub champion_id: i32,
    pub name: String,
    /// 0 — игр в этом сезоне нет, чемпион взят из мастерства.
    pub games: u32,
    pub wins: u32,
    pub kda: String,
    /// На каких линиях чемпиона играют (порядок не важен); пусто — клиент не сказал.
    pub positions: Vec<Position>,
    /// Основная линия чемпиона; `None` — не узнали.
    pub main_position: Option<Position>,
}

impl ScoutChampion {
    /// Чемпион подходит на эту линию.
    pub fn fits(&self, role: Position) -> bool {
        self.main_position == Some(role) || self.positions.contains(&role)
    }
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
    /// KDA из разведки («3.4»); пустая — нет игр в сезоне.
    pub kda: String,
}

impl BanSuggestion {
    /// «ривен — ник: 9 игр, 67%».
    pub fn label(&self) -> String {
        let who = if self.player.is_empty() { String::new() } else { format!(" — {}", self.player) };
        format!("{}{who}: {}", self.champion.to_lowercase(), self.stats())
    }

    /// «9 игр, 67%»; без игр в сезоне — «мастерство».
    pub fn stats(&self) -> String {
        if self.games == 0 {
            "мастерство".into()
        } else {
            format!("{}, {}%", crate::lobby::games_count(self.games), self.wins * 100 / self.games)
        }
    }
}

/// Сколько банов предлагать (в Clash у команды 5 банов).
pub const BAN_SUGGESTIONS: usize = 5;
/// Сколько запасных вариантов показывать после основных (если основной уже забанили
/// или его взяли себе).
pub const BAN_RESERVE: usize = 3;
/// Больше трёх банов на одного игрока не предлагаем: остальные игроки тоже опасны.
const BANS_PER_PLAYER: usize = 3;
/// Сколько чемпионов сезона брать из разведки на игрока: в таблице видны первые три,
/// остальные нужны для запасных банов.
const SCOUT_CHAMPIONS: usize = 5;

/// Кого банить против команды: сначала [`BAN_SUGGESTIONS`] основных, затем до
/// [`BAN_RESERVE`] запасных. Опаснее тот чемпион, на котором у игрока больше игр
/// в этом сезоне и выше процент побед (по разведке). Один чемпион — один раз (у самого
/// опасного игрока), на одного игрока — не больше трёх. Если ни у кого нет игр в
/// сезоне — по мастерству (оно стоит в разведке первым).
pub fn ban_suggestions(team: &ClashTeam) -> Vec<BanSuggestion> {
    // (очки, номер игрока, подсказка)
    let mut all: Vec<(u32, usize, BanSuggestion)> = Vec::new();
    for (index, player) in team.players.iter().enumerate() {
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
                index,
                BanSuggestion {
                    champion: champion.name.clone(),
                    player: name.clone(),
                    games: champion.games,
                    wins: champion.wins,
                    kda: champion.kda.clone(),
                },
            ));
        }
    }
    all.sort_by_key(|(score, ..)| std::cmp::Reverse(*score));
    let has_games = all.iter().any(|(.., s)| s.games > 0);
    let mut seen = std::collections::HashSet::new();
    let mut per_player: HashMap<usize, usize> = HashMap::new();
    let mut picked = Vec::new();
    for (_, player, suggestion) in all {
        // Есть игры в сезоне — одиночные «по мастерству» уже не нужны.
        if has_games && suggestion.games < 2 {
            continue;
        }
        let count = per_player.entry(player).or_default();
        if *count >= BANS_PER_PLAYER || !seen.insert(suggestion.champion.clone()) {
            continue;
        }
        *count += 1;
        picked.push(suggestion);
        if picked.len() == BAN_SUGGESTIONS + BAN_RESERVE {
            break;
        }
    }
    picked
}

// ----- Прошлые Clash игрока -------------------------------------------------------

/// Очереди Clash: 5×5 на Ущелье (700) и ARAM Clash (720). Проверено 2026-11-22: игры
/// этого турнира в истории клиента записаны с очередью 700.
pub const CLASH_QUEUES: [i32; 2] = [700, 720];
/// Сколько последних игр Clash загружать через Riot API (по запросу на игру).
pub const HISTORY_GAMES: usize = 10;
/// Сколько игр показывать в окне (из всех источников вместе).
pub const HISTORY_SHOWN: usize = 15;
/// Игры ближе трёх часов друг к другу — один день турнира (раунды идут подряд;
/// день может перейти через полночь).
const DAY_GAP: i64 = 3 * 3600;

/// Чьи прошлые Clash открыть (щелчок по нику на вкладке Clash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRequest {
    pub riot_id: RiotId,
    /// PUUID из клиента League (не Riot API); пустой — неизвестен.
    pub puuid: String,
    /// Команда игрока в нынешнем турнире и он в ней — для сегодняшних раундов.
    pub roster_id: i64,
    pub summoner_id: u64,
}

/// Прошлые игры игрока в Clash (окно по щелчку на ник). Источники дополняют друг друга
/// (проверено 2026-11-22): в Riot API есть старые игры Clash, но нет свежих (игр за
/// ноябрь 2026 там не нашлось вовсе); в истории клиента League свежие есть, но у чужих
/// игроков она неполная; сегодняшние раунды — из итогов команды в турнире (без K/D/A).
#[derive(Debug, Clone, PartialEq)]
pub struct ClashHistory {
    /// Новые первыми, не больше [`HISTORY_SHOWN`].
    pub games: Vec<MatchSummary>,
    /// Ответил ли Riot API (ключ есть и запрос удался).
    pub from_api: bool,
    /// Почему не получилось через Riot API, хотя ключ есть.
    pub api_problem: Option<AppError>,
}

impl ClashHistory {
    /// Игра взята из итогов команды: известны только чемпион и результат.
    pub fn is_brief(game: &MatchSummary) -> bool {
        game.stats_version == 0
    }
}

/// Сегодняшние раунды игрока из итогов его команды: чемпион, результат, длительность.
/// K/D/A и роли там нет — у таких записей `stats_version` 0 (см. [`ClashHistory::is_brief`]).
pub fn roster_games(stats: &LcuClashRosterStats, summoner_id: u64) -> Vec<MatchSummary> {
    let key = summoner_id.to_string();
    let mut games = Vec::new();
    for period in &stats.period_stats {
        for round in period.match_stats.iter().filter(|m| m.game_id > 0) {
            let Some(&champion_id) = round.player_champion_ids.get(&key) else {
                continue; // игрок в этом раунде не играл (замена)
            };
            games.push(MatchSummary {
                id: format!("CLASH_{}", round.game_id),
                win: round.win,
                remake: false,
                queue_id: 700,
                // Точного времени игры нет — начало дня плюс номер раунда (для порядка).
                played_at: period.time / 1000 + i64::from(round.round) * 60,
                duration_secs: u32::try_from(round.duration_ms / 1000).unwrap_or(0),
                stats_version: 0,
                me: PlayerStats {
                    champion_id,
                    ..PlayerStats::default()
                },
            });
        }
    }
    games
}

/// Склеивает игры из нескольких источников: одна игра — один раз (по номеру игры;
/// берётся из первого списка, где она есть, — передавайте самые подробные первыми),
/// новые первыми, не больше `limit`.
pub fn merge_history(sources: Vec<Vec<MatchSummary>>, limit: usize) -> Vec<MatchSummary> {
    let mut seen = std::collections::HashSet::new();
    let mut games: Vec<MatchSummary> = sources
        .into_iter()
        .flatten()
        .filter(|g| seen.insert(MatchSummary::game_id(&g.id).map_or_else(|| g.id.clone(), |id| id.to_string())))
        .collect();
    games.sort_by_key(|g| std::cmp::Reverse(g.played_at));
    games.truncate(limit);
    games
}

/// Один день турнира: игры подряд, новые первыми.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryDay<'a> {
    pub games: Vec<&'a MatchSummary>,
    pub wins: usize,
    pub losses: usize,
}

impl HistoryDay<'_> {
    /// Начало первой игры дня (unix-время, секунды).
    pub fn start(&self) -> i64 {
        self.games.last().map_or(0, |g| g.played_at)
    }
}

/// Игры Clash по дням турнира, новые дни первыми. Ремейки не считаются ни победой,
/// ни поражением.
pub fn history_days(games: &[MatchSummary]) -> Vec<HistoryDay<'_>> {
    let mut sorted: Vec<&MatchSummary> = games.iter().collect();
    sorted.sort_by_key(|g| std::cmp::Reverse(g.played_at));
    let mut days: Vec<HistoryDay> = Vec::new();
    for game in sorted {
        let same_day = days
            .last()
            .and_then(|d| d.games.last())
            .is_some_and(|previous| previous.played_at - game.played_at <= DAY_GAP);
        if !same_day {
            days.push(HistoryDay {
                games: Vec::new(),
                wins: 0,
                losses: 0,
            });
        }
        if let Some(day) = days.last_mut() {
            if !game.remake {
                if game.win {
                    day.wins += 1;
                } else {
                    day.losses += 1;
                }
            }
            day.games.push(game);
        }
    }
    days
}

/// На ком игрок играл в Clash: чемпион, игры, победы — чаще первыми.
pub fn history_champions(games: &[MatchSummary]) -> Vec<(String, u32, u32)> {
    let mut list: Vec<(i32, String, u32, u32)> = Vec::new();
    for game in games.iter().filter(|g| !g.remake) {
        let name = if game.me.champion.is_empty() {
            format!("#{}", game.me.champion_id)
        } else {
            game.me.champion.clone()
        };
        match list.iter_mut().find(|(id, ..)| *id == game.me.champion_id) {
            Some(entry) => {
                entry.2 += 1;
                entry.3 += u32::from(game.win);
            }
            None => list.push((game.me.champion_id, name, 1, u32::from(game.win))),
        }
    }
    // Сортировка устойчивая: при равном числе игр — кто встретился раньше (игры идут
    // новыми первыми, значит — кто сыгран позже).
    list.sort_by_key(|(_, _, games, _)| std::cmp::Reverse(*games));
    list.into_iter().map(|(_, name, games, wins)| (name, games, wins)).collect()
}

// ----- Баны по ролям ----------------------------------------------------------------

/// Роли по порядку строк: топ, лес, мид, бот, поддержка (индекс = [`position_order`]).
pub const ROLES: [Position; 5] = [
    Position::Top,
    Position::Jungle,
    Position::Middle,
    Position::Bottom,
    Position::Utility,
];

/// Сколько банов предлагать на одну роль.
pub const BANS_PER_ROLE: usize = 3;

/// Баны против одной роли соперника.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleBans {
    pub role: Position,
    /// Кто, скорее всего, на этой роли («имя» без тега; пустое — имя не загрузилось).
    /// `None` — игрока на эту роль не нашлось.
    pub player: Option<String>,
    /// Роль выбрана самим игроком в Clash, а не угадана по чемпионам.
    pub known: bool,
    /// Угадана, но чемпионы игрока на эту роль почти не похожи.
    pub unsure: bool,
    pub bans: Vec<BanSuggestion>,
}

/// Ответ клиента «id строкой → линии» → id → позиции.
pub fn champion_positions(raw: HashMap<String, LcuRecommendedPositions>) -> HashMap<i32, Vec<Position>> {
    raw.into_iter()
        .filter_map(|(id, p)| {
            let positions = p.recommended_positions.iter().filter_map(|s| position(s)).collect();
            Some((id.parse().ok()?, positions))
        })
        .collect()
}

/// Вес чемпиона игрока: игры в сезоне, а без игр (по мастерству) — 3, 2, 1 по порядку.
fn champion_weight(rank: usize, champion: &ScoutChampion) -> u32 {
    if champion.games > 0 { champion.games } else { 3 - rank.min(2) as u32 }
}

/// Насколько чемпионы игрока похожи на каждую роль (индексы как в [`ROLES`]):
/// основная линия чемпиона — тройной вес, остальные его линии — одинарный.
fn role_affinity(player: &ClashPlayer) -> [u32; 5] {
    let mut affinity = [0; 5];
    for (rank, champion) in player.champions.iter().enumerate() {
        let weight = champion_weight(rank, champion);
        for (index, &role) in ROLES.iter().enumerate() {
            if champion.main_position == Some(role) {
                affinity[index] += weight * 3;
            } else if champion.positions.contains(&role) {
                affinity[index] += weight;
            }
        }
    }
    affinity
}

/// Кто на какой роли: для каждой роли из [`ROLES`] — номер игрока в `team.players`.
/// Роли, выбранные в Clash, закреплены; остальных игроков расставляем так, чтобы
/// их чемпионы в сумме лучше всего подходили к ролям (перебор: игроков ≤ 5 — 120 вариантов).
fn assign_roles(team: &ClashTeam) -> [Option<usize>; 5] {
    let affinity: Vec<[u32; 5]> = team.players.iter().map(role_affinity).collect();
    let mut roles = [None; 5];
    let mut free = Vec::new();
    for (index, player) in team.players.iter().enumerate() {
        match player.position.map(|p| usize::from(position_order(Some(p)))) {
            Some(role) if roles[role].is_none() => roles[role] = Some(index),
            _ => free.push(index),
        }
    }
    let mut best = None;
    place(&free, &affinity, &mut roles, 0, &mut best);
    best.map(|(_, roles)| roles).unwrap_or(roles)
}

/// Перебор для [`assign_roles`]: ставим `free[0]` на каждую свободную роль и идём дальше.
fn place(
    free: &[usize],
    affinity: &[[u32; 5]],
    roles: &mut [Option<usize>; 5],
    score: u32,
    best: &mut Option<(u32, [Option<usize>; 5])>,
) {
    let Some((&player, rest)) = free.split_first() else {
        if best.is_none_or(|(top, _)| score > top) {
            *best = Some((score, *roles));
        }
        return;
    };
    for role in 0..ROLES.len() {
        if roles[role].is_none() {
            roles[role] = Some(player);
            place(rest, affinity, roles, score + affinity[player][role], best);
            roles[role] = None;
        }
    }
    // Игроков больше, чем свободных ролей (запасной в составе) — кто-то остаётся без роли.
    if free.len() > roles.iter().filter(|r| r.is_none()).count() {
        place(rest, affinity, roles, score, best);
    }
}

/// Баны по ролям соперника: на каждую роль — до [`BANS_PER_ROLE`] чемпионов игрока,
/// который, скорее всего, её играет. У соперника в Clash позиции обычно «любая», поэтому
/// роль угадывается по его чемпионам (см. [`assign_roles`]). Сначала идут чемпионы,
/// которые подходят на эту роль, внутри — у кого больше игр и побед.
pub fn role_bans(team: &ClashTeam) -> Vec<RoleBans> {
    let assigned = assign_roles(team);
    ROLES
        .iter()
        .zip(assigned)
        .map(|(&role, index)| {
            let Some(player) = index.and_then(|i| team.players.get(i)) else {
                return RoleBans {
                    role,
                    player: None,
                    known: false,
                    unsure: false,
                    bans: Vec::new(),
                };
            };
            let affinity = role_affinity(player);
            let total: u32 = affinity.iter().sum();
            let here = affinity[usize::from(position_order(Some(role)))];
            let mut champions: Vec<(usize, &ScoutChampion)> = player.champions.iter().enumerate().collect();
            champions.sort_by_key(|&(rank, c)| {
                let danger = if c.games > 0 { c.games + c.wins } else { champion_weight(rank, c) };
                (!c.fits(role), std::cmp::Reverse(danger))
            });
            let name = player.riot_id.as_ref().map(|id| id.game_name.clone()).unwrap_or_default();
            RoleBans {
                role,
                known: player.position == Some(role),
                // Меньше трети «веса» чемпионов игрока подходит на эту роль.
                unsure: player.position != Some(role) && here * 3 < total.max(1),
                bans: champions
                    .into_iter()
                    .take(BANS_PER_ROLE)
                    .map(|(_, c)| BanSuggestion {
                        champion: c.name.clone(),
                        player: name.clone(),
                        games: c.games,
                        wins: c.wins,
                        kda: c.kda.clone(),
                    })
                    .collect(),
                player: Some(name),
            }
        })
        .collect()
}

/// Чемпионы из разведки: сначала сыгранные в этом сезоне (до пяти, по числу игр),
/// а если таких нет — до трёх с наибольшим мастерством. `positions` — линии чемпионов
/// из [`champion_positions`].
pub fn scout_champions(
    scouting: &LcuScoutingChampions,
    names: &HashMap<i32, String>,
    positions: &HashMap<i32, Vec<Position>>,
) -> Vec<ScoutChampion> {
    let name = |id: i32| names.get(&id).cloned().unwrap_or_else(|| format!("#{id}"));
    let lines = |id: i32| positions.get(&id).cloned().unwrap_or_default();
    let mut season: Vec<_> = scouting
        .top_season_champions
        .iter()
        .filter(|c| c.game_count > 0 && c.champion_id > 0)
        .collect();
    // sort_by_key + Reverse — по убыванию игр.
    season.sort_by_key(|c| std::cmp::Reverse(c.game_count));
    let picked: Vec<ScoutChampion> = season
        .into_iter()
        .take(SCOUT_CHAMPIONS)
        .map(|c| ScoutChampion {
            champion_id: c.champion_id,
            name: name(c.champion_id),
            games: c.game_count,
            wins: c.win_count,
            kda: c.kda.clone(),
            positions: lines(c.champion_id),
            main_position: None,
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
            champion_id: m.champion_id,
            name: name(m.champion_id),
            games: 0,
            wins: 0,
            kda: String::new(),
            positions: lines(m.champion_id),
            main_position: None,
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
            champion_id: 0,
            name: name.into(),
            games,
            wins,
            kda: String::new(),
            positions: Vec::new(),
            main_position: None,
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
    fn bans_spread_over_players_and_add_reserve() {
        let champ = |name: String, games| ScoutChampion {
            champion_id: 0,
            name,
            games,
            wins: games / 2,
            kda: "3.0".into(),
            positions: Vec::new(),
            main_position: None,
        };
        // Один «уанчемп» с пятью чемпионами на кучу игр и четыре игрока поскромнее.
        let mut players = vec![ClashPlayer {
            riot_id: RiotId::parse("main#RU1").ok(),
            champions: (0..5).map(|i| champ(format!("A{i}"), 50 - i)).collect(),
            ..ClashPlayer::default()
        }];
        for p in 0..4 {
            players.push(ClashPlayer {
                riot_id: RiotId::parse(&format!("p{p}#RU1")).ok(),
                champions: vec![champ(format!("B{p}"), 10 - p), champ(format!("C{p}"), 4)],
                ..ClashPlayer::default()
            });
        }
        let bans = ban_suggestions(&ClashTeam { players, ..ClashTeam::default() });
        assert_eq!(bans.len(), BAN_SUGGESTIONS + BAN_RESERVE);
        let from_main = bans.iter().filter(|b| b.player == "main").count();
        assert_eq!(from_main, 3, "не больше трёх банов на одного игрока");
        let first: Vec<_> = bans.iter().take(BAN_SUGGESTIONS).map(|b| b.champion.as_str()).collect();
        assert_eq!(first, ["A0", "A1", "A2", "B0", "B1"]);
        assert_eq!(bans[0].kda, "3.0");
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
        let positions = HashMap::from([(38, vec![Position::Middle])]);
        let picked = scout_champions(&scouting, &names, &positions);
        let list: Vec<_> = picked.iter().map(|c| (c.name.as_str(), c.games)).collect();
        assert_eq!(list, [("Кассадин", 9), ("Физз", 4)], "без игр — не показываем");
        assert_eq!(picked[0].positions, [Position::Middle]);
        assert!(picked[1].positions.is_empty(), "линии неизвестны");

        let no_games = LcuScoutingChampions {
            top_season_champions: vec![season(26, 0)],
            ..scouting
        };
        let picked = scout_champions(&no_games, &names, &positions);
        let list: Vec<_> = picked.iter().map(|c| (c.name.as_str(), c.games)).collect();
        assert_eq!(list, [("Кассадин", 0), ("#131", 0)], "по мастерству");
    }

    #[test]
    fn champion_positions_from_client() {
        let raw: HashMap<String, LcuRecommendedPositions> = serde_json::from_str(
            r#"{"1":{"recommendedPositions":["MIDDLE","UTILITY"]},"102":{"recommendedPositions":["JUNGLE"]},"x":{}}"#,
        )
        .unwrap();
        let map = champion_positions(raw);
        assert_eq!(map[&1], [Position::Middle, Position::Utility]);
        assert_eq!(map[&102], [Position::Jungle]);
        assert_eq!(map.len(), 2, "непонятный id пропускаем");
    }

    #[test]
    fn role_bans_guess_roles_from_champions() {
        use Position::*;
        // main — основная линия, lines — все линии в алфавитном порядке, как отдаёт клиент.
        let champ = |name: &str, games, main, lines: &[Position]| ScoutChampion {
            champion_id: 0,
            name: name.into(),
            games,
            wins: games / 2,
            kda: String::new(),
            positions: lines.to_vec(),
            main_position: Some(main),
        };
        let player = |name: &str, champions: Vec<ScoutChampion>| ClashPlayer {
            riot_id: RiotId::parse(&format!("{name}#RU1")).ok(),
            champions,
            ..ClashPlayer::default()
        };
        // Живой соперник 2026-11-22: позиции у всех «любая».
        let team = ClashTeam {
            players: vec![
                player("lle", vec![
                    champ("Jinx", 7, Bottom, &[Bottom]),
                    champ("Ezreal", 7, Bottom, &[Bottom]),
                    champ("Vayne", 6, Bottom, &[Bottom, Top]),
                    champ("Syndra", 4, Middle, &[Middle]),
                    champ("Rengar", 3, Jungle, &[Jungle]),
                ]),
                player("yim", vec![
                    champ("Yunara", 3, Bottom, &[Bottom]),
                    champ("Ashe", 1, Bottom, &[Bottom, Utility]),
                    champ("Twisted Fate", 1, Middle, &[Middle]),
                ]),
                // Darius у клиента «JUNGLE, TOP», Kayle «MIDDLE, TOP» — но основная у обоих топ.
                player("yan", vec![
                    champ("Darius", 35, Top, &[Jungle, Top]),
                    champ("Kayle", 19, Top, &[Middle, Top]),
                    champ("Smolder", 18, Bottom, &[Bottom, Middle]),
                    champ("Swain", 17, Utility, &[Bottom, Middle, Utility]),
                    champ("Mel", 10, Middle, &[Bottom, Middle, Utility]),
                ]),
                player("tuz", vec![
                    champ("Blitzcrank", 1, Utility, &[Utility]),
                    champ("Ahri", 1, Middle, &[Middle]),
                    champ("Garen", 1, Top, &[Top]),
                ]),
                player("ail", vec![champ("Nami", 27, Utility, &[Utility]), champ("Lulu", 3, Utility, &[Middle, Utility])]),
            ],
            ..ClashTeam::default()
        };
        let roles = role_bans(&team);
        let who = |r: &RoleBans| r.player.clone().unwrap_or_default();
        assert_eq!(who(&roles[0]), "yan", "Darius 35 игр — топ, хотя у него много мид-чемпионов");
        assert_eq!(who(&roles[3]), "lle");
        assert_eq!(who(&roles[4]), "ail");
        let rest = [who(&roles[1]), who(&roles[2])];
        assert!(rest.contains(&"yim".to_string()) && rest.contains(&"tuz".to_string()));
        assert!(roles[1].unsure, "лесных чемпионов ни у кого из оставшихся нет");
        // На топ — сначала чемпионы, которые туда подходят, потом остальные по опасности.
        let top: Vec<_> = roles[0].bans.iter().map(|b| b.champion.as_str()).collect();
        assert_eq!(top, ["Darius", "Kayle", "Smolder"]);
        assert!(!roles[0].unsure && !roles[0].known);
        assert_eq!(roles[3].bans.len(), BANS_PER_ROLE);

        // Позиция, выбранная в Clash, закреплена, даже если чемпионы говорят другое.
        let mut fixed = team.clone();
        fixed.players[1].position = Some(Bottom);
        let roles = role_bans(&fixed);
        assert_eq!(roles[3].player.as_deref(), Some("yim"));
        assert!(roles[3].known && !roles[3].unsure);
        assert_eq!(roles[0].player.as_deref(), Some("yan"));
    }

    #[test]
    fn history_groups_games_into_tournament_days() {
        let game = |hour: i64, champion: &str, id: i32, win: bool| MatchSummary {
            id: format!("RU_{hour}"),
            win,
            remake: false,
            queue_id: 700,
            played_at: 1_795_000_000 + hour * 3600,
            duration_secs: 1800,
            stats_version: 0,
            me: crate::riot::models::PlayerStats {
                champion_id: id,
                champion: champion.into(),
                ..Default::default()
            },
        };
        // Неделю назад — три игры подряд, позже — ещё две.
        let games = vec![
            game(0, "Darius", 122, true),
            game(1, "Darius", 122, false),
            game(3, "Kayle", 10, true),
            game(7 * 24, "Darius", 122, true),
            game(7 * 24 + 1, "Garen", 86, true),
        ];
        let days = history_days(&games);
        assert_eq!(days.len(), 2);
        assert_eq!((days[0].wins, days[0].losses, days[0].games.len()), (2, 0, 2));
        assert_eq!(days[0].games[0].me.champion, "Garen", "новые первыми");
        assert_eq!((days[1].wins, days[1].losses), (2, 1));
        assert_eq!(days[1].start(), 1_795_000_000);

        let champions = history_champions(&games);
        assert_eq!(champions[0], ("Darius".to_string(), 3, 2));
        assert_eq!(champions.len(), 3);
    }

    #[test]
    fn today_rounds_come_from_roster_stats_and_merge_without_duplicates() {
        // Как живой ответ /roster/{id}/stats: в раунде все десять игроков, 0 — игры не было.
        let stats: LcuClashRosterStats = serde_json::from_str(
            r#"{"periodStats":[{"time":1795366800000,"matchStats":[
                {"round":1,"durationMs":2528808,"win":true,"gameId":574310186,
                 "playerChampionIds":{"17212083":86,"487376":516}},
                {"round":2,"durationMs":0,"win":true,"gameId":0,"playerChampionIds":{}}
            ]}]}"#,
        )
        .unwrap();
        let today = roster_games(&stats, 17212083);
        assert_eq!(today.len(), 1, "раунд без игры пропускаем");
        assert_eq!((today[0].me.champion_id, today[0].win, today[0].duration_secs), (86, true, 2528));
        assert!(ClashHistory::is_brief(&today[0]));
        assert!(roster_games(&stats, 1).is_empty(), "чужого игрока там нет");

        // Та же игра пришла и из клиента (подробнее) — остаётся одна, подробная.
        let full = MatchSummary {
            id: "RU_574310186".into(),
            stats_version: crate::riot::models::MATCH_STATS_VERSION,
            me: PlayerStats {
                champion_id: 86,
                kills: 7,
                ..PlayerStats::default()
            },
            ..today[0].clone()
        };
        let old = MatchSummary {
            id: "RU_1".into(),
            played_at: 1_700_000_000,
            ..full.clone()
        };
        let merged = merge_history(vec![vec![old], vec![full], today], HISTORY_SHOWN);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].me.kills, 7, "подробная запись важнее короткой");
        assert_eq!(merged[1].id, "RU_1", "новые первыми");
    }
}
