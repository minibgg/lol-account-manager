//! Ответы LCU (только нужные нам поля; остальные serde пропускает).

use std::collections::HashMap;

use serde::Deserialize;

use crate::lobby;
use crate::riot::models::{FETCH_MATCHES, LeagueEntryDto, MATCH_STATS_VERSION};
use crate::riot::{MatchDetails, MatchSummary, PlayerLine, PlayerStats, RankedQueues, TeamDetails, Tier};

/// `/lol-summoner/v1/current-summoner`, `/lol-summoner/v1/summoners?name=...`,
/// `/lol-summoner/v2/summoners/puuid/{puuid}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LcuSummoner {
    /// Настоящий PUUID (UUID). У Riot API он другой — зашифрованный.
    pub puuid: String,
    #[serde(default)]
    pub game_name: String,
    #[serde(default)]
    pub tag_line: String,
    pub profile_icon_id: i32,
    pub summoner_level: i64,
    /// Номер призывателя на сервере (нужен для запросов коллекции).
    #[serde(default)]
    pub summoner_id: u64,
}

/// `/riotclient/region-locale`.
#[derive(Debug, Clone, Deserialize)]
pub struct LcuRegionLocale {
    pub region: String,
}

/// `/lol-champ-select/v1/session` — выбор чемпионов (404, если его сейчас нет).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuChampSelectSession {
    pub local_player_cell_id: i64,
    pub my_team: Vec<LcuChampSelectMember>,
}

/// Игрок своей команды в выборе чемпионов. В ранговых у чужих игроков
/// `puuid`, `game_name` и `tag_line` пустые — клиент их скрывает.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuChampSelectMember {
    pub cell_id: i64,
    /// "top", "jungle", "middle", "bottom", "utility" или "" (без позиций).
    pub assigned_position: String,
    pub champion_id: i32,
    /// Чемпион, которого игрок собирается взять (ещё не выбран окончательно).
    pub champion_pick_intent: i32,
    pub puuid: String,
    pub game_name: String,
    pub tag_line: String,
}

/// Чемпион из `/lol-champions/v1/owned-champions-minimal` (там же — бесплатные на этой неделе).
/// `/lol-champions/v1/owned-champions-minimal` — чемпионы аккаунта из клиента.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LcuOwnedChampion {
    pub id: i32,
    /// На языке клиента.
    pub name: String,
    pub ownership: LcuOwnership,
}

/// `/lol-champions/v1/inventories/{summonerId}/skins-minimal` — все образы игры
/// с пометкой, какие есть на аккаунте.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuSkinMinimal {
    pub id: i32,
    pub champion_id: i32,
    /// На языке клиента: «Звёздная защитница Ари».
    pub name: String,
    /// Базовый образ есть у всех, у кого есть чемпион, — его не считаем.
    pub is_base: bool,
    pub ownership: LcuOwnership,
}

/// `/lol-honor-v2/v1/profile`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuHonorProfile {
    pub honor_level: i32,
}

/// `/lol-player-behavior/v1/chat-restriction` (404 — ограничения нет).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuChatRestriction {
    pub games_remaining: i64,
    /// Миллисекунды unix; 0 — по времени не ограничено.
    pub expiration_millis: u64,
}

/// `/lol-leaver-buster/v1/ranked-restriction`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuRankedRestriction {
    pub punished_games_remaining: i32,
}

/// `/lol-leaver-buster/v1/notifications` — штрафы за выходы из игр.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuLeaverNotification {
    /// "PunishedGamesRemaining" (очередь с задержкой), "OnLockoutWarning" и др.
    #[serde(rename = "type")]
    pub kind: String,
    pub punished_games_remaining: i32,
}

/// `/lol-clash/v1/all-tournaments` — расписание турниров Clash.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuClashTournament {
    pub id: i64,
    /// Ключ перевода названия, например «tournament_name_bilgewater».
    pub name_loc_key: String,
    pub name_loc_key_secondary: String,
    /// Миллисекунды unix.
    pub schedule_time: i64,
    pub schedule_end_time: i64,
    /// До какого момента можно собрать команду (мс unix).
    pub roster_create_deadline: i64,
    /// "CANCELLED", "DEFAULT", "PAUSED", "PRERESUME".
    pub status: String,
    pub phases: Vec<LcuClashTournamentPhase>,
}

