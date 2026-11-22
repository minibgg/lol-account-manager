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
use crate::lcu::models::{ChatParticipant, LcuChampSelectSession, LcuGameData, LcuGameflowSession};
use crate::riot::{MatchSummary, RankedQueues, Region, RiotId};

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
    /// Прошлые игры с этим игроком (по архиву всех ваших аккаунтов); `None` — не встречались.
    pub met: Option<Encounters>,
}

impl LobbyPlayer {
    pub fn new(puuid: String) -> Self {
        Self {
            puuid,
            ..Self::default()
        }
    }
}

/// Игра, которая идёт прямо сейчас («в игре · экко · 12 мин»).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LiveGame {
    /// Номер игры (как в «RU_573082534» после подчёркивания).
    pub game_id: i64,
    pub queue_id: i32,
    pub champion_id: i32,
    /// Название чемпиона; пустое — неизвестно.
    pub champion: String,
    /// «top» … «utility»; пусто — неизвестна или в режиме нет позиций.
    pub position: String,
    /// Когда началась (unix-время). Для игры из клиента — когда мы её впервые увидели.
    pub started_at: i64,
}

impl LiveGame {
    /// Игра аккаунта `my_puuid` из ответа клиента League.
    pub fn from_session(
        session: &LcuGameflowSession,
        my_puuid: &str,
        champions: &HashMap<i32, String>,
        now: i64,
    ) -> Option<Self> {
        let data = &session.game_data;
        if data.game_id <= 0 {
            return None;
        }
        let champion_id = data
            .player_champion_selections
            .iter()
            .find(|s| s.puuid == my_puuid)
            .map_or(0, |s| s.champion_id);
        let position = data
            .team_one
            .iter()
            .chain(&data.team_two)
            .find(|m| m.puuid == my_puuid)
            .map(|m| m.selected_position.to_lowercase())
            // «none» и всё незнакомое — позиции нет.
            .filter(|p| Position::from_lcu(p).is_some())
            .unwrap_or_default();
        Some(Self {
            game_id: data.game_id,
            queue_id: data.queue.id,
            champion_id,
            champion: champions.get(&champion_id).cloned().unwrap_or_default(),
            position,
            started_at: now,
        })
    }

    /// «в игре · экко · 12 мин».
    pub fn label(&self, now: i64) -> String {
        let mut text = "в игре".to_string();
        if !self.champion.is_empty() {
            text.push_str(&format!(" · {}", self.champion.to_lowercase()));
        }
        text.push_str(&format!(" · {} мин", (now - self.started_at).max(0) / 60));
        text
    }
}

/// Что показывает вкладка «Лобби».
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LobbyState {
    pub phase: LobbyPhase,
    pub players: Vec<LobbyPlayer>,
    /// Почему имён может не хватать (чат ещё не подключился и т.п.).
    pub problem: Option<AppError>,
    /// Игра, которая сейчас идёт у аккаунта из клиента.
    pub game: Option<LiveGame>,
    /// Команда противника — с загрузки игры и до её конца.
    pub enemies: Vec<EnemyPlayer>,
}

// ----- Противники (с экрана загрузки) -------------------------------------------
// Источник — `/lol-gameflow/v1/session`: там PUUID и чемпионы всех десяти игроков.
// Проверено на живой ранговой (RU, 2026-11-21): в `teamOne`/`teamTwo` клиент пропускает
// по одному игроку (видимо, со скрытым профилем), а в `playerChampionSelections` есть все
// десять — первые пять из `teamOne`, следующие пять из `teamTwo`.

/// Игрок команды противника.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnemyPlayer {
    pub puuid: String,
    pub riot_id: Option<RiotId>,
    pub champion_id: i32,
    pub champion: Option<String>,
    pub position: Option<Position>,
    /// `None` — ещё не загружен или не удалось.
    pub profile: Option<PlayerProfile>,
    /// Последние игры; `None` — ещё не загружены.
    pub form: Option<RecentForm>,
    /// Прошлые игры с этим игроком (по архиву всех ваших аккаунтов); `None` — не встречались.
    pub met: Option<Encounters>,
}

// ----- «Уже встречались» и «похоже на смурфа» ------------------------------------

/// Одна прошлая игра с этим игроком — строка из архива игр (см. `Db::meetings`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meeting {
    pub match_id: String,
    pub played_at: i64,
    /// Ваш аккаунт, на котором была игра («Имя#TAG»).
    pub account: String,
    /// Он был в вашей команде.
    pub same_team: bool,
    /// Вы выиграли.
    pub won: bool,
    /// Чемпион, на котором играл он.
    pub champion: String,
}

