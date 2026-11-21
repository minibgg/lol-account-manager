//! Каталог образов (skins) для вкладки «Обои».
//!
//! Данные берём из CommunityDragon — это открытая выгрузка игровых данных Riot,
//! без ключа и без лимитов. Нужны четыре файла: список чемпионов и список образов,
//! по-английски и по-русски. Из них собираем плоский список образов, по которому
//! можно искать (по имени чемпиона, имени образа или числовому id), а затем поставить
//! выбранный образ фоном профиля (см. [`crate::lcu::LcuClient::set_background_skin`]).
//!
//! Приложение сами игровые файлы не трогает: один POST к локальному клиенту —
//! и всё (как это делает проект league-bg-changer, который взят за основу).

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use tracing::debug;

use crate::error::AppResult;

/// Откуда берём данные (последняя версия, глобальная выгрузка).
const BASE: &str = "https://raw.communitydragon.org/latest/plugins/rcp-be-lol-game-data/global";

/// Один образ в списке результатов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skin {
    /// id образа — его и отправляем клиенту.
    pub id: i64,
    /// Как показать в списке: «Ари — K/DA ALL OUT Ари».
    pub name: String,
    /// По чему ищем — имена чемпиона и образа (рус. и англ.), уже в нижнем регистре.
    search: Vec<String>,
}

impl Skin {
    /// Подходит ли образ под запрос: точное совпадение по числовому id либо подстрока
    /// в любом из имён. Пустой запрос не подходит никому (список прячем).
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return false;
        }
        if q.parse::<i64>().is_ok_and(|n| n == self.id) {
            return true;
        }
        self.search.iter().any(|name| name.contains(&q))
    }
}

/// Faker's Ahri Signature: клиент принимает фоном только id 103086, но в каталоге
/// эти образы лежат под другими id. Перед отправкой подменяем (как в оригинале).
pub fn remap(skin_id: i64) -> i64 {
    match skin_id {
        103061 | 103062 | 103063 | 103087 => 103086,
        other => other,
    }
}