/// День турнира: когда открывается регистрация и когда начинаются игры.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuClashTournamentPhase {
    pub id: i64,
    /// Миллисекунды unix.
    pub registration_time: i64,
    pub start_time: i64,
    pub cancelled: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LcuOwnership {
    pub owned: bool,
}

/// Предмет из `/lol-inventory/v2/inventory/CHAMPION_SKIN` — образ чемпиона.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LcuInventoryItem {
    pub owned: bool,
}

/// `/lol-gameflow/v1/session` — текущая игра: номер, очередь, кто на ком и на какой позиции.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuGameflowSession {
    pub game_data: LcuGameData,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuGameData {
    pub game_id: i64,
    pub queue: LcuGameQueue,
    pub player_champion_selections: Vec<LcuChampionSelection>,
    pub team_one: Vec<LcuGameMember>,
    pub team_two: Vec<LcuGameMember>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LcuGameQueue {
    pub id: i32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuChampionSelection {
    pub champion_id: i32,
    pub puuid: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuGameMember {
    pub puuid: String,
    /// «TOP», «JUNGLE», «MIDDLE», «BOTTOM», «UTILITY» или «NONE».
    pub selected_position: String,
}

/// `/lol-lobby/v2/lobby` — группа до начала поиска игры.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LcuLobby {
    pub members: Vec<LcuLobbyMember>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LcuLobbyMember {
    pub puuid: String,
}

/// `/lol-game-data/assets/v1/champion-summary.json` — id и название чемпиона.
#[derive(Debug, Deserialize)]
pub struct LcuChampionSummary {
    pub id: i32,
    pub name: String,
}

/// Riot Client `/chat/v5/participants` — участники всех чатов, где вы сейчас есть.
/// Поля в snake_case (так отвечает Riot Client).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ChatParticipants {
    pub participants: Vec<ChatParticipant>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ChatParticipant {
    /// Идентификатор комнаты чата; у выбора чемпионов в нём есть «champ-select».
    pub cid: String,
    pub game_name: String,
    pub game_tag: String,
    pub puuid: String,
}

/// `/lol-match-history/v1/products/lol/{puuid}/matches?begIndex=0&endIndex=19` —
/// последние игры игрока, новые первыми (`endIndex` включительно: 0..=19 — двадцать игр).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LcuMatchHistory {
    pub games: LcuGameList,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LcuGameList {
    pub games: Vec<LcuGame>,
}

/// Одна игра. В списке последних игр в `participants` — только сам игрок,
/// а `/lol-match-history/v1/games/{gameId}` отдаёт всех десятерых.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuGame {
    pub game_id: i64,
    /// «EUW1», «RU» — как в id игр Riot API («EUW1_7012345678»).
    pub platform_id: String,
    /// Начало игры, unix-время в миллисекундах.
    pub game_creation: i64,
    /// Длительность в секундах.
    pub game_duration: i64,
    pub queue_id: i32,
    pub participants: Vec<LcuParticipant>,
    /// Имена игроков: связаны с `participants` через `participant_id`.
    pub participant_identities: Vec<LcuParticipantIdentity>,
    pub teams: Vec<LcuTeam>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuParticipant {
    pub participant_id: i32,
    /// 100 — синяя команда, 200 — красная.
    pub team_id: i32,
    pub champion_id: i32,
    pub stats: LcuParticipantStats,
    pub timeline: LcuTimeline,
}

/// Линия и роль игрока, как их определил сервер по ходу игры.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuTimeline {
    /// «TOP», «JUNGLE», «MIDDLE», «BOTTOM» или «NONE».
    pub lane: String,
    /// «SOLO», «CARRY», «SUPPORT», «DUO_CARRY», «DUO_SUPPORT», «NONE»…
    pub role: String,
}

impl LcuTimeline {
    /// Позиция в нашем виде («top» … «utility»); пусто — клиент её не знает.
    /// Клиент часто отдаёт «NONE» вместо линии (и тогда роль у всех одинаковая,
    /// ей верить нельзя), поэтому позицию берём, только когда линия названа.
    pub fn position(&self) -> String {
        match (self.lane.as_str(), self.role.as_str()) {
            ("TOP", _) => "top",
            ("JUNGLE", _) => "jungle",
            ("MIDDLE" | "MID", _) => "middle",
            ("BOTTOM", "SUPPORT" | "DUO_SUPPORT") => "utility",
            ("BOTTOM", "CARRY" | "DUO_CARRY") => "bottom",
            _ => "",
        }
        .to_string()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuParticipantStats {
    pub champ_level: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub total_minions_killed: u32,
    pub neutral_minions_killed: u32,
    pub gold_earned: u32,
    pub total_damage_dealt_to_champions: u32,
    pub vision_score: u32,
    pub largest_multi_kill: u32,
    pub item0: i32,
    pub item1: i32,
    pub item2: i32,
    pub item3: i32,
    pub item4: i32,
    pub item5: i32,
    pub item6: i32,
    pub win: bool,
    /// Ремейк: игру закончили в первые минуты.
    pub game_ended_in_early_surrender: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuParticipantIdentity {
    pub participant_id: i32,
    pub player: LcuPlayer,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuPlayer {
    pub puuid: String,
    pub game_name: String,
    pub tag_line: String,
}

/// Итоги команды. Поле `win` клиент отдаёт строкой («Win» / «Fail») — его не разбираем,
/// победу берём из показателей игроков.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuTeam {
    pub team_id: i32,
    pub tower_kills: u32,
    pub dragon_kills: u32,
    pub baron_kills: u32,
}

impl LcuParticipant {
    fn to_stats(&self) -> PlayerStats {
        let s = &self.stats;
        PlayerStats {
            champion_id: self.champion_id,
            champion: String::new(),
            champ_level: s.champ_level,
            kills: s.kills,
            deaths: s.deaths,
            assists: s.assists,
            cs: s.total_minions_killed + s.neutral_minions_killed,
            gold: s.gold_earned,
            damage: s.total_damage_dealt_to_champions,
            vision: s.vision_score,
            items: vec![s.item0, s.item1, s.item2, s.item3, s.item4, s.item5, s.item6],
            largest_multi_kill: s.largest_multi_kill,
            position: self.timeline.position(),
        }
    }
}

impl LcuGame {
    fn match_id(&self) -> String {
        format!("{}_{}", self.platform_id, self.game_id)
    }

    fn duration_secs(&self) -> u32 {
        self.game_duration.max(0) as u32
    }

    /// Обе команды целиком. `me_puuid` — PUUID клиента League (чью строку выделить).
    pub fn details(&self, me_puuid: Option<&str>) -> MatchDetails {
        let player = |p: &LcuParticipant| {
            let identity = self
                .participant_identities
                .iter()
                .find(|i| i.participant_id == p.participant_id)
                .map(|i| &i.player);
            PlayerLine {
                riot_id: identity.and_then(|i| lobby::riot_id(&i.game_name, &i.tag_line)),
                is_me: identity.is_some_and(|i| me_puuid == Some(i.puuid.as_str())),
                stats: p.to_stats(),
            }
        };
        let mut teams: Vec<TeamDetails> = [100, 200]
            .into_iter()
            .map(|team_id| {
                let members: Vec<&LcuParticipant> = self.participants.iter().filter(|p| p.team_id == team_id).collect();
                let team = self.teams.iter().find(|t| t.team_id == team_id);
                TeamDetails {
                    win: members.iter().any(|p| p.stats.win),
                    towers: team.map_or(0, |t| t.tower_kills),
                    dragons: team.map_or(0, |t| t.dragon_kills),
                    barons: team.map_or(0, |t| t.baron_kills),
                    players: members.into_iter().map(player).collect(),
                }
            })
            .filter(|t| !t.players.is_empty())
            .collect();
        // Режимы, где команды не 100/200 (Арена), — всех одним списком.
        if teams.is_empty() && !self.participants.is_empty() {
            teams.push(TeamDetails {
                win: false,
                towers: 0,
                dragons: 0,
                barons: 0,
                players: self.participants.iter().map(player).collect(),
            });
        }
        let mut details = MatchDetails {
            id: self.match_id(),
            queue_id: self.queue_id,
            played_at: self.game_creation / 1000,
            duration_secs: self.duration_secs(),
            remake: self.participants.iter().any(|p| p.stats.game_ended_in_early_surrender),
            teams,
        };
        details.put_my_team_first();
        details
    }
}

impl LcuMatchHistory {
    /// В общий формат. Новые игры первыми, не больше `FETCH_MATCHES`.
    pub fn to_summaries(&self) -> Vec<MatchSummary> {
        let mut matches = self.all_summaries();
        matches.truncate(FETCH_MATCHES);
        matches
    }

    /// Все игры ответа, новые первыми.
    pub fn all_summaries(&self) -> Vec<MatchSummary> {
        let mut matches: Vec<MatchSummary> = self
            .games
            .games
            .iter()
            .filter_map(|g| {
                let me = g.participants.first()?;
                Some(MatchSummary {
                    id: g.match_id(),
                    win: me.stats.win,
                    remake: me.stats.game_ended_in_early_surrender,
                    queue_id: g.queue_id,
                    played_at: g.game_creation / 1000,
                    duration_secs: g.duration_secs(),
                    stats_version: MATCH_STATS_VERSION,
                    me: me.to_stats(),
                })
            })
            .collect();
        // Клиент и так отдаёт новые первыми, но порядок не обещан — сортируем сами.
        matches.sort_by_key(|m| std::cmp::Reverse(m.played_at));
        matches
    }
}

/// `/lol-ranked/v1/ranked-stats/{puuid}`: список по всем очередям (включая TFT).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LcuRankedStats {
    pub queues: Vec<LcuQueue>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuQueue {
    pub queue_type: String,
    /// "PLATINUM", "MASTER"... Пустая строка — нет ранга.
    pub tier: String,
    /// "I".."IV". "NA" — нет ранга или тир без дивизионов (Мастер и выше).
    pub division: String,
    pub league_points: i32,
    pub wins: u32,
    pub losses: u32,
}

impl LcuRankedStats {
    /// Переводим в формат Riot API и разбираем общим кодом (`RankedQueues::from_entries`).
    pub fn to_queues(&self) -> RankedQueues {
        let entries: Vec<LeagueEntryDto> = self
            .queues
            .iter()
            .filter_map(|q| {
                let tier = Tier::from_api(&q.tier)?; // нет ранга — пропускаем молча
                // У Мастера и выше в LCU дивизион "NA", а в Riot API — "I".
                let rank = if tier.is_apex() { "I" } else { q.division.as_str() };
                Some(LeagueEntryDto {
                    queue_type: q.queue_type.clone(),
                    tier: q.tier.clone(),
                    rank: rank.to_string(),
                    league_points: q.league_points,
                    wins: q.wins,
                    losses: q.losses,
                })
            })
            .collect();
        RankedQueues::from_entries(&entries)
    }
}

// ----- Clash ------------------------------------------------------------------
// Проверено на живом клиенте (RU, 2026-11-21) во время фазы «Разведка».

/// `/lol-clash/v1/tournament-summary` — турниры, в которых вы зарегистрированы.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuClashSummary {
    /// "UPCOMING", "LOCK_IN", "SCOUTING", "IN_GAME", "RESULTS", "IDLE".
    pub state: String,
    pub tournament_id: i64,
    /// Здесь строкой, в сетке — числом.
    pub roster_id: String,
    /// 0 — сетки ещё нет.
    pub bracket_id: i64,
}

/// `/lol-clash/v1/bracket/{bracketId}` — сетка турнира: все матчи и команды.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuClashBracket {
    pub id: i64,
    pub size: i32,
    pub matches: Vec<LcuBracketMatch>,
    pub rosters: Vec<LcuBracketRoster>,
    pub is_complete: bool,
}

/// Матч сетки. У будущих матчей вместо команд — отрицательные id матчей, из которых
/// придут победитель или проигравший (`-1048662`), или 0.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuBracketMatch {
    pub id: i64,
    pub round: i32,
    pub roster_id1: i64,
    pub roster_id2: i64,
    /// "UPCOMING", "COMPLETED", …
    pub status: String,
    /// 0 — ещё не сыгран.
    pub winner_id: i64,
    pub loser_bracket: bool,
    /// Когда начнётся раунд (мс unix); -1 — неизвестно.
    pub round_start_time: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuBracketRoster {
    pub roster_id: i64,
    pub name: String,
    pub short_name: String,
}

/// `/lol-clash/v1/roster/{rosterId}` — команда (своя или любая из вашей сетки).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuClashRoster {
    pub id: String,
    pub name: String,
    pub short_name: String,
    pub captain_summoner_id: u64,
    pub members: Vec<LcuClashRosterMember>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuClashRosterMember {
    pub summoner_id: u64,
    /// "TOP", "JUNGLE", "MIDDLE", "BOTTOM", "UTILITY", "FILL", "UNSELECTED".
    /// У соперников клиент показывает "FILL", пока те не выбрали позиции.
    pub position: String,
}

/// `/lol-summoner/v1/summoners/{summonerId}` — участники Clash-команд даны по summonerId.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuSummonerById {
    pub summoner_id: u64,
    pub puuid: String,
    pub game_name: String,
    pub tag_line: String,
    pub profile_icon_id: i32,
    pub summoner_level: i64,
}

/// `/lol-clash/v1/scouting/champions?puuids=[…]` — то же, что вкладка «Разведка» в клиенте.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuScoutingChampions {
    /// summonerId.
    pub player_id: u64,
    pub top_masteries: Vec<LcuScoutingMastery>,
    pub top_season_champions: Vec<LcuScoutingSeasonChampion>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuScoutingMastery {
    pub champion_id: i32,
    pub champion_points: i64,
}

/// Линии чемпиона из `/lol-perks/v1/recommended-champion-positions` (ответ — объект
/// «id чемпиона строкой → это»). ВНИМАНИЕ (проверено 2026-11-22): порядок алфавитный,
/// а не по популярности (у Darius «JUNGLE, TOP») — основную линию отсюда не узнать,
/// она в [`LcuRecommendedPage`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuRecommendedPositions {
    /// "TOP", "JUNGLE", "MIDDLE", "BOTTOM", "UTILITY".
    pub recommended_positions: Vec<String>,
}

/// Итоги команды Clash в её турнире (`/lol-clash/v1/roster/{rosterId}/stats`; работает и
/// для команды соперника — проверено 2026-11-22). Команда живёт один день турнира, так что
/// здесь — только сегодняшние раунды.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuClashRosterStats {
    pub period_stats: Vec<LcuClashPeriodStats>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuClashPeriodStats {
    /// Начало дня турнира, unix-время в миллисекундах.
    pub time: i64,
    pub match_stats: Vec<LcuClashMatchStats>,
}

/// Один раунд. `game_id` 0 — игры не было (соперник не явился).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuClashMatchStats {
    pub round: i32,
    pub duration_ms: i64,
    pub win: bool,
    pub game_id: i64,
    /// summonerId строкой → id чемпиона; здесь все десять игроков игры.
    pub player_champion_ids: HashMap<String, i32>,
}

