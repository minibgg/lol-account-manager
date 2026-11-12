//! Ответы LCU (только нужные нам поля; остальные serde пропускает).

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
        matches.truncate(FETCH_MATCHES);
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
