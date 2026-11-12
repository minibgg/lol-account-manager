//! Модели данных Riot API.
//!
//! Два уровня:
//! - DTO (`...Dto`) — ровно то, что приходит в JSON. serde разбирает их автоматически;
//! - доменные типы (`Tier`, `RankedStats`, ...) — удобные для приложения: с порядком
//!   для сортировки, русскими названиями, винрейтом.

use std::fmt;

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::routing::{Region, RiotId};

pub const QUEUE_SOLO: &str = "RANKED_SOLO_5x5";
pub const QUEUE_FLEX: &str = "RANKED_FLEX_SR";

// ---------------------------------------------------------------------------
// DTO
// ---------------------------------------------------------------------------

/// account-v1. `rename_all = "camelCase"`: поле `game_name` в Rust ↔ `gameName` в JSON.
/// Лишние поля JSON serde просто игнорирует.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub puuid: String,
    // Option<T> — значение может отсутствовать (в JSON нет поля или null).
    pub game_name: Option<String>,
    pub tag_line: Option<String>,
}

/// summoner-v4 (by-puuid). Устаревшие id/accountId Riot убрал — они нам и не нужны.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummonerDto {
    pub profile_icon_id: i32,
    pub summoner_level: i64,
}

/// league-v4 (entries/by-puuid): одна запись на каждую рейтинговую очередь.
/// `#[serde(default)]` — если поля нет, берём значение по умолчанию (0, ""),
/// чтобы одна странная запись не ломала разбор всего ответа.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LeagueEntryDto {
    pub queue_type: String,
    pub tier: String,
    pub rank: String,
    pub league_points: i32,
    pub wins: u32,
    pub losses: u32,
}

// ---------------------------------------------------------------------------
// Доменные типы
// ---------------------------------------------------------------------------

/// Тир (лига).
///
/// `derive(PartialOrd, Ord)` сравнивает варианты enum по порядку объявления:
/// Iron < Bronze < … < Challenger. Поэтому сортировка по рангу работает «из коробки».
/// Serialize/Deserialize — для кэша в SQLite (в JSON тир пишется как «GOLD»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Tier {
    Iron,
    Bronze,
    Silver,
    Gold,
    Platinum,
    Emerald,
    Diamond,
    Master,
    Grandmaster,
    Challenger,
}

impl Tier {
    /// Из строки Riot API («GOLD») в Tier. None — неизвестное значение.
    pub fn from_api(s: &str) -> Option<Self> {
        Some(match s {
            "IRON" => Self::Iron,
            "BRONZE" => Self::Bronze,
            "SILVER" => Self::Silver,
            "GOLD" => Self::Gold,
            "PLATINUM" => Self::Platinum,
            "EMERALD" => Self::Emerald,
            "DIAMOND" => Self::Diamond,
            "MASTER" => Self::Master,
            "GRANDMASTER" => Self::Grandmaster,
            "CHALLENGER" => Self::Challenger,
            _ => return None,
        })
    }

    /// Обратно в строку Riot API — для хранения в БД и имён файлов эмблем.
    pub fn api_name(self) -> &'static str {
        match self {
            Self::Iron => "IRON",
            Self::Bronze => "BRONZE",
            Self::Silver => "SILVER",
            Self::Gold => "GOLD",
            Self::Platinum => "PLATINUM",
            Self::Emerald => "EMERALD",
            Self::Diamond => "DIAMOND",
            Self::Master => "MASTER",
            Self::Grandmaster => "GRANDMASTER",
            Self::Challenger => "CHALLENGER",
        }
    }

    /// Название как в русском клиенте LoL.
    pub fn label_ru(self) -> &'static str {
        match self {
            Self::Iron => "Железо",
            Self::Bronze => "Бронза",
            Self::Silver => "Серебро",
            Self::Gold => "Золото",
            Self::Platinum => "Платина",
            Self::Emerald => "Изумруд",
            Self::Diamond => "Алмаз",
            Self::Master => "Мастер",
            Self::Grandmaster => "Грандмастер",
            Self::Challenger => "Претендент",
        }
    }

    /// Мастер и выше — без дивизионов.
    pub fn is_apex(self) -> bool {
        self >= Self::Master
    }
}