/// Сколько раз вы уже играли с этим игроком или против него. Ремейки не считаются.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Encounters {
    /// Игры, где он был в вашей команде, и ваши победы в них.
    pub with: u32,
    pub with_wins: u32,
    /// Игры против него и ваши победы в них.
    pub against: u32,
    pub against_wins: u32,
    /// Последняя встреча: когда (unix-время), на каком вашем аккаунте, на ком играл он.
    pub last_at: i64,
    pub last_account: String,
    pub last_champion: String,
}

impl Encounters {
    /// Собирает встречи из строк архива. Одна и та же игра может быть в архиве двух
    /// ваших аккаунтов (играли вместе) — считаем её один раз. `None` — встреч нет.
    pub fn from_meetings(meetings: &[Meeting]) -> Option<Self> {
        let mut seen = HashSet::new();
        let mut result = Self::default();
        for m in meetings {
            if !seen.insert(m.match_id.as_str()) {
                continue;
            }
            if m.same_team {
                result.with += 1;
                result.with_wins += u32::from(m.won);
            } else {
                result.against += 1;
                result.against_wins += u32::from(m.won);
            }
            if m.played_at >= result.last_at {
                result.last_at = m.played_at;
                result.last_account = m.account.clone();
                result.last_champion = m.champion.clone();
            }
        }
        (result.games() > 0).then_some(result)
    }

    pub fn games(&self) -> u32 {
        self.with + self.against
    }

    /// Метка в таблице: «↺3».
    pub fn mark(&self) -> String {
        format!("↺{}", self.games())
    }

    /// Подсказка: «Уже встречались: 3 игры. Вместе — 2 (побед 1), против — 1 (побед 0).
    /// Последняя — 5 дн. назад, на аккаунте X; он играл на Ари.»
    pub fn describe(&self, now: i64) -> String {
        let mut parts = Vec::new();
        if self.with > 0 {
            parts.push(format!("в вашей команде — {} (ваших побед {})", self.with, self.with_wins));
        }
        if self.against > 0 {
            parts.push(format!("против вас — {} (ваших побед {})", self.against, self.against_wins));
        }
        let days = (now - self.last_at).max(0) / 86_400;
        let when = match days {
            0 => "сегодня".to_string(),
            1 => "вчера".to_string(),
            n => format!("{n} дн. назад"),
        };
        let mut text = format!("Уже встречались: {}. {}.", games_count(self.games()), parts.join(", "));
        text.push_str(&format!("\nПоследняя игра — {when}, ваш аккаунт {}", self.last_account));
        if !self.last_champion.is_empty() {
            text.push_str(&format!("; он играл на чемпионе «{}»", self.last_champion));
        }
        text.push('.');
        text
    }
}

/// «1 игра», «3 игры», «5 игр».
pub fn games_count(n: u32) -> String {
    let word = match (n % 10, n % 100) {
        (1, 11) => "игр",
        (1, _) => "игра",
        (2..=4, 12..=14) => "игр",
        (2..=4, _) => "игры",
        _ => "игр",
    };
    format!("{n} {word}")
}

/// Ниже этого уровня аккаунт считаем новым (для пометки «смурф?»).
pub const SMURF_LEVEL: i64 = 60;

/// Похоже ли на смурфа: аккаунт новый (уровень ниже [`SMURF_LEVEL`]), а играет как
/// опытный — высокий ранг или очень много побед. Возвращает пояснение для подсказки;
/// `None` — не похоже или данных мало. Это только догадка по цифрам, а не приговор.
pub fn smurf_hint(profile: Option<&PlayerProfile>, form: Option<&RecentForm>) -> Option<String> {
    let profile = profile?;
    let level = profile.summoner_level;
    if level <= 0 || level >= SMURF_LEVEL {
        return None;
    }
    let mut reasons = Vec::new();
    if let Some(f) = form
        && f.games >= 8
        && let Some(wr) = f.winrate()
        && wr >= 65
    {
        reasons.push(format!("{wr}% побед в последних {}", games_count(f.games)));
    }
    let queues = [("solo/duo", profile.ranked.solo.as_ref()), ("flex", profile.ranked.flex.as_ref())];
    if let Some((queue, stats)) = queues
        .iter()
        .filter_map(|(q, s)| s.map(|s| (*q, s)))
        .filter(|(_, s)| s.tier >= crate::riot::Tier::Emerald)
        .max_by_key(|(_, s)| s.tier)
    {
        reasons.push(format!("{} в {queue}", stats.rank_label().to_lowercase()));
    }
    if let Some(stats) = profile.ranked.solo.as_ref()
        && stats.games().is_some_and(|g| g >= 20)
        && let Some(wr) = stats.winrate()
        && wr >= 65.0
    {
        reasons.push(format!("{wr:.0}% побед в solo/duo"));
    }
    (!reasons.is_empty()).then(|| format!("Похоже на смурфа: уровень {level}, {}.", reasons.join(", ")))
}

