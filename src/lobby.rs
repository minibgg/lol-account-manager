//! Вкладка «Лобби»: кто с вами в группе, в выборе чемпионов и в игре.
//!
//! Откуда имена:
//! - **группа** — `/lol-lobby/v2/lobby` клиента League (имена там и так видны);
//! - **выбор чемпионов** — участники комнаты чата выбора чемпионов из Riot Client
//!   (`/chat/v5/participants`, комнаты с «champ-select» в `cid`) плюс сессия выбора
//!   (`/lol-champ-select/v1/session`): из неё позиции и чемпионы. В ранговых клиент
//!   скрывает имена союзников в сессии, а в чате они есть — так работает и reveal.
//!
//! Здесь только типы и чистые функции; опрос клиента — в backend.rs.

use std::collections::{HashMap, HashSet};

use crate::error::AppError;
use crate::lcu::models::{ChatParticipant, LcuChampSelectSession};
use crate::riot::{RankedQueues, Region, RiotId};

/// Этап, на котором сейчас игрок.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LobbyPhase {
    /// Клиент League не запущен.
    #[default]
    NoClient,
    /// Клиент запущен, но вы не в лобби.
    Idle,
    /// В группе (лобби) до поиска игры.
    Party,
    /// Поиск игры или окно «Принять».
    Queue,
    ChampSelect,
    /// Загрузка, игра, экран после игры.
    InGame,
}

impl LobbyPhase {
    /// Из ответа `/lol-gameflow/v1/gameflow-phase`.
    /// Игра ещё идёт (до экрана после игры). На «EndOfGame» клиент уже знает новые LP.
    pub fn game_running(phase: &str) -> bool {
        matches!(
            phase,
            "GameStart" | "InProgress" | "Reconnect" | "WaitingForStats" | "PreEndOfGame"
        )
    }

    pub fn from_gameflow(phase: &str) -> Self {
        match phase {
            "Lobby" => Self::Party,
            "Matchmaking" | "ReadyCheck" | "CheckedIntoTournament" => Self::Queue,
            "ChampSelect" => Self::ChampSelect,
            "GameStart" | "InProgress" | "Reconnect" | "WaitingForStats" | "PreEndOfGame"
            | "EndOfGame" => Self::InGame,
            _ => Self::Idle, // "None" и всё незнакомое
        }
    }

    /// Есть что показать (вкладка «Лобби» помечается точкой).
    pub fn is_active(self) -> bool {
        matches!(self, Self::Party | Self::Queue | Self::ChampSelect | Self::InGame)
    }
}

/// Позиция в выборе чемпионов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Top,
    Jungle,
    Middle,
    Bottom,
    Utility,
}

impl Position {
    pub fn from_lcu(s: &str) -> Option<Self> {
        Some(match s {
            "top" => Self::Top,
            "jungle" => Self::Jungle,
            "middle" => Self::Middle,
            "bottom" => Self::Bottom,
            "utility" => Self::Utility,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Top => "Топ",
            Self::Jungle => "Лес",
            Self::Middle => "Мид",
            Self::Bottom => "Бот",
            Self::Utility => "Поддержка",
        }
    }

    /// Для плашки под иконкой игрока.
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Top => "ТОП",
            Self::Jungle => "ЛЕС",
            Self::Middle => "МИД",
            Self::Bottom => "БОТ",
            Self::Utility => "САП",
        }
    }
}