/// Дивизион внутри тира. Порядок объявления: Four < Three < Two < One.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Division {
    #[serde(rename = "IV")]
    Four,
    #[serde(rename = "III")]
    Three,
    #[serde(rename = "II")]
    Two,
    #[serde(rename = "I")]
    One,
}

impl Division {
    pub fn from_api(s: &str) -> Option<Self> {
        Some(match s {
            "IV" => Self::Four,
            "III" => Self::Three,
            "II" => Self::Two,
            "I" => Self::One,
            _ => return None,
        })
    }

    pub fn roman(self) -> &'static str {
        match self {
            Self::Four => "IV",
            Self::Three => "III",
            Self::Two => "II",
            Self::One => "I",
        }
    }
}

/// Ранг в одной очереди.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankedStats {
    pub tier: Tier,
    pub division: Division,
    pub lp: i32,
    pub wins: u32,
    /// `None` — неизвестно: клиент League показывает поражения только аккаунта,
    /// в который в нём выполнен вход (у остальных отдаёт 0). В старом кэше здесь число —
    /// оно читается как `Some`.
    pub losses: Option<u32>,
}

impl RankedStats {
    /// None, если тир или дивизион не распознаны.
    pub fn from_entry(e: &LeagueEntryDto) -> Option<Self> {
        Some(Self {
            // `?` работает и с Option: если from_api вернул None — сразу возвращаем None.
            tier: Tier::from_api(&e.tier)?,
            division: Division::from_api(&e.rank)?,
            lp: e.league_points,
            wins: e.wins,
            losses: Some(e.losses),
        })
    }

    /// Всего игр; `None`, если поражения неизвестны.
    pub fn games(&self) -> Option<u32> {
        Some(self.wins + self.losses?)
    }

    /// Винрейт в процентах; `None`, если игр не было или поражения неизвестны.
    pub fn winrate(&self) -> Option<f32> {
        let games = self.games()?;
        (games > 0).then(|| self.wins as f32 * 100.0 / games as f32)
    }

    /// «Золото II» или «Мастер».
    pub fn rank_label(&self) -> String {
        if self.tier.is_apex() {
            self.tier.label_ru().to_string()
        } else {
            format!("{} {}", self.tier.label_ru(), self.division.roman())
        }
    }

    /// Ключ для сортировки: кортежи сравниваются поэлементно — сначала тир,
    /// потом дивизион, потом LP.
    pub fn sort_key(&self) -> (Tier, Division, i32) {
        (self.tier, self.division, self.lp)
    }
}

impl fmt::Display for RankedStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} · {} LP · {}W", self.rank_label(), self.lp, self.wins)?;
        match self.losses {
            Some(losses) => write!(f, " / {losses}L")?,
            None => f.write_str(" / поражения скрыты")?,
        }
        if let Some(wr) = self.winrate() {
            write!(f, " · {wr:.1}%")?;
        }
        Ok(())
    }
}

/// Ранги аккаунта в двух очередях. None — нет ранга (не играл калибровку).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankedQueues {
    pub solo: Option<RankedStats>,
    pub flex: Option<RankedStats>,
}

impl RankedQueues {
    /// `&[LeagueEntryDto]` — срез: заимствованный «вид» на массив/вектор без копирования.
    pub fn from_entries(entries: &[LeagueEntryDto]) -> Self {
        let mut out = Self::default();
        for e in entries {
            let slot = match e.queue_type.as_str() {
                QUEUE_SOLO => &mut out.solo,
                QUEUE_FLEX => &mut out.flex,
                _ => continue, // другие очереди (Арена и т.п.) не показываем
            };
            match RankedStats::from_entry(e) {
                Some(stats) => *slot = Some(stats),
                None => warn!(
                    queue = %e.queue_type, tier = %e.tier, rank = %e.rank,
                    "неизвестный тир/дивизион, запись пропущена"
                ),
            }
        }
        out
    }