/// Форма игрока по последним играм (без ремейков).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecentForm {
    pub games: u32,
    pub wins: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    /// Игры и победы на чемпионе, которого он взял сейчас.
    pub champion_games: u32,
    pub champion_wins: u32,
    /// Серия: +3 — три победы подряд, -2 — два поражения (с последней игры).
    pub streak: i32,
}

impl RecentForm {
    /// `matches` — новые первыми (так их отдаёт клиент).
    pub fn from_matches(matches: &[MatchSummary], champion_id: i32) -> Self {
        let mut form = Self::default();
        let mut streak_open = true;
        for game in matches.iter().filter(|m| !m.remake) {
            form.games += 1;
            form.wins += u32::from(game.win);
            form.kills += game.me.kills;
            form.deaths += game.me.deaths;
            form.assists += game.me.assists;
            if champion_id > 0 && game.me.champion_id == champion_id {
                form.champion_games += 1;
                form.champion_wins += u32::from(game.win);
            }
            // Серия идёт, пока результат совпадает с самой последней игрой.
            if streak_open {
                let step = if game.win { 1 } else { -1 };
                if form.streak == 0 || form.streak.signum() == step {
                    form.streak += step;
                } else {
                    streak_open = false;
                }
            }
        }
        form
    }

    /// Процент побед; `None` — игр нет.
    pub fn winrate(&self) -> Option<u32> {
        (self.games > 0).then(|| self.wins * 100 / self.games)
    }

    /// KDA: (убийства + помощь) / смерти.
    pub fn kda(&self) -> f32 {
        (self.kills + self.assists) as f32 / self.deaths.max(1) as f32
    }
}

