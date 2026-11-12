//! Поиск, фильтр по тегам и сортировка списка аккаунтов.
//!
//! Чистые функции без egui: так их легко проверить тестами.

use crate::riot::{Division, Tier};
use crate::storage::Account;

/// Теги из ТЗ: в форме и в фильтре показываются всегда.
pub const STANDARD_TAGS: [&str; 3] = ["main", "smurf", "other"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    /// Сначала самый высокий ранг Solo/Duo (при равенстве — Flex).
    #[default]
    Rank,
    Level,
    Name,
    /// В порядке добавления.
    Added,
}

impl SortOrder {
    pub const ALL: [Self; 4] = [Self::Rank, Self::Level, Self::Name, Self::Added];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rank => "По рангу",
            Self::Level => "По уровню",
            Self::Name => "По имени",
            Self::Added => "По дате добавления",
        }
    }

    /// Короткое имя для сохранения между запусками (не меняется вместе с текстом в UI).
    pub fn key(self) -> &'static str {
        match self {
            Self::Rank => "rank",
            Self::Level => "level",
            Self::Name => "name",
            Self::Added => "added",
        }
    }

    /// Обратно из [`Self::key`]; неизвестное значение → `None`.
    pub fn from_key(key: &str) -> Option<Self> {
        // `into_iter().find(..)` — первый элемент, для которого замыкание вернуло true.
        Self::ALL.into_iter().find(|s| s.key() == key)
    }
}

/// Что сейчас выбрано над списком.
#[derive(Debug, Default)]
pub struct ListView {
    pub search: String,
    /// `None` — все теги.
    pub tag: Option<String>,
    pub sort: SortOrder,
}

impl ListView {
    /// Включены поиск или фильтр (тогда показываем «Показано N из M»).
    pub fn is_filtered(&self) -> bool {
        !self.search.trim().is_empty() || self.tag.is_some()
    }

    /// Аккаунты, которые надо показать, в нужном порядке.
    /// Возвращает ссылки (`&'a Account`) — сами аккаунты не копируются.
    /// `'a` связывает время жизни результата со списком `accounts`.
    pub fn apply<'a>(&self, accounts: &'a [Account]) -> Vec<&'a Account> {
        let query = self.search.trim().to_lowercase();
        let mut list: Vec<&Account> = accounts
            .iter()
            .filter(|a| matches_search(a, &query) && self.matches_tag(a))
            .collect();

        // sort_by — устойчивая сортировка: у равных элементов сохраняется исходный порядок.
        // cmp(b, a) вместо cmp(a, b) — по убыванию. then_with — «при равенстве сравнить ещё так».
        match self.sort {
            SortOrder::Rank => list.sort_by(|a, b| {
                rank_key(b)
                    .cmp(&rank_key(a))
                    .then_with(|| name_key(a).cmp(&name_key(b)))
            }),
            SortOrder::Level => list.sort_by(|a, b| {
                level(b).cmp(&level(a)).then_with(|| name_key(a).cmp(&name_key(b)))
            }),
            // sort_by_cached_key считает ключ один раз на элемент (to_lowercase не бесплатный).
            SortOrder::Name => list.sort_by_cached_key(|a| name_key(a)),
            SortOrder::Added => list.sort_by_key(|a| (a.created_at, a.id)),
        }
        list
    }

    fn matches_tag(&self, account: &Account) -> bool {
        match &self.tag {
            None => true,
            Some(tag) => account.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)),
        }
    }
}

/// Все теги для фильтра: стандартные и свои теги аккаунтов, без повторов.
pub fn all_tags(accounts: &[Account]) -> Vec<String> {
    let mut tags: Vec<String> = STANDARD_TAGS.iter().map(|t| t.to_string()).collect();
    for tag in accounts.iter().flat_map(|a| &a.tags) {
        if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            tags.push(tag.clone());
        }
    }
    tags
}

/// Поиск по Riot ID («Имя#TAG»), заметке и региону.
fn matches_search(account: &Account, query: &str) -> bool {
    query.is_empty()
        || account.riot_id.to_string().to_lowercase().contains(query)
        || account.note.to_lowercase().contains(query)
        || account.region.label().eq_ignore_ascii_case(query)
}

type QueueKey = Option<(Tier, Division, i32)>;