    /// Поражения неизвестны (см. [`RankedStats::losses`]).
    pub fn hide_losses(&mut self) {
        for stats in [&mut self.solo, &mut self.flex].into_iter().flatten() {
            stats.losses = None;
        }
    }

    /// Есть ранг, у которого поражения неизвестны.
    pub fn has_hidden_losses(&self) -> bool {
        [&self.solo, &self.flex].into_iter().flatten().any(|s| s.losses.is_none())
    }
}

// ---------------------------------------------------------------------------
// Последние игры
// ---------------------------------------------------------------------------

/// Сколько последних игр показываем на строке аккаунта.
pub const RECENT_MATCHES: usize = 5;

/// Сколько последних игр запрашиваем при каждом обновлении. Все они ложатся в архив
/// (без ограничения), поэтому игры не теряются, даже если между обновлениями их было
/// больше пяти.
pub const FETCH_MATCHES: usize = 20;

/// Сколько новых игр за одно обновление загружать через Riot API (каждая — запрос;
/// лимит ключа разработчика — 100 запросов за 2 минуты). Остальные — при следующем.
pub const API_NEW_MATCHES_PER_REFRESH: usize = 10;

/// Версия набора показателей в [`MatchSummary`]. Записи из кэша с меньшей версией
/// (без урона, золота, предметов) загружаем заново, а не берём из кэша.
pub const MATCH_STATS_VERSION: u8 = 1;

/// Показатели одного игрока в одной игре — и для сводки на строке аккаунта,
/// и для подробностей по всем десяти игрокам.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerStats {
    pub champion_id: i32,
    /// Название чемпиона по-русски. Пустое — не удалось узнать (покажем номер).
    pub champion: String,
    /// Уровень чемпиона в конце игры.
    pub champ_level: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    /// Убитые миньоны и монстры.
    pub cs: u32,
    pub gold: u32,
    /// Урон по чемпионам.
    pub damage: u32,
    /// Очки обзора.
    pub vision: u32,
    /// Предметы: 6 слотов и тринкет; 0 — пустой слот.
    pub items: Vec<i32>,
    /// Лучшая серия убийств подряд: 2 — двойное … 5 — пентакилл.
    pub largest_multi_kill: u32,
}

impl PlayerStats {
    /// «15/11/9».
    pub fn kda(&self) -> String {
        format!("{}/{}/{}", self.kills, self.deaths, self.assists)
    }

    /// KDA-коэффициент: (убийства + помощь) / смерти. Без смертей — «идеальный», делим на 1.
    pub fn kda_ratio(&self) -> f32 {
        (self.kills + self.assists) as f32 / self.deaths.max(1) as f32
    }

    /// CS в минуту.
    pub fn cs_per_minute(&self, duration_secs: u32) -> f32 {
        self.cs as f32 * 60.0 / duration_secs.max(60) as f32
    }

    /// «Тройное убийство» и т. п.; `None` — серий не было.
    pub fn multikill_label(&self) -> Option<&'static str> {
        match self.largest_multi_kill {
            0 | 1 => None,
            2 => Some("Двойное убийство"),
            3 => Some("Тройное убийство"),
            4 => Some("Четверное убийство"),
            _ => Some("Пентакилл"),
        }
    }
}