/// Уровень, иконка и ранги игрока (из клиента League).
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerProfile {
    pub profile_icon_id: i32,
    pub summoner_level: i64,
    pub ranked: RankedQueues,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LobbyPlayer {
    /// Пусто — скрытый игрок, имя узнать не удалось.
    pub puuid: String,
    pub riot_id: Option<RiotId>,
    pub is_self: bool,
    /// Место в выборе чемпионов (порядок пиков).
    pub cell_id: Option<i64>,
    pub position: Option<Position>,
    pub champion: Option<String>,
    /// `None` — ещё не загружен или не удалось.
    pub profile: Option<PlayerProfile>,
}

impl LobbyPlayer {
    pub fn new(puuid: String) -> Self {
        Self {
            puuid,
            ..Self::default()
        }
    }
}

/// Что показывает вкладка «Лобби».
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LobbyState {
    pub phase: LobbyPhase,
    pub players: Vec<LobbyPlayer>,
    /// Почему имён может не хватать (чат ещё не подключился и т.п.).
    pub problem: Option<AppError>,
}

/// Riot ID из имени и тега, если оба есть.
pub fn riot_id(game_name: &str, tag: &str) -> Option<RiotId> {
    (!game_name.is_empty() && !tag.is_empty()).then(|| RiotId {
        game_name: game_name.to_string(),
        tag_line: tag.to_string(),
    })
}

/// Своя команда в выборе чемпионов.
///
/// 1. Участники комнаты чата выбора чемпионов — это и есть имена союзников.
/// 2. Видимые игроки сессии (в обычных играх клиент показывает всех).
/// 3. Позиции, чемпионы и порядок — из сессии, сопоставляем по PUUID.
/// 4. Скрытые места, для которых имени не нашлось, — заглушки «скрытый игрок».
pub fn champ_select_players(
    chat: &[ChatParticipant],
    session: Option<&LcuChampSelectSession>,
    champions: &HashMap<i32, String>,
    my_puuid: Option<&str>,
) -> Vec<LobbyPlayer> {
    let members = session.map(|s| s.my_team.as_slice()).unwrap_or_default();
    let local_cell = session.map(|s| s.local_player_cell_id);

    // HashSet::insert возвращает false, если такой PUUID уже был — так убираем повторы.
    let mut seen = HashSet::new();
    let mut players = Vec::new();
    let from_chat = chat
        .iter()
        .filter(|p| p.cid.contains("champ-select") && !p.puuid.is_empty())
        .map(|p| (p.puuid.as_str(), riot_id(&p.game_name, &p.game_tag)));
    let from_session = members
        .iter()
        .filter(|m| !m.puuid.is_empty())
        .map(|m| (m.puuid.as_str(), riot_id(&m.game_name, &m.tag_line)));
    for (puuid, id) in from_chat.chain(from_session) {
        if seen.insert(puuid.to_string()) {
            players.push(LobbyPlayer {
                riot_id: id,
                ..LobbyPlayer::new(puuid.to_string())
            });
        }
    }

    for player in &mut players {
        player.is_self = my_puuid == Some(player.puuid.as_str());
        if let Some(member) = members.iter().find(|m| m.puuid == player.puuid) {
            player.cell_id = Some(member.cell_id);
            player.position = Position::from_lcu(&member.assigned_position);
            player.champion = champion_name(member.champion_id, member.champion_pick_intent, champions);
            player.is_self |= Some(member.cell_id) == local_cell;
        }
    }

    // Скрытые места сессии: имя есть у кого-то из «неопознанных» игроков чата,
    // но к месту его не привязать. Заглушки добавляем, только если имён не хватает.
    let unmatched = players.iter().filter(|p| p.cell_id.is_none()).count();
    let hidden: Vec<_> = members.iter().filter(|m| m.puuid.is_empty()).collect();
    for member in hidden.iter().skip(unmatched) {
        players.push(LobbyPlayer {
            cell_id: Some(member.cell_id),
            position: Position::from_lcu(&member.assigned_position),
            champion: champion_name(member.champion_id, member.champion_pick_intent, champions),
            ..LobbyPlayer::default()
        });
    }

    // По порядку пиков; неопознанные — в конце, по имени.
    players.sort_by_key(|p| {
        (
            p.cell_id.unwrap_or(i64::MAX),
            p.riot_id.as_ref().map(|id| id.to_string().to_lowercase()),
        )
    });
    players
}

/// Выбранный чемпион, а если ещё не выбран — тот, кого игрок навёл.
fn champion_name(id: i32, intent: i32, champions: &HashMap<i32, String>) -> Option<String> {
    let id = if id > 0 { id } else { intent };
    (id > 0).then(|| champions.get(&id).cloned().unwrap_or_else(|| format!("чемпион #{id}")))
}

// ----- Ссылки на OP.GG (кнопки на вкладке «Лобби», как в reveal) -------------
// Ссылка открывается в браузере только по нажатию; сами по себе имена никуда не уходят.

/// Мультипоиск OP.GG: все игроки на одной странице.
pub fn opgg_multisearch_url(region: Region, ids: &[RiotId]) -> String {
    let names: Vec<String> = ids.iter().map(ToString::to_string).collect();
    format!(
        "https://www.op.gg/multisearch/{}?summoners={}",
        opgg_region(region),
        percent_encode(&names.join(","))
    )
}

/// Профиль одного игрока: `https://www.op.gg/summoners/euw/Имя-TAG`.
pub fn opgg_profile_url(region: Region, id: &RiotId) -> String {
    format!(
        "https://www.op.gg/summoners/{}/{}-{}",
        opgg_region(region),
        percent_encode(&id.game_name),
        percent_encode(&id.tag_line)
    )
}

fn opgg_region(region: Region) -> &'static str {
    match region {
        Region::Br => "br",
        Region::Eune => "eune",
        Region::Euw => "euw",
        Region::Jp => "jp",
        Region::Kr => "kr",
        Region::Lan => "lan",
        Region::Las => "las",
        Region::Me => "me",
        Region::Na => "na",
        Region::Oce => "oce",
        Region::Ru => "ru",
        Region::Sg => "sg",
        Region::Tr => "tr",
        Region::Tw => "tw",
        Region::Vn => "vn",
    }
}

