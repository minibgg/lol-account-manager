//! Регионы LoL и маршрутизация запросов к Riot API.
//!
//! Riot API разделён на хосты двух видов:
//! - платформенные (`euw1.api.riotgames.com`) — summoner-v4, league-v4: данные конкретного сервера;
//! - региональные (`europe.api.riotgames.com`) — account-v1, match-v5: общие для группы серверов.
//!
//! Таблица сверена с developer.riotgames.com/docs/lol и crate riven (октябрь 2026).
//! PH2 и TH2 с 08.01.2025 влиты в SG2, поэтому отдельно не перечислены.

use std::fmt;
use std::str::FromStr;

use crate::error::AppError;

/// Региональный кластер Riot API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegionalRoute {
    Americas,
    Asia,
    Europe,
    Sea,
}

impl RegionalRoute {
    /// Префикс хоста: `europe` → `europe.api.riotgames.com`.
    ///
    /// `&'static str` — ссылка на строку, которая живёт всё время работы программы
    /// (строковый литерал зашит в exe). Такую ссылку можно хранить где угодно.
    pub fn host(self) -> &'static str {
        match self {
            Self::Americas => "americas",
            Self::Asia => "asia",
            Self::Europe => "europe",
            Self::Sea => "sea",
        }
    }
}

/// Сервер (платформа) League of Legends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Region {
    Br,
    Eune,
    Euw,
    Jp,
    Kr,
    Lan,
    Las,
    Me,
    Na,
    Oce,
    Ru,
    Sg,
    Tr,
    Tw,
    Vn,
}

impl Region {
    /// Все регионы в порядке для выпадающего списка (популярные в начале).
    pub const ALL: [Region; 15] = [
        Self::Euw,
        Self::Eune,
        Self::Na,
        Self::Kr,
        Self::Tr,
        Self::Ru,
        Self::Br,
        Self::Lan,
        Self::Las,
        Self::Oce,
        Self::Jp,
        Self::Me,
        Self::Sg,
        Self::Tw,
        Self::Vn,
    ];

    /// Короткое название, как в клиенте: «EUW». Оно же — код региона в базе данных.
    pub fn label(self) -> &'static str {
        match self {
            Self::Br => "BR",
            Self::Eune => "EUNE",
            Self::Euw => "EUW",
            Self::Jp => "JP",
            Self::Kr => "KR",
            Self::Lan => "LAN",
            Self::Las => "LAS",
            Self::Me => "ME",
            Self::Na => "NA",
            Self::Oce => "OCE",
            Self::Ru => "RU",
            Self::Sg => "SG",
            Self::Tr => "TR",
            Self::Tw => "TW",
            Self::Vn => "VN",
        }
    }

    /// Полное название для подсказок в UI.
    pub fn full_name(self) -> &'static str {
        match self {
            Self::Br => "Бразилия",
            Self::Eune => "Европа: Север и Восток",
            Self::Euw => "Европа: Запад",
            Self::Jp => "Япония",
            Self::Kr => "Корея",
            Self::Lan => "Латинская Америка: Север",
            Self::Las => "Латинская Америка: Юг",
            Self::Me => "Ближний Восток",
            Self::Na => "Северная Америка",
            Self::Oce => "Океания",
            Self::Ru => "Россия",
            Self::Sg => "Сингапур, Малайзия, Индонезия, Таиланд, Филиппины",
            Self::Tr => "Турция",
            Self::Tw => "Тайвань, Гонконг, Макао",
            Self::Vn => "Вьетнам",
        }
    }

    /// Платформенный хост: `euw1` → `euw1.api.riotgames.com` (summoner-v4, league-v4).
    pub fn platform(self) -> &'static str {
        match self {
            Self::Br => "br1",
            Self::Eune => "eun1",
            Self::Euw => "euw1",
            Self::Jp => "jp1",
            Self::Kr => "kr",
            Self::Lan => "la1",
            Self::Las => "la2",
            Self::Me => "me1",
            Self::Na => "na1",
            Self::Oce => "oc1",
            Self::Ru => "ru",
            Self::Sg => "sg2",
            Self::Tr => "tr1",
            Self::Tw => "tw2",
            Self::Vn => "vn2",
        }
    }

    /// Региональный кластер для матчей (match-v5, понадобится позже).
    pub fn regional(self) -> RegionalRoute {
        use RegionalRoute::*;
        match self {
            Self::Na | Self::Br | Self::Lan | Self::Las => Americas,
            Self::Kr | Self::Jp => Asia,
            Self::Euw | Self::Eune | Self::Tr | Self::Ru | Self::Me => Europe,
            Self::Oce | Self::Sg | Self::Tw | Self::Vn => Sea,
        }
    }

    /// Кластер для account-v1. Данные аккаунтов одинаковы во всех кластерах,
    /// но account-v1 официально работает только в americas / asia / europe,
    /// поэтому для SEA-серверов ходим в ближайший — asia.
    pub fn account_route(self) -> RegionalRoute {
        match self.regional() {
            RegionalRoute::Sea => RegionalRoute::Asia,
            other => other,
        }
    }

    /// Регион из ответа клиента League (`/riotclient/region-locale`): там бывает
    /// и короткое имя («RU», «EUW»), и платформа («LA1», «OC1»).
    /// Филиппины и Таиланд с января 2025 играют на сервере SG2.
    pub fn from_lcu(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "PH" | "PH2" | "TH" | "TH2" => Some(Self::Sg),
            other => other.parse().ok(),
        }
    }
}