/// Одна сыгранная игра — для полоски «последние игры».
/// Serialize/Deserialize — для кэша в SQLite (JSON-массив в account_cache).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchSummary {
    /// «EUW1_7012345678» — одинаковый вид для клиента League и Riot API: по нему
    /// понимаем, какие игры уже есть в кэше, и не загружаем их заново.
    pub id: String,
    pub win: bool,
    /// Ремейк — игру закончили в первые минуты; это не победа и не поражение.
    #[serde(default)]
    pub remake: bool,
    pub queue_id: i32,
    /// Начало игры, unix-время в секундах.
    pub played_at: i64,
    pub duration_secs: u32,
    /// См. [`MATCH_STATS_VERSION`]; у записей, сохранённых до появления версии, — 0.
    #[serde(default)]
    pub stats_version: u8,
    /// Показатели самого игрока. `flatten` — в JSON поля лежат прямо в записи игры,
    /// как и до появления `PlayerStats`, поэтому старый кэш читается.
    #[serde(flatten)]
    pub me: PlayerStats,
}

impl MatchSummary {
    /// «Ранговая соло/дуо», «ARAM» и т. п.
    pub fn queue_label(&self) -> &'static str {
        queue_label(self.queue_id)
    }

    /// «15/11/9».
    pub fn kda(&self) -> String {
        self.me.kda()
    }

    /// В кэше есть все показатели (урон, золото, предметы) — заново загружать не нужно.
    pub fn has_full_stats(&self) -> bool {
        self.stats_version >= MATCH_STATS_VERSION
    }

    /// Номер игры без платформы: «RU_573082534» → 573082534 (так игру ищет клиент League).
    pub fn game_id(match_id: &str) -> Option<i64> {
        match_id.rsplit_once('_')?.1.parse().ok()
    }
}

/// Подробности одной игры: обе команды целиком. Serialize/Deserialize — для архива
/// в SQLite: сохранённую игру можно открыть и через год, когда клиент её уже не отдаст.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchDetails {
    pub id: String,
    pub queue_id: i32,
    pub played_at: i64,
    pub duration_secs: u32,
    pub remake: bool,
    /// Своя команда первой.
    pub teams: Vec<TeamDetails>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamDetails {
    pub win: bool,
    pub towers: u32,
    pub dragons: u32,
    pub barons: u32,
    pub players: Vec<PlayerLine>,
}

impl TeamDetails {
    pub fn kills(&self) -> u32 {
        self.players.iter().map(|p| p.stats.kills).sum()
    }