/// Загрузить весь каталог образов из CommunityDragon.
pub async fn load() -> AppResult<Vec<Skin>> {
    let http = reqwest::Client::builder()
        .user_agent(concat!("lol-account-manager/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()?;

    // Четыре файла: чемпионы и образы, по-английски и по-русски.
    let champs_en = fetch(&http, "default/v1/champion-summary.json").await?;
    let skins_en = fetch(&http, "default/v1/skins.json").await?;
    let champs_ru = fetch(&http, "ru_ru/v1/champion-summary.json").await?;
    let skins_ru = fetch(&http, "ru_ru/v1/skins.json").await?;

    let skins = build(champs_en, skins_en, champs_ru, skins_ru);
    debug!(count = skins.len(), "каталог образов загружен");
    Ok(skins)
}

/// Элемент с полями `id` и `name` (и чемпион, и образ выглядят так). Остальные поля
/// в файлах нам не нужны — serde их просто пропустит.
#[derive(Debug, Deserialize)]
struct NamedById {
    id: i64,
    #[serde(default)]
    name: String,
}

/// Файл CommunityDragon бывает массивом (чемпионы) или объектом id→элемент (образы).
/// `#[serde(untagged)]` — пробуем сначала массив, потом объект.
#[derive(Deserialize)]
#[serde(untagged)]
enum Listing {
    List(Vec<NamedById>),
    Map(HashMap<String, NamedById>),
}

impl Listing {
    fn into_vec(self) -> Vec<NamedById> {
        match self {
            Self::List(v) => v,
            Self::Map(m) => m.into_values().collect(),
        }
    }
}

async fn fetch(http: &reqwest::Client, path: &str) -> AppResult<Vec<NamedById>> {
    let url = format!("{BASE}/{path}");
    let listing: Listing = http.get(&url).send().await?.error_for_status()?.json().await?;
    Ok(listing.into_vec())
}

/// Собрать плоский список образов из четырёх выгрузок. Основа — английский список
/// образов; русские имена и имена чемпионов добавляем как варианты для поиска.
fn build(
    champs_en: Vec<NamedById>,
    skins_en: Vec<NamedById>,
    champs_ru: Vec<NamedById>,
    skins_ru: Vec<NamedById>,
) -> Vec<Skin> {
    let names_en = by_id(champs_en);
    let names_ru = by_id(champs_ru);
    let skin_ru = by_id(skins_ru);

    let mut skins: Vec<Skin> = skins_en
        .into_iter()
        // id -1 и прочий мусор без имени пропускаем.
        .filter(|s| s.id > 0 && !s.name.is_empty())
        .map(|skin| {
            // id образа = id чемпиона * 1000 + номер образа.
            let champ_id = skin.id / 1000;
            let champ_en = names_en.get(&champ_id);
            let name = match champ_en {
                Some(champ) if *champ != skin.name => format!("{champ} — {}", skin.name),
                _ => skin.name.clone(),
            };
            let mut search = vec![skin.name.to_lowercase()];
            add_name(&mut search, champ_en);
            add_name(&mut search, names_ru.get(&champ_id));
            add_name(&mut search, skin_ru.get(&skin.id));
            Skin { id: skin.id, name, search }
        })
        .collect();

    // Два особых образа, которых нет в обычной выгрузке (как в оригинале).
    push_manual(&mut skins, 103087, "Ари — Signature Immortalized Legend (Faker)", &["ари", "ahri", "faker", "фейкер"]);
    push_manual(&mut skins, 145071, "Кайса — Signature Immortalized Legend", &["кайса", "kaisa", "kai'sa"]);

    skins.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    skins
}

fn by_id(items: Vec<NamedById>) -> HashMap<i64, String> {
    items.into_iter().filter(|i| !i.name.is_empty()).map(|i| (i.id, i.name)).collect()
}

fn add_name(search: &mut Vec<String>, name: Option<&String>) {
    if let Some(name) = name {
        let lower = name.to_lowercase();
        if !search.contains(&lower) {
            search.push(lower);
        }
    }
}

/// Добавить образ вручную, если его ещё нет (по id).
fn push_manual(skins: &mut Vec<Skin>, id: i64, name: &str, aliases: &[&str]) {
    if skins.iter().any(|s| s.id == id) {
        return;
    }
    let mut search: Vec<String> = aliases.iter().map(|a| a.to_lowercase()).collect();
    search.push(name.to_lowercase());
    skins.push(Skin { id, name: name.to_string(), search });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(id: i64, name: &str) -> NamedById {
        NamedById { id, name: name.to_string() }
    }

    #[test]
    fn remaps_fakers_ahri() {
        assert_eq!(remap(103087), 103086);
        assert_eq!(remap(103061), 103086);
        // Обычный образ остаётся как есть.
        assert_eq!(remap(103001), 103001);
    }

    #[test]
    fn builds_and_searches() {
        let champs_en = vec![named(103, "Ahri")];
        let champs_ru = vec![named(103, "Ари")];
        let skins_en = vec![named(103000, "Ahri"), named(103001, "K/DA ALL OUT Ahri"), named(-1, "None")];
        let skins_ru = vec![named(103001, "K/DA ALL OUT Ари")];
        let skins = build(champs_en, skins_en, champs_ru, skins_ru);

        // Мусорный id -1 отброшен; остались два образа + два ручных.
        assert!(skins.iter().any(|s| s.id == 103001));
        assert!(skins.iter().any(|s| s.id == 103087), "ручной образ Faker добавлен");
        assert!(!skins.iter().any(|s| s.id == -1));

        let kda = skins.iter().find(|s| s.id == 103001).unwrap();
        // Ищем и по-английски, и по-русски, и по id.
        assert!(kda.matches("ahri"));
        assert!(kda.matches("ари"));
        assert!(kda.matches("k/da"));
        assert!(kda.matches("103001"));
        assert!(!kda.matches("garen"));
        // Пустой запрос ничего не находит.
        assert!(!kda.matches("   "));
    }

    #[test]
    fn parses_both_array_and_object() {
        let arr: Listing = serde_json::from_str(r#"[{"id":1,"name":"A"}]"#).unwrap();
        let obj: Listing = serde_json::from_str(r#"{"1":{"id":1,"name":"A"}}"#).unwrap();
        assert_eq!(arr.into_vec().len(), 1);
        assert_eq!(obj.into_vec().len(), 1);
    }
}