/// Команда противника из сессии игры. `my_puuid` — аккаунт из клиента.
pub fn enemy_team(data: &LcuGameData, my_puuid: &str, champions: &HashMap<i32, String>) -> Vec<EnemyPlayer> {
    let in_one = data.team_one.iter().any(|m| m.puuid == my_puuid);
    let in_two = data.team_two.iter().any(|m| m.puuid == my_puuid);
    let selections = &data.player_champion_selections;
    // Сторона по порядку выбора: первые пять — teamOne (нужно, когда игрока нет в списках команд).
    let first_half = |puuid: &str| {
        (selections.len() == 10)
            .then(|| selections.iter().position(|s| s.puuid == puuid).map(|i| i < 5))
            .flatten()
    };
    let i_am_first = if in_one || in_two { in_one } else {
        match first_half(my_puuid) {
            Some(first) => first,
            None => return Vec::new(), // не понять, где мы
        }
    };
    let (enemy_members, ally_members) = if i_am_first {
        (&data.team_two, &data.team_one)
    } else {
        (&data.team_one, &data.team_two)
    };
    let champion = |id: i32| (id > 0).then(|| champions.get(&id).cloned().unwrap_or_else(|| format!("чемпион #{id}")));

    let mut enemies: Vec<EnemyPlayer> = enemy_members
        .iter()
        .filter(|m| !m.puuid.is_empty())
        .map(|m| {
            let champion_id = selections.iter().find(|s| s.puuid == m.puuid).map_or(0, |s| s.champion_id);
            EnemyPlayer {
                puuid: m.puuid.clone(),
                champion_id,
                champion: champion(champion_id),
                position: Position::from_lcu(&m.selected_position.to_lowercase()),
                ..EnemyPlayer::default()
            }
        })
        .collect();
    // Те, кого нет в списках команд, — по порядку выбора.
    let listed = |puuid: &str| enemy_members.iter().chain(ally_members).any(|m| m.puuid == puuid);
    for selection in selections.iter().filter(|s| !s.puuid.is_empty() && !listed(&s.puuid)) {
        if first_half(&selection.puuid) == Some(!i_am_first) {
            enemies.push(EnemyPlayer {
                puuid: selection.puuid.clone(),
                champion_id: selection.champion_id,
                champion: champion(selection.champion_id),
                ..EnemyPlayer::default()
            });
        }
    }
    // Одному игроку без позиции достаётся единственная оставшаяся.
    const ALL: [Position; 5] = [Position::Top, Position::Jungle, Position::Middle, Position::Bottom, Position::Utility];
    let free: Vec<Position> = ALL
        .into_iter()
        .filter(|p| !enemies.iter().any(|e| e.position == Some(*p)))
        .collect();
    let mut unknown = enemies.iter_mut().filter(|e| e.position.is_none());
    // В ARAM позиций нет вовсе — там свободны все пять, и это условие не сработает.
    if let (Some(player), None, [only]) = (unknown.next(), unknown.next(), free.as_slice()) {
        player.position = Some(*only);
    }
    enemies.sort_by_key(|e| crate::clash::position_order(e.position));
    enemies
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

    fn game_member(puuid: &str, position: &str) -> crate::lcu::models::LcuGameMember {
        crate::lcu::models::LcuGameMember {
            puuid: puuid.into(),
            selected_position: position.into(),
        }
    }

    fn selection(puuid: &str, champion: i32) -> crate::lcu::models::LcuChampionSelection {
        crate::lcu::models::LcuChampionSelection {
            champion_id: champion,
            puuid: puuid.into(),
        }
    }

    #[test]
    fn enemy_team_like_live_ranked_game() {
        // Как в живой ранговой: в каждой команде клиент пропустил по игроку,
        // а в выборе чемпионов есть все десять (первые пять — teamOne).
        let data = LcuGameData {
            game_id: 1,
            team_one: vec![
                game_member("me", "BOTTOM"),
                game_member("a1", "TOP"),
                game_member("a2", "JUNGLE"),
                game_member("a3", "MIDDLE"),
            ],
            team_two: vec![
                game_member("e1", "UTILITY"),
                game_member("e2", "MIDDLE"),
                game_member("e3", "TOP"),
                game_member("e4", "JUNGLE"),
            ],
            player_champion_selections: vec![
                selection("a3", 91),
                selection("me", 42),
                selection("a4", 89),
                selection("a2", 131),
                selection("a1", 82),
                selection("e3", 122),
                selection("e2", 55),
                selection("e5", 222),
                selection("e4", 141),
                selection("e1", 235),
            ],
            ..LcuGameData::default()
        };
        let champions = HashMap::from([(222, "Джинкс".to_string())]);
        let enemies = enemy_team(&data, "me", &champions);
        let list: Vec<_> = enemies.iter().map(|e| (e.puuid.as_str(), e.position)).collect();
        assert_eq!(
            list,
            [
                ("e3", Some(Position::Top)),
                ("e4", Some(Position::Jungle)),
                ("e2", Some(Position::Middle)),
                ("e5", Some(Position::Bottom)), // пропущенный — по оставшейся позиции
                ("e1", Some(Position::Utility)),
            ]
        );
        assert_eq!(enemies[3].champion.as_deref(), Some("Джинкс"));
        assert!(enemies.iter().all(|e| !e.puuid.starts_with('a')), "союзники не попадают");

        // С другой стороны — противники наоборот, вместе с пропущенным a4.
        let other = enemy_team(&data, "e1", &champions);
        assert_eq!(other.len(), 5);
        assert!(other.iter().any(|e| e.puuid == "a4" && e.position == Some(Position::Utility)));
    }

    #[test]
    fn aram_without_positions() {
        let data = LcuGameData {
            team_one: vec![game_member("me", "NONE")],
            team_two: vec![game_member("e1", "NONE"), game_member("e2", "NONE")],
            ..LcuGameData::default()
        };
        let enemies = enemy_team(&data, "me", &HashMap::new());
        assert_eq!(enemies.len(), 2);
        assert!(enemies.iter().all(|e| e.position.is_none()));
    }

    fn played(champion: i32, win: bool, remake: bool) -> MatchSummary {
        MatchSummary {
            id: String::new(),
            win,
            remake,
            queue_id: 420,
            played_at: 0,
            duration_secs: 0,
            stats_version: 0,
            me: crate::riot::PlayerStats {
                champion_id: champion,
                kills: 5,
                deaths: 2,
                assists: 5,
                ..Default::default()
            },
        }
    }

    #[test]
    fn recent_form_counts_champion_and_streak() {
        let games = [
            played(222, true, false),
            played(222, true, false),
            played(1, false, true), // ремейк не считаем — и серию он не рвёт
            played(1, true, false),
            played(222, false, false),
        ];
        let form = RecentForm::from_matches(&games, 222);
        assert_eq!((form.games, form.wins), (4, 3));
        assert_eq!((form.champion_games, form.champion_wins), (3, 2));
        assert_eq!(form.streak, 3);
        assert_eq!(form.winrate(), Some(75));
        assert!((form.kda() - 5.0).abs() < 0.01);

        let losing = [played(1, false, false), played(1, false, false), played(1, true, false)];
        assert_eq!(RecentForm::from_matches(&losing, 0).streak, -2);
        assert_eq!(RecentForm::from_matches(&[], 0).winrate(), None);
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

    fn meeting(id: &str, at: i64, same_team: bool, won: bool) -> Meeting {
        Meeting {
            match_id: id.into(),
            played_at: at,
            account: format!("main#{at}"),
            same_team,
            won,
            champion: format!("чемп {at}"),
        }
    }

    #[test]
    fn encounters_count_each_game_once() {
        let meetings = [
            meeting("RU_1", 100, true, true),
            meeting("RU_1", 100, true, true), // та же игра в архиве второго аккаунта
            meeting("RU_2", 300, false, false),
            meeting("RU_3", 200, true, false),
        ];
        let met = Encounters::from_meetings(&meetings).unwrap();
        assert_eq!((met.with, met.with_wins), (2, 1));
        assert_eq!((met.against, met.against_wins), (1, 0));
        assert_eq!(met.games(), 3);
        assert_eq!(met.mark(), "↺3");
        assert_eq!((met.last_at, met.last_account.as_str(), met.last_champion.as_str()), (300, "main#300", "чемп 300"));
        let text = met.describe(300 + 86_400 * 5);
        assert!(text.starts_with("Уже встречались: 3 игры."), "{text}");
        assert!(text.contains("5 дн. назад"), "{text}");
        assert!(Encounters::from_meetings(&[]).is_none());
    }

    #[test]
    fn games_count_declension() {
        assert_eq!(games_count(1), "1 игра");
        assert_eq!(games_count(3), "3 игры");
        assert_eq!(games_count(11), "11 игр");
        assert_eq!(games_count(22), "22 игры");
        assert_eq!(games_count(25), "25 игр");
    }

    #[test]
    fn smurf_needs_low_level_and_strong_play() {
        use crate::riot::{Division, RankedStats, Tier};
        let stats = |tier, wins, losses| RankedStats {
            tier,
            division: Division::Two,
            lp: 10,
            wins,
            losses,
        };
        let profile = |level, solo| PlayerProfile {
            profile_icon_id: 1,
            summoner_level: level,
            ranked: RankedQueues { solo, flex: None },
        };
        let hot = RecentForm {
            games: 15,
            wins: 12,
            ..RecentForm::default()
        };
        // Новый аккаунт с 80% побед.
        let hint = smurf_hint(Some(&profile(35, None)), Some(&hot)).unwrap();
        assert!(hint.contains("уровень 35") && hint.contains("80% побед"), "{hint}");
        // Тот же винрейт на старом аккаунте — не смурф.
        assert_eq!(smurf_hint(Some(&profile(300, None)), Some(&hot)), None);
        // Новый, но с обычной формой и низким рангом — тоже нет.
        let usual = RecentForm {
            games: 20,
            wins: 10,
            ..RecentForm::default()
        };
        let silver = Some(stats(Tier::Silver, 10, None));
        assert_eq!(smurf_hint(Some(&profile(40, silver)), Some(&usual)), None);
        // Высокий ранг на новом аккаунте — да (даже без истории).
        let emerald = Some(stats(Tier::Emerald, 30, None));
        let hint = smurf_hint(Some(&profile(45, emerald)), None).unwrap();
        assert!(hint.contains("изумруд"), "{hint}");
        // Свой аккаунт с известными поражениями: 70% побед в 30 играх.
        let winning = Some(stats(Tier::Gold, 21, Some(9)));
        assert!(smurf_hint(Some(&profile(50, winning)), None).unwrap().contains("70% побед в solo/duo"));
        // Без профиля — ничего не утверждаем.
        assert_eq!(smurf_hint(None, Some(&hot)), None);
    }
}