    /// Есть ли в команде игрок, для которого открыли подробности.
    pub fn is_mine(&self) -> bool {
        self.players.iter().any(|p| p.is_me)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerLine {
    /// `None` — имя скрыто или неизвестно.
    pub riot_id: Option<RiotId>,
    /// Это тот аккаунт, для которого открыли подробности.
    pub is_me: bool,
    pub stats: PlayerStats,
}

impl PlayerLine {
    /// Участие в убийствах команды, %.
    pub fn kill_participation(&self, team_kills: u32) -> Option<f32> {
        (team_kills > 0).then(|| (self.stats.kills + self.stats.assists) as f32 * 100.0 / team_kills as f32)
    }
}

impl MatchDetails {
    /// Своя команда — первой (порядок остальных не меняется).
    pub fn put_my_team_first(&mut self) {
        // sort_by_key устойчивая: `false` (своя) раньше `true`.
        self.teams.sort_by_key(|t| !t.is_mine());
    }

    /// Все игроки обеих команд — например, чтобы подставить названия чемпионов.
    pub fn players_mut(&mut self) -> impl Iterator<Item = &mut PlayerLine> {
        self.teams.iter_mut().flat_map(|t| t.players.iter_mut())
    }
}

/// Название очереди (режима) по её номеру. Номера — из справочника Riot (queues.json).
pub fn queue_label(queue_id: i32) -> &'static str {
    match queue_id {
        420 => "Ранговая соло/дуо",
        440 => "Ранговая флекс",
        400 => "Обычная (выбор)",
        430 => "Обычная (вслепую)",
        480 | 490 => "Быстрая игра",
        450 => "ARAM",
        700 | 720 => "Clash",
        900 | 1010 | 1900 => "URF",
        1700 | 1710 => "Арена",
        830 | 840 | 850 | 870 | 880 | 890 => "Против ИИ",
        0 => "Своя игра",
        _ => "Другой режим",
    }
}

/// match-v5: список id игр — просто массив строк. Сама игра (`/matches/{id}`):
#[derive(Debug, Clone, Deserialize)]
pub struct MatchDto {
    pub metadata: MatchMetadataDto,
    pub info: MatchInfoDto,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchMetadataDto {
    pub match_id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MatchInfoDto {
    /// Начало игры, unix-время в миллисекундах.
    pub game_creation: i64,
    /// Длительность в секундах (в очень старых играх — в миллисекундах, см. ниже).
    pub game_duration: i64,
    /// Есть только у игр после патча 11.20 — по нему и отличаем секунды от миллисекунд.
    pub game_end_timestamp: Option<i64>,
    pub queue_id: i32,
    pub participants: Vec<MatchParticipantDto>,
    pub teams: Vec<MatchTeamDto>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MatchParticipantDto {
    pub puuid: String,
    pub riot_id_game_name: String,
    pub riot_id_tagline: String,
    pub team_id: i32,
    pub champion_id: i32,
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
    pub game_ended_in_early_surrender: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MatchTeamDto {
    pub team_id: i32,
    pub win: bool,
    pub objectives: MatchObjectivesDto,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MatchObjectivesDto {
    pub tower: MatchObjectiveDto,
    pub dragon: MatchObjectiveDto,
    pub baron: MatchObjectiveDto,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MatchObjectiveDto {
    pub kills: u32,
}

impl MatchParticipantDto {
    fn stats(&self) -> PlayerStats {
        PlayerStats {
            champion_id: self.champion_id,
            champion: String::new(),
            champ_level: self.champ_level,
            kills: self.kills,
            deaths: self.deaths,
            assists: self.assists,
            cs: self.total_minions_killed + self.neutral_minions_killed,
            gold: self.gold_earned,
            damage: self.total_damage_dealt_to_champions,
            vision: self.vision_score,
            items: vec![self.item0, self.item1, self.item2, self.item3, self.item4, self.item5, self.item6],
            largest_multi_kill: self.largest_multi_kill,
        }
    }
}

impl MatchDto {
    fn duration_secs(&self) -> u32 {
        let duration = if self.info.game_end_timestamp.is_some() {
            self.info.game_duration
        } else {
            self.info.game_duration / 1000
        };
        duration.max(0) as u32
    }

    /// Итог игры для игрока `puuid`. `None` — его нет среди участников.
    pub fn summary_for(&self, puuid: &str) -> Option<MatchSummary> {
        let me = self.info.participants.iter().find(|p| p.puuid == puuid)?;
        Some(MatchSummary {
            id: self.metadata.match_id.clone(),
            win: me.win,
            remake: me.game_ended_in_early_surrender,
            queue_id: self.info.queue_id,
            played_at: self.info.game_creation / 1000,
            duration_secs: self.duration_secs(),
            stats_version: MATCH_STATS_VERSION,
            me: me.stats(),
        })
    }

    /// Обе команды целиком. `me_puuid` — чью строку выделить.
    pub fn details(&self, me_puuid: Option<&str>) -> MatchDetails {
        // Команды в match-v5: 100 — синяя, 200 — красная.
        let mut teams: Vec<TeamDetails> = [100, 200]
            .into_iter()
            .map(|team_id| {
                let team = self.info.teams.iter().find(|t| t.team_id == team_id);
                let players: Vec<PlayerLine> = self
                    .info
                    .participants
                    .iter()
                    .filter(|p| p.team_id == team_id)
                    .map(|p| PlayerLine {
                        riot_id: crate::lobby::riot_id(&p.riot_id_game_name, &p.riot_id_tagline),
                        is_me: me_puuid == Some(p.puuid.as_str()),
                        stats: p.stats(),
                    })
                    .collect();
                TeamDetails {
                    win: team.map_or_else(|| self.info.participants.iter().any(|p| p.team_id == team_id && p.win), |t| t.win),
                    towers: team.map_or(0, |t| t.objectives.tower.kills),
                    dragons: team.map_or(0, |t| t.objectives.dragon.kills),
                    barons: team.map_or(0, |t| t.objectives.baron.kills),
                    players,
                }
            })
            .filter(|t| !t.players.is_empty())
            .collect();
        // Арена и другие режимы с командами не 100/200 — показываем всех одним списком.
        if teams.is_empty() && !self.info.participants.is_empty() {
            teams.push(TeamDetails {
                win: false,
                towers: 0,
                dragons: 0,
                barons: 0,
                players: self
                    .info
                    .participants
                    .iter()
                    .map(|p| PlayerLine {
                        riot_id: crate::lobby::riot_id(&p.riot_id_game_name, &p.riot_id_tagline),
                        is_me: me_puuid == Some(p.puuid.as_str()),
                        stats: p.stats(),
                    })
                    .collect(),
            });
        }
        let mut details = MatchDetails {
            id: self.metadata.match_id.clone(),
            queue_id: self.info.queue_id,
            played_at: self.info.game_creation / 1000,
            duration_secs: self.duration_secs(),
            remake: self.info.participants.iter().any(|p| p.game_ended_in_early_surrender),
            teams,
        };
        details.put_my_team_first();
        details
    }
}

/// Откуда получены данные.
///
/// PUUID у источников РАЗНЫЕ: Riot API отдаёт зашифрованный под API-ключ PUUID
/// (78 символов), а клиент League — настоящий (UUID, 36 символов). Поэтому в базе
/// они хранятся в разных столбцах.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSource {
    RiotApi,
    /// Локальный API запущенного клиента League (LCU).
    Lcu,
}

/// Всё, что мы узнали об аккаунте за одно обновление.
#[derive(Debug, Clone)]
pub struct AccountSnapshot {
    pub source: DataSource,
    /// PUUID в «системе координат» источника (см. [`DataSource`]).
    pub puuid: String,
    /// Актуальный Riot ID (игрок мог его сменить).
    pub riot_id: RiotId,
    pub region: Region,
    pub profile_icon_id: i32,
    pub summoner_level: i64,
    pub ranked: RankedQueues,
    /// Последние игры, новые первыми. `None` — загрузить не удалось (тогда в кэше
    /// остаются прежние); ранги при этом всё равно сохраняются.
    pub matches: Option<Vec<MatchSummary>>,
    /// Подробности (обе команды) новых игр, если они пришли вместе с играми — так
    /// бывает с Riot API, где игра и так загружается целиком. Ложатся в архив.
    pub details: Vec<MatchDetails>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_match_v5() {
        // Форма ответа match-v5 (лишние поля serde пропускает).
        let json = r#"{
            "metadata": {"matchId": "EUW1_7012345678", "participants": ["me", "other"]},
            "info": {"gameCreation": 1793934757581, "gameDuration": 1819,
                     "gameEndTimestamp": 1793936600000, "queueId": 420,
                     "participants": [
                        {"puuid": "other", "championId": 1, "kills": 0, "deaths": 0, "assists": 0, "win": true},
                        {"puuid": "me", "championId": 42, "kills": 15, "deaths": 11, "assists": 9,
                         "win": false, "gameEndedInEarlySurrender": false, "goldEarned": 15000}
                     ]}
        }"#;
        let m: MatchDto = serde_json::from_str(json).unwrap();
        let s = m.summary_for("me").unwrap();
        assert_eq!(s.id, "EUW1_7012345678");
        assert_eq!((s.me.champion_id, s.win, s.remake), (42, false, false));
        assert_eq!(s.kda(), "15/11/9");
        assert_eq!(s.played_at, 1793934757);
        assert_eq!(s.duration_secs, 1819);
        assert_eq!(s.queue_label(), "Ранговая соло/дуо");
        assert!(m.summary_for("nobody").is_none());

        // Старые игры без gameEndTimestamp: длительность в миллисекундах.
        let old = MatchDto {
            info: MatchInfoDto {
                game_duration: 1_500_000,
                game_end_timestamp: None,
                ..m.info.clone()
            },
            ..m
        };
        assert_eq!(old.summary_for("me").unwrap().duration_secs, 1500);
        assert_eq!(queue_label(99999), "Другой режим");
    }

    #[test]
    fn match_v5_details_and_old_cache() {
        let json = r#"{"metadata": {"matchId": "RU_5"},
          "info": {"gameCreation": 1000000, "gameDuration": 1800, "gameEndTimestamp": 1, "queueId": 450,
            "participants": [
              {"puuid": "a", "riotIdGameName": "Враг", "riotIdTagline": "RU1", "teamId": 200,
               "championId": 1, "kills": 3, "deaths": 1, "assists": 0, "win": true},
              {"puuid": "me", "riotIdGameName": "Я", "riotIdTagline": "RU1", "teamId": 100,
               "championId": 42, "champLevel": 18, "kills": 10, "deaths": 2, "assists": 5,
               "totalMinionsKilled": 200, "neutralMinionsKilled": 40, "goldEarned": 15000,
               "totalDamageDealtToChampions": 32000, "visionScore": 20, "largestMultiKill": 3,
               "item0": 3031, "item6": 3340, "win": false},
              {"puuid": "b", "teamId": 100, "championId": 2, "kills": 5, "deaths": 4, "assists": 1, "win": false}
            ],
            "teams": [
              {"teamId": 100, "win": false, "objectives": {"tower": {"kills": 2}, "dragon": {"kills": 1}, "baron": {"kills": 0}}},
              {"teamId": 200, "win": true, "objectives": {"tower": {"kills": 9}}}
            ]}}"#;
        let m: MatchDto = serde_json::from_str(json).unwrap();
        let s = m.summary_for("me").unwrap();
        assert_eq!((s.me.cs, s.me.gold, s.me.damage, s.me.champ_level), (240, 15000, 32000, 18));
        assert_eq!(s.me.items, [3031, 0, 0, 0, 0, 0, 3340]);
        assert_eq!(s.me.multikill_label(), Some("Тройное убийство"));
        assert!((s.me.kda_ratio() - 7.5).abs() < 0.01);
        assert!((s.me.cs_per_minute(s.duration_secs) - 8.0).abs() < 0.01);
        assert!(s.has_full_stats());

        let d = m.details(Some("me"));
        assert_eq!(d.teams.len(), 2);
        assert!(d.teams[0].is_mine() && !d.teams[0].win, "своя команда первой");
        assert_eq!(d.teams[0].kills(), 15);
        assert_eq!((d.teams[0].towers, d.teams[0].dragons, d.teams[1].towers), (2, 1, 9));
        let me = &d.teams[0].players[0];
        assert!(me.is_me);
        assert_eq!(me.riot_id.as_ref().unwrap().to_string(), "Я#RU1");
        assert_eq!(me.kill_participation(15).unwrap().round(), 100.0);
        assert!(d.teams[0].players[1].riot_id.is_none(), "имя не пришло — None");

        // Кэш старого вида (без показателей и версии) читается; такие игры загрузим заново.
        let old = r#"{"id":"RU_1","champion_id":42,"champion":"Корки","win":true,"kills":1,
                      "deaths":2,"assists":3,"queue_id":420,"played_at":5,"duration_secs":60}"#;
        let old: MatchSummary = serde_json::from_str(old).unwrap();
        assert_eq!((old.me.champion_id, old.me.champion.as_str(), old.kda().as_str()), (42, "Корки", "1/2/3"));
        assert!(!old.has_full_stats());
        assert_eq!(MatchSummary::game_id("RU_573082534"), Some(573_082_534));
        assert_eq!(MatchSummary::game_id("EUW1_7"), Some(7));
    }

    // r#"..."# — «сырая» строка: внутри можно писать кавычки без экранирования.
    const SAMPLE: &str = r#"[
        {"leagueId":"a","queueType":"RANKED_FLEX_SR","tier":"EMERALD","rank":"III",
         "puuid":"p","leaguePoints":12,"wins":5,"losses":7,
         "veteran":false,"inactive":false,"freshBlood":true,"hotStreak":false},
        {"leagueId":"b","queueType":"RANKED_SOLO_5x5","tier":"MASTER","rank":"I",
         "puuid":"p","leaguePoints":210,"wins":120,"losses":100,
         "veteran":false,"inactive":false,"freshBlood":false,"hotStreak":true},
        {"queueType":"CHERRY","puuid":"p","leaguePoints":0,"wins":3,"losses":1}
    ]"#;

    #[test]
    fn parses_league_entries() {
        let entries: Vec<LeagueEntryDto> = serde_json::from_str(SAMPLE).unwrap();
        let q = RankedQueues::from_entries(&entries);

        let solo = q.solo.expect("solo");
        assert_eq!(solo.tier, Tier::Master);
        assert_eq!(solo.lp, 210);
        assert_eq!(solo.rank_label(), "Мастер");

        let flex = q.flex.expect("flex");
        assert_eq!(flex.rank_label(), "Изумруд III");
        assert_eq!(flex.games(), Some(12));
    }

    #[test]
    fn empty_entries_means_unranked() {
        let q = RankedQueues::from_entries(&[]);
        assert_eq!(q, RankedQueues::default());
    }

    #[test]
    fn rank_ordering() {
        let r = |tier, division, lp| RankedStats {
            tier,
            division,
            lp,
            wins: 0,
            losses: Some(0),
        };
        assert!(
            r(Tier::Master, Division::One, 0).sort_key()
                > r(Tier::Diamond, Division::One, 99).sort_key()
        );
        assert!(
            r(Tier::Gold, Division::One, 0).sort_key() > r(Tier::Gold, Division::Two, 99).sort_key()
        );
        assert!(r(Tier::Gold, Division::Two, 50).sort_key() > r(Tier::Gold, Division::Two, 49).sort_key());
    }

    #[test]
    fn winrate_and_display() {
        let s = RankedStats {
            tier: Tier::Gold,
            division: Division::Two,
            lp: 45,
            wins: 30,
            losses: Some(25),
        };
        assert_eq!(s.to_string(), "Золото II · 45 LP · 30W / 25L · 54.5%");

        let none = RankedStats { wins: 0, losses: Some(0), ..s.clone() };
        assert_eq!(none.winrate(), None);

        // Поражения неизвестны — ни винрейта, ни числа игр, и в тексте честно.
        let hidden = RankedStats { losses: None, ..s };
        assert_eq!((hidden.winrate(), hidden.games()), (None, None));
        assert_eq!(hidden.to_string(), "Золото II · 45 LP · 30W / поражения скрыты");
        let mut queues = RankedQueues { solo: Some(hidden.clone()), flex: None };
        assert!(queues.has_hidden_losses());
        queues.solo.as_mut().unwrap().losses = Some(1);
        assert!(!queues.has_hidden_losses());
        queues.hide_losses();
        assert!(queues.has_hidden_losses());

        // Старый кэш (число) и null читаются.
        let cached: RankedStats = serde_json::from_str(
            r#"{"tier":"GOLD","division":"II","lp":45,"wins":30,"losses":25}"#,
        )
        .unwrap();
        assert_eq!(cached.losses, Some(25));
    }
}