/// Ранг для сортировки: (Solo/Duo, Flex, есть ли данные). `None` меньше любого `Some`,
/// а `false` меньше `true`, поэтому по убыванию в конце оказываются сначала аккаунты
/// без ранга, а за ними — те, по которым данных ещё нет.
fn rank_key(account: &Account) -> (QueueKey, QueueKey, bool) {
    match &account.cache {
        Some(cache) => (
            cache.ranked.solo.as_ref().map(|s| s.sort_key()),
            cache.ranked.flex.as_ref().map(|s| s.sort_key()),
            true,
        ),
        None => (None, None, false),
    }
}

fn level(account: &Account) -> Option<i64> {
    account.cache.as_ref().map(|c| c.summoner_level)
}

fn name_key(account: &Account) -> String {
    account.riot_id.to_string().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::{RankedQueues, RankedStats, Region, RiotId};
    use crate::storage::CachedProfile;

    #[test]
    fn sort_order_keys_round_trip() {
        for sort in SortOrder::ALL {
            assert_eq!(SortOrder::from_key(sort.key()), Some(sort));
        }
        assert_eq!(SortOrder::from_key("что-то старое"), None);
    }

    fn account(id: i64, name: &str, solo: Option<(Tier, Division, i32)>, level: i64) -> Account {
        Account {
            id,
            riot_id: RiotId::parse(name).unwrap(),
            region: Region::Ru,
            puuid: None,
            lcu_puuid: None,
            note: String::new(),
            tags: vec![],
            secret_ref: String::new(),
            has_login: false,
            has_password: false,
            created_at: id,
            cache: Some(CachedProfile {
                profile_icon_id: 1,
                summoner_level: level,
                ranked: RankedQueues {
                    solo: solo.map(|(tier, division, lp)| RankedStats {
                        tier,
                        division,
                        lp,
                        wins: 1,
                        losses: Some(1),
                    }),
                    flex: None,
                },
                matches: None,
                fetched_at: 0,
            }),
            history: vec![],
            archived_games: 0,
        }
    }

    fn names(list: &[&Account]) -> Vec<String> {
        list.iter().map(|a| a.riot_id.game_name.clone()).collect()
    }

    #[test]
    fn sorts_by_rank_then_unranked_then_no_data() {
        let mut no_data = account(5, "NoData#RU1", None, 1);
        no_data.cache = None;
        let accounts = vec![
            account(1, "Gold#RU1", Some((Tier::Gold, Division::Two, 10)), 100),
            no_data,
            account(2, "Plat#RU1", Some((Tier::Platinum, Division::Four, 0)), 50),
            account(3, "Unranked#RU1", None, 300),
            account(4, "GoldHigh#RU1", Some((Tier::Gold, Division::Two, 80)), 20),
        ];
        let view = ListView::default();
        assert_eq!(
            names(&view.apply(&accounts)),
            ["Plat", "GoldHigh", "Gold", "Unranked", "NoData"]
        );

        let by_level = ListView { sort: SortOrder::Level, ..Default::default() };
        assert_eq!(
            names(&by_level.apply(&accounts)),
            ["Unranked", "Gold", "Plat", "GoldHigh", "NoData"]
        );

        let by_name = ListView { sort: SortOrder::Name, ..Default::default() };
        assert_eq!(
            names(&by_name.apply(&accounts)),
            ["Gold", "GoldHigh", "NoData", "Plat", "Unranked"]
        );
    }

    #[test]
    fn search_and_tag_filter() {
        let mut main = account(1, "Шрек#RU1", None, 1);
        main.tags = vec!["main".into()];
        let mut smurf = account(2, "Осёл#RU2", None, 1);
        smurf.tags = vec!["Smurf".into(), "aram".into()];
        smurf.note = "Для ARAM".into();
        let accounts = vec![main, smurf];

        let search = |text: &str| ListView { search: text.into(), ..Default::default() };
        assert_eq!(names(&search("шрек").apply(&accounts)), ["Шрек"], "без учёта регистра, кириллица");
        assert_eq!(names(&search("#ru2").apply(&accounts)), ["Осёл"], "по тегу Riot ID");
        assert_eq!(names(&search("aram").apply(&accounts)), ["Осёл"], "по заметке");
        assert_eq!(search("ru").apply(&accounts).len(), 2, "по региону");
        assert!(search("нет такого").apply(&accounts).is_empty());

        let tag = ListView { tag: Some("smurf".into()), ..Default::default() };
        assert_eq!(names(&tag.apply(&accounts)), ["Осёл"]);
        assert!(tag.is_filtered());

        assert_eq!(all_tags(&accounts), ["main", "smurf", "other", "aram"]);
    }
}