/// Кодирование для адреса: латиница, цифры и `-_.~` как есть, остальное (пробел,
/// `#`, запятая, кириллица) — байты UTF-8 в виде `%XX`.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcu::models::LcuChampSelectMember;

    #[test]
    fn opgg_links_are_encoded() {
        let ids = [
            RiotId::parse("Шрек#RU1").unwrap(),
            RiotId::parse("Big Boss#EUW").unwrap(),
        ];
        assert_eq!(
            opgg_multisearch_url(Region::Ru, &ids),
            "https://www.op.gg/multisearch/ru?summoners=%D0%A8%D1%80%D0%B5%D0%BA%23RU1%2CBig%20Boss%23EUW"
        );
        assert_eq!(
            opgg_profile_url(Region::Euw, &ids[1]),
            "https://www.op.gg/summoners/euw/Big%20Boss-EUW"
        );
    }

    fn chat(cid: &str, name: &str, puuid: &str) -> ChatParticipant {
        ChatParticipant {
            cid: cid.into(),
            game_name: name.into(),
            game_tag: "RU1".into(),
            puuid: puuid.into(),
        }
    }

    fn member(cell: i64, position: &str, puuid: &str, champion: i32) -> LcuChampSelectMember {
        LcuChampSelectMember {
            cell_id: cell,
            assigned_position: position.into(),
            champion_id: champion,
            puuid: puuid.into(),
            ..Default::default()
        }
    }

    #[test]
    fn ranked_champ_select_reveals_names_from_chat() {
        // Ранговая: в сессии виден только свой PUUID, остальные скрыты.
        let session = LcuChampSelectSession {
            local_player_cell_id: 1,
            my_team: vec![
                member(0, "top", "", 0),
                member(1, "jungle", "me", 64),
                member(2, "middle", "", 0),
            ],
        };
        let chat = vec![
            chat("abc@champ-select.ru1.pvp.net", "Союзник", "p2"),
            chat("abc@champ-select.ru1.pvp.net", "Я", "me"),
            chat("xyz@champ-select.ru1.pvp.net", "Другой", "p3"),
            chat("party@sec.ru1.pvp.net", "Друг из чата", "p9"), // не выбор чемпионов
        ];
        let champions = HashMap::from([(64, "Ли Син".to_string())]);
        let players = champ_select_players(&chat, Some(&session), &champions, Some("me"));

        let names: Vec<_> = players
            .iter()
            .map(|p| p.riot_id.as_ref().map(|id| id.game_name.clone()))
            .collect();
        // Сначала свой (место 1), потом неопознанные по имени; заглушек нет — имён хватает.
        assert_eq!(
            names,
            [Some("Я".into()), Some("Другой".into()), Some("Союзник".into())]
        );
        assert!(players[0].is_self);
        assert_eq!(players[0].position, Some(Position::Jungle));
        assert_eq!(players[0].champion.as_deref(), Some("Ли Син"));
        assert!(players.iter().all(|p| p.puuid != "p9"), "чужие чаты не попадают");
    }

    #[test]
    fn hidden_slots_without_names_become_placeholders() {
        let session = LcuChampSelectSession {
            local_player_cell_id: 0,
            my_team: vec![member(0, "bottom", "me", 0), member(1, "utility", "", 0)],
        };
        // Чат не ответил — имён нет.
        let players = champ_select_players(&[], Some(&session), &HashMap::new(), Some("me"));
        assert_eq!(players.len(), 2);
        assert!(players[0].is_self);
        assert_eq!(players[1].riot_id, None);
        assert_eq!(players[1].position, Some(Position::Utility));
    }

    #[test]
    fn gameflow_phases() {
        assert_eq!(LobbyPhase::from_gameflow("ChampSelect"), LobbyPhase::ChampSelect);
        assert_eq!(LobbyPhase::from_gameflow("Lobby"), LobbyPhase::Party);
        assert_eq!(LobbyPhase::from_gameflow("ReadyCheck"), LobbyPhase::Queue);
        assert_eq!(LobbyPhase::from_gameflow("InProgress"), LobbyPhase::InGame);
        assert_eq!(LobbyPhase::from_gameflow("None"), LobbyPhase::Idle);
        assert!(!LobbyPhase::Idle.is_active());
        assert!(LobbyPhase::game_running("WaitingForStats"));
        assert!(!LobbyPhase::game_running("EndOfGame"), "после игры — уже можно обновлять");
        assert!(!LobbyPhase::game_running("Lobby"));
    }
}