/// `impl Display` позволяет писать `format!("{region}")` и `region.to_string()`.
impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Разбор из строки: принимает «EUW», «euw», «euw1» и т.п.
/// Использование: `let r: Region = "euw".parse()?;`
impl FromStr for Region {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        // into_iter().find(...) — перебираем массив и берём первый подходящий элемент.
        // Результат — Option<Region>; ok_or_else превращает None в Err(...).
        Region::ALL
            .into_iter()
            .find(|r| r.label().eq_ignore_ascii_case(s) || r.platform().eq_ignore_ascii_case(s))
            .ok_or_else(|| AppError::UnknownRegion(s.to_string()))
    }
}

/// Riot ID: «gameName#tagLine», например «Hide on bush#KR1».
/// Serialize/Deserialize — для архива подробностей игр в SQLite (имена игроков).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RiotId {
    pub game_name: String,
    pub tag_line: String,
}

impl RiotId {
    /// Разбирает строку «Имя#TAG».
    ///
    /// Параметр `&str` — заимствованная строка (только читаем), а в структуру кладём
    /// `String` — собственную копию. `to_string()` как раз создаёт такую копию.
    pub fn parse(input: &str) -> Result<Self, AppError> {
        let (name, tag) = input.trim().split_once('#').ok_or(AppError::InvalidRiotId)?;
        let (name, tag) = (name.trim(), tag.trim());

        // Правила Riot: имя 3–16 символов, тег 3–5. Проверяем мягко (только верхнюю
        // границу): у старых аккаунтов встречаются короткие имена. Считаем символы,
        // а не байты: «Ё» в UTF-8 занимает 2 байта.
        let valid = !name.is_empty()
            && !tag.is_empty()
            && !tag.contains('#')
            && name.chars().count() <= 16
            && tag.chars().count() <= 5;

        if valid {
            Ok(Self {
                game_name: name.to_string(),
                tag_line: tag.to_string(),
            })
        } else {
            Err(AppError::InvalidRiotId)
        }
    }
}

impl fmt::Display for RiotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.game_name, self.tag_line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_table_matches_riot_docs() {
        use RegionalRoute::*;
        let expected = [
            (Region::Br, "br1", Americas),
            (Region::Eune, "eun1", Europe),
            (Region::Euw, "euw1", Europe),
            (Region::Jp, "jp1", Asia),
            (Region::Kr, "kr", Asia),
            (Region::Lan, "la1", Americas),
            (Region::Las, "la2", Americas),
            (Region::Me, "me1", Europe),
            (Region::Na, "na1", Americas),
            (Region::Oce, "oc1", Sea),
            (Region::Ru, "ru", Europe),
            (Region::Sg, "sg2", Sea),
            (Region::Tr, "tr1", Europe),
            (Region::Tw, "tw2", Sea),
            (Region::Vn, "vn2", Sea),
        ];
        assert_eq!(expected.len(), Region::ALL.len());
        for (region, platform, regional) in expected {
            assert_eq!(region.platform(), platform, "{region}");
            assert_eq!(region.regional(), regional, "{region}");
        }
    }

    #[test]
    fn account_route_never_uses_sea() {
        for r in Region::ALL {
            assert_ne!(r.account_route(), RegionalRoute::Sea, "{r}");
        }
        assert_eq!(Region::Oce.account_route(), RegionalRoute::Asia);
        assert_eq!(Region::Euw.account_route(), RegionalRoute::Europe);
    }

    #[test]
    fn region_from_str() {
        assert_eq!("euw".parse::<Region>(), Ok(Region::Euw));
        assert_eq!(" EUN1 ".parse::<Region>(), Ok(Region::Eune));
        assert_eq!("kr".parse::<Region>(), Ok(Region::Kr));
        assert!("xx".parse::<Region>().is_err());
    }

    #[test]
    fn riot_id_parse() {
        let id = RiotId::parse("  Hide on bush#KR1 ").unwrap();
        assert_eq!(id.game_name, "Hide on bush");
        assert_eq!(id.tag_line, "KR1");
        assert_eq!(id.to_string(), "Hide on bush#KR1");

        let cyr = RiotId::parse("Ёжик в тумане#RU1").unwrap();
        assert_eq!(cyr.game_name, "Ёжик в тумане");

        assert!(RiotId::parse("NoTag").is_err());
        assert!(RiotId::parse("#TAG").is_err());
        assert!(RiotId::parse("Name#").is_err());
        assert!(RiotId::parse("Name#A#B").is_err());
        assert!(RiotId::parse("Name#TOOLONG").is_err());
    }
}