/// Рекомендуемая страница рун из
/// `/lol-perks/v1/recommended-pages/champion/{id}/position/NONE/map/11`: с позицией NONE
/// клиент отдаёт страницы для основной линии чемпиона (`isDefaultPosition: true`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuRecommendedPage {
    pub position: String,
    pub is_default_position: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LcuScoutingSeasonChampion {
    pub champion_id: i32,
    pub game_count: u32,
    pub win_count: u32,
    /// Например "3.4".
    pub kda: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Форма ответа взята с настоящего клиента (значения изменены).
    const SAMPLE: &str = r#"{"queues":[
        {"queueType":"RANKED_SOLO_5x5","tier":"PLATINUM","division":"III","leaguePoints":42,
         "wins":20,"losses":18,"isProvisional":false},
        {"queueType":"RANKED_FLEX_SR","tier":"","division":"NA","leaguePoints":0,
         "wins":0,"losses":0,"isProvisional":true},
        {"queueType":"RANKED_TFT","tier":"GOLD","division":"I","leaguePoints":1,
         "wins":1,"losses":1}
    ], "queueMap": {}, "seasons": {}}"#;

    #[test]
    fn converts_lcu_ranked_stats() {
        let stats: LcuRankedStats = serde_json::from_str(SAMPLE).unwrap();
        let queues = stats.to_queues();
        assert_eq!(queues.solo.unwrap().to_string(), "Платина III · 42 LP · 20W / 18L · 52.6%");
        assert_eq!(queues.flex, None, "пустой тир = без ранга");
    }

    /// Форма ответа истории матчей — с настоящего клиента (значения изменены).
    #[test]
    fn converts_match_history() {
        let json = r#"{"accountId": 1, "platformId": "RU", "games": {
            "gameBeginDate": "", "gameCount": 20, "gameEndDate": "", "gameIndexBegin": 0, "gameIndexEnd": 4,
            "games": [
              {"gameId": 100, "platformId": "RU", "gameCreation": 1793933264853, "gameDuration": 67,
               "queueId": 420, "gameMode": "CLASSIC", "mapId": 11,
               "participants": [{"participantId": 1, "championId": 36, "teamId": 100,
                  "stats": {"kills": 0, "deaths": 0, "assists": 0, "win": true,
                            "gameEndedInEarlySurrender": true, "goldEarned": 500}}],
               "participantIdentities": [{"participantId": 1, "player": {"puuid": "p"}}]},
              {"gameId": 101, "platformId": "RU", "gameCreation": 1793934757581, "gameDuration": 1819,
               "queueId": 450,
               "participants": [{"participantId": 1, "championId": 42,
                  "stats": {"kills": 15, "deaths": 11, "assists": 9, "win": false}}]}
            ]}}"#;
        let history: LcuMatchHistory = serde_json::from_str(json).unwrap();
        let matches = history.to_summaries();
        assert_eq!(matches.len(), 2);
        // Новые первыми, даже если клиент прислал в другом порядке.
        assert_eq!(matches[0].id, "RU_101");
        assert_eq!((matches[0].me.champion_id, matches[0].win, matches[0].kda().as_str()), (42, false, "15/11/9"));
        assert_eq!(matches[0].queue_label(), "ARAM");
        assert!(matches[1].remake, "игра 67 секунд — ремейк");
        assert_eq!(matches[1].played_at, 1793933264);
    }

    /// Подробности одной игры: `/lol-match-history/v1/games/{gameId}` (форма как у клиента).
    #[test]
    fn converts_game_details() {
        let json = r#"{"gameId": 7, "platformId": "RU", "gameCreation": 1000000, "gameDuration": 1500, "queueId": 420,
            "participants": [
              {"participantId": 1, "teamId": 100, "championId": 42,
               "stats": {"kills": 4, "deaths": 1, "assists": 6, "win": false, "champLevel": 15,
                         "totalMinionsKilled": 150, "neutralMinionsKilled": 10, "goldEarned": 11000,
                         "totalDamageDealtToChampions": 20000, "item0": 3031}},
              {"participantId": 6, "teamId": 200, "championId": 1,
               "stats": {"kills": 9, "deaths": 4, "assists": 2, "win": true}}
            ],
            "participantIdentities": [
              {"participantId": 6, "player": {"puuid": "enemy", "gameName": "Враг", "tagLine": "RU1"}},
              {"participantId": 1, "player": {"puuid": "me", "gameName": "Я", "tagLine": "RU2"}}
            ],
            "teams": [{"teamId": 100, "win": "Fail", "towerKills": 3}, {"teamId": 200, "win": "Win", "towerKills": 8, "dragonKills": 2}]}"#;
        let game: LcuGame = serde_json::from_str(json).unwrap();
        let d = game.details(Some("me"));
        assert_eq!(d.id, "RU_7");
        assert_eq!(d.teams.len(), 2);
        let mine = &d.teams[0];
        assert!(mine.is_mine() && !mine.win);
        assert_eq!(mine.towers, 3);
        assert_eq!(mine.players[0].riot_id.as_ref().unwrap().to_string(), "Я#RU2");
        assert_eq!((mine.players[0].stats.cs, mine.players[0].stats.items[0]), (160, 3031));
        let enemy = &d.teams[1];
        assert!(enemy.win && !enemy.players[0].is_me);
        assert_eq!((enemy.towers, enemy.dragons, enemy.kills()), (8, 2, 9));
    }

    #[test]
    fn apex_tier_without_division() {
        let json = r#"{"queues":[{"queueType":"RANKED_FLEX_SR","tier":"MASTER","division":"NA",
            "leaguePoints":120,"wins":50,"losses":40}]}"#;
        let stats: LcuRankedStats = serde_json::from_str(json).unwrap();
        assert_eq!(stats.to_queues().flex.unwrap().rank_label(), "Мастер");
    }
}
