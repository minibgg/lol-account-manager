//! Окно «Коллекция» (клавиша b, команда «коллекция ари»): на каком аккаунте есть чемпион
//! или образ, и что вообще есть на каждом аккаунте — чемпионы, образы, валюта, честь,
//! ограничения.
//!
//! Откуда данные: клиент League отдаёт инвентарь только аккаунта, в который в нём выполнен
//! вход. Приложение запоминает его каждый раз, когда видит аккаунт в клиенте (см.
//! `backend::inventory`), поэтому здесь — последнее увиденное, с пометкой «когда».
//!
//! Пустая строка поиска — таблица аккаунтов; набрали текст — найденные чемпионы и образы:
//! «есть на» и «нет на». Esc закрывает окно.

use std::collections::{BTreeMap, HashMap};

use eframe::egui::{self, Align, Color32, Layout, Margin, RichText, Sense, Stroke, pos2, vec2};

use crate::riot::Inventory;
use crate::storage::{Account, AccountId, unix_now};
use crate::ui::command::swap_layout;
use crate::ui::format;
use crate::ui::match_view;
use crate::ui::terminal::{self, FONT, ROW_HEIGHT, job};
use crate::ui::theme::{self, Weight, white};
use crate::wallpapers::Skin;

const WIDTH: f32 = 860.0;
/// Таблица аккаунтов: аккаунт, чемпионы, образы, синяя, оранжевая, RP, честь;
/// дальше — ограничения и давность данных.
const COLUMNS: [f32; 7] = [230.0, 70.0, 70.0, 80.0, 70.0, 60.0, 56.0];
const HEADERS: [&str; 8] = ["аккаунт", "чемп.", "образы", "синяя", "оранж.", "RP", "честь", "ограничения · данные"];
/// Больше результатов поиска не показываем — уточните запрос.
const MAX_HITS: usize = 60;

/// Что нашлось: чемпион или образ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    Champion,
    Skin,
}

/// Найденный чемпион или образ и на каких аккаунтах он есть.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub kind: HitKind,
    pub name: String,
    /// Аккаунты, на которых он есть.
    pub owners: Vec<AccountId>,
    /// Аккаунты с известным инвентарём, на которых его нет.
    pub missing: Vec<AccountId>,
}

/// Насколько название подходит под запрос: 0 — совпадает, 1 — начинается с него,
/// 2 — с него начинается одно из слов, 3 — просто содержит; `None` — не подходит.
fn score(name: &str, query: &str) -> Option<u8> {
    let name = name.to_lowercase();
    let check = |q: &str| {
        if q.is_empty() {
            None
        } else if name == q {
            Some(0)
        } else if name.starts_with(q) {
            Some(1)
        } else if name.split(|c: char| !c.is_alphanumeric()).any(|word| word.starts_with(q)) {
            Some(2)
        } else if name.contains(q) {
            Some(3)
        } else {
            None
        }
    };
    check(query).or_else(|| check(&swap_layout(query)))
}

/// Лучшая оценка среди нескольких имён.
fn best_score<'a>(names: impl IntoIterator<Item = &'a str>, query: &str) -> Option<u8> {
    names.into_iter().filter_map(|name| score(name, query)).min()
}

/// Каталог образов вкладки «Обои» (id образа → образ): из него русские названия
/// и английские — для поиска. Клиент League отдаёт названия на своём языке.
pub type Catalog<'a> = HashMap<i64, &'a Skin>;

/// Поиск по инвентарям всех аккаунтов. Чемпионы — первыми; образы подходят и по своему
/// названию, и по названию чемпиона («ари» — все образы Ари). Названия — по-русски из
/// `catalog`, если он загружен; искать можно и по-русски, и по-английски.
pub fn search(inventories: &[(AccountId, &Inventory)], catalog: &Catalog, query: &str) -> Vec<Hit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    // Название для показа и все имена для поиска (из каталога + то, что дал клиент).
    // У базового образа чемпиона id = id чемпиона × 1000 — из него имя чемпиона.
    let named = |catalog_id: i64, client_name: &str, champion: bool| -> (String, Vec<String>) {
        let mut names = vec![client_name.to_lowercase()];
        let shown = match catalog.get(&catalog_id) {
            Some(skin) => {
                names.extend(skin.names().iter().cloned());
                if champion { skin.champion.clone() } else { skin.title.clone() }
            }
            None => client_name.to_string(),
        };
        let shown = if shown.is_empty() { client_name.to_string() } else { shown };
        (shown, names)
    };
    // BTreeMap — по порядку id, чтобы результат не зависел от порядка в HashMap.
    let mut champions: BTreeMap<i32, (String, Vec<String>, Vec<AccountId>)> = BTreeMap::new();
    let mut skins: BTreeMap<i32, (String, Vec<String>, i32, Vec<AccountId>)> = BTreeMap::new();
    for (account, inventory) in inventories {
        for champion in &inventory.champion_list {
            let entry = champions.entry(champion.id).or_insert_with(|| {
                let (shown, names) = named(i64::from(champion.id) * 1000, &champion.name, true);
                (shown, names, Vec::new())
            });
            entry.2.push(*account);
        }
        for skin in &inventory.skin_list {
            let entry = skins.entry(skin.id).or_insert_with(|| {
                let (shown, names) = named(i64::from(skin.id), &skin.name, false);
                (shown, names, skin.champion_id, Vec::new())
            });
            entry.3.push(*account);
        }
    }
    let with_champions: Vec<AccountId> = inventories
        .iter()
        .filter(|(_, inv)| !inv.champion_list.is_empty())
        .map(|(id, _)| *id)
        .collect();
    let with_skins: Vec<AccountId> = inventories
        .iter()
        .filter(|(_, inv)| !inv.skin_list.is_empty())
        .map(|(id, _)| *id)
        .collect();
    let missing = |known: &[AccountId], owners: &[AccountId]| -> Vec<AccountId> {
        known.iter().filter(|id| !owners.contains(id)).copied().collect()
    };

    let mut found: Vec<(u8, HitKind, Hit)> = Vec::new();
    let mut champion_scores: HashMap<i32, u8> = HashMap::new();
    for (id, (name, names, owners)) in &champions {
        if let Some(s) = best_score(names.iter().map(String::as_str).chain([name.as_str()]), &query) {
            champion_scores.insert(*id, s);
            found.push((
                s,
                HitKind::Champion,
                Hit {
                    kind: HitKind::Champion,
                    name: name.clone(),
                    owners: owners.clone(),
                    missing: missing(&with_champions, owners),
                },
            ));
        }
    }
    for (name, names, champion_id, owners) in skins.values() {
        // Образы чемпиона, который нашёлся по началу названия, — тоже показываем.
        let by_champion = champion_scores.get(champion_id).filter(|s| **s <= 1).copied();
        let own = best_score(names.iter().map(String::as_str).chain([name.as_str()]), &query);
        if let Some(s) = own.into_iter().chain(by_champion).min() {
            found.push((
                s,
                HitKind::Skin,
                Hit {
                    kind: HitKind::Skin,
                    name: name.clone(),
                    owners: owners.clone(),
                    missing: missing(&with_skins, owners),
                },
            ));
        }
    }
    // Есть совпадения с начала слова — совпадения из середины слов («ари» в «Катарина»)
    // только мешают; если других нет — показываем их.
    if found.iter().any(|(s, _, _)| *s <= 2) {
        found.retain(|(s, _, _)| *s <= 2);
    }
    // Чемпионы раньше образов, внутри — лучшие совпадения и по алфавиту.
    found.sort_by(|a, b| {
        let kind = |k: HitKind| u8::from(k == HitKind::Skin);
        (kind(a.1), a.0, a.2.name.to_lowercase()).cmp(&(kind(b.1), b.0, b.2.name.to_lowercase()))
    });
    found.into_iter().map(|(_, _, hit)| hit).take(MAX_HITS).collect()
}

/// Открытое окно «Коллекция».
pub struct CollectionView {
    pub query: String,
    /// Поставить фокус в поле поиска в первом кадре.
    focus: bool,
}

impl CollectionView {
    pub fn new(query: String) -> Self {
        Self { query, focus: true }
    }

    /// Рисует окно. `inventories` — `None`, пока инвентари загружаются; `skins` — каталог
    /// образов «Обоев» (русские названия; пока он не загружен — названия из клиента).
    /// Возвращает true, если окно закрыли.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        accounts: &[Account],
        inventories: Option<&HashMap<AccountId, Inventory>>,
        skins: &[Skin],
    ) -> bool {
        let mut close = false;
        let names: HashMap<AccountId, String> = accounts.iter().map(|a| (a.id, a.riot_id.to_string())).collect();
        // Только существующие аккаунты, в порядке списка.
        let known: Vec<(AccountId, &Inventory)> = accounts
            .iter()
            .filter_map(|a| Some((a.id, inventories?.get(&a.id)?)))
            .collect();
        let height = (ctx.content_rect().height() - 320.0).max(ROW_HEIGHT * 4.0);
        let modal = egui::Modal::new(egui::Id::unique("collection_view"))
            .frame(theme::dialog_frame().inner_margin(Margin::symmetric(20, 16)))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing.y = 4.0;
                let font = theme::mono(FONT);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("коллекция").font(theme::mono(15.0)).color(theme::TEXT).semibold());
                    let seen = format!("данные есть по {} из {} аккаунтов", known.len(), accounts.len());
                    ui.label(RichText::new(seen).font(font.clone()).color(theme::TEXT_MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if terminal::key_button(ui, "esc", "закрыть", false).clicked() {
                            close = true;
                        }
                    });
                });
                self.search_field(ui);
                ui.add_space(6.0);
                match inventories {
                    None => {
                        ui.label(job(&[("загружаю…", theme::TEXT_HINT)], &font));
                    }
                    Some(_) if self.query.trim().is_empty() => accounts_table(ui, accounts, inventories, height),
                    Some(_) => {
                        let catalog: Catalog = skins.iter().map(|s| (s.id, s)).collect();
                        hits(ui, &search(&known, &catalog, &self.query), &names, known.len(), height);
                    }
                }
                ui.add_space(6.0);
                let note = "Клиент League показывает инвентарь только аккаунта, в который выполнен вход: \
                            данные обновляются, когда вы заходите в аккаунт.";
                ui.add(egui::Label::new(RichText::new(note).font(theme::mono(11.0)).color(theme::TEXT_HINT)).wrap());
            });
        // Esc: сначала очищает поиск, потом закрывает окно.
        if modal.should_close() {
            if self.query.is_empty() {
                close = true;
            } else {
                self.query.clear();
                self.focus = true;
            }
        }
        close
    }

    fn search_field(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("/").font(theme::mono(12.5)).color(theme::ACCENT_TEXT));
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .id(egui::Id::unique("collection_search"))
                    .font(theme::mono(12.5))
                    .text_color(theme::TEXT)
                    .frame(egui::Frame::NONE)
                    .hint_text(RichText::new("чемпион или образ: ари, звёздная защитница…").color(theme::TEXT_HINT))
                    .desired_width(420.0),
            );
            if std::mem::take(&mut self.focus) {
                response.request_focus();
            }
            terminal::focus_outline(ui, response.rect.expand2(vec2(6.0, 3.0)));
        });
    }
}

/// «12 415» (отрицательных здесь не бывает).
fn number(n: i64) -> String {
    match_view::number(n.clamp(0, i64::from(u32::MAX)) as u32)
}

/// Таблица всех аккаунтов: что на каждом есть и когда это видели.
fn accounts_table(ui: &mut egui::Ui, accounts: &[Account], inventories: Option<&HashMap<AccountId, Inventory>>, height: f32) {
    let font = theme::mono(FONT);
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let painter = ui.painter();
    let mut x = head.left();
    for (index, title) in HEADERS.iter().enumerate() {
        painter.text(pos2(x, head.center().y), egui::Align2::LEFT_CENTER, *title, font.clone(), theme::TEXT_HINT);
        x += COLUMNS.get(index).copied().unwrap_or(0.0);
    }
    painter.hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::BORDER));

    let now = unix_now();
    let (mut blue, mut orange, mut rp) = (0, 0, 0);
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .id_salt("collection_accounts")
        .max_height(height)
        .min_scrolled_height(height.min(accounts.len() as f32 * ROW_HEIGHT))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for account in accounts {
                let inventory = inventories.and_then(|map| map.get(&account.id));
                if let Some(inv) = inventory {
                    blue += inv.blue_essence;
                    orange += inv.orange_essence;
                    rp += inv.rp;
                }
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
                let painter = ui.painter_at(rect);
                if response.hovered() {
                    painter.rect_filled(rect, 0, white(7));
                }
                let mut cells: Vec<Vec<(String, Color32)>> = vec![vec![(account.riot_id.to_string(), theme::TEXT)]];
                match inventory {
                    None => cells.push(vec![(
                        "нет данных — войдите в этот аккаунт в клиенте League".into(),
                        theme::TEXT_HINT,
                    )]),
                    Some(inv) => {
                        cells.push(vec![(inv.champions.to_string(), theme::TEXT)]);
                        cells.push(vec![(inv.skins.to_string(), theme::TEXT)]);
                        cells.push(vec![(number(inv.blue_essence), theme::TEXT)]);
                        cells.push(vec![(number(inv.orange_essence), theme::AMBER_TEXT)]);
                        cells.push(vec![(number(inv.rp), theme::TEXT)]);
                        cells.push(vec![(
                            inv.honor_level.map_or("—".into(), |h| h.to_string()),
                            if inv.honor_level.is_some_and(|h| h < 2) { theme::RED_TEXT } else { theme::TEXT_MUTED },
                        )]);
                        let mut last = Vec::new();
                        for restriction in &inv.restrictions {
                            last.push((restriction.label(now), theme::RED_TEXT));
                            last.push((" · ".into(), theme::TEXT_HINT));
                        }
                        last.push((format::relative_time(inv.fetched_at), theme::TEXT_HINT));
                        cells.push(last);
                    }
                }
                let mut x = rect.left();
                for (index, parts) in cells.iter().enumerate() {
                    let parts: Vec<(&str, Color32)> = parts.iter().map(|(t, c)| (t.as_str(), *c)).collect();
                    // Пояснение «нет данных» и последняя колонка — до правого края.
                    let wide = inventory.is_none() && index == 1;
                    let width = if wide { rect.right() - x } else { COLUMNS.get(index).copied().unwrap_or(rect.right() - x) };
                    let mut cell = job(&parts, &font);
                    cell.wrap = egui::text::TextWrapping::truncate_at_width((width - 8.0).max(8.0));
                    let galley = painter.layout_job(cell);
                    painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
                    x += COLUMNS.get(index).copied().unwrap_or(0.0);
                }
            }
        });
    ui.add_space(6.0);
    let total = [
        ("всего: синяя эссенция ".to_string(), theme::TEXT_HINT),
        (number(blue), theme::TEXT),
        (" · оранжевая ".to_string(), theme::TEXT_HINT),
        (number(orange), theme::AMBER_TEXT),
        (" · RP ".to_string(), theme::TEXT_HINT),
        (number(rp), theme::TEXT),
    ];
    let parts: Vec<(&str, Color32)> = total.iter().map(|(t, c)| (t.as_str(), *c)).collect();
    ui.label(job(&parts, &font));
}

/// Результаты поиска: «чемпион  Ари   есть: main#ru1, alt#ru1   нет: smurf#ru1».
fn hits(ui: &mut egui::Ui, hits: &[Hit], names: &HashMap<AccountId, String>, with_data: usize, height: f32) {
    let font = theme::mono(FONT);
    if with_data == 0 {
        ui.label(job(
            &[("пока нет данных ни по одному аккаунту — войдите в аккаунт в клиенте League", theme::TEXT_HINT)],
            &font,
        ));
        return;
    }
    if hits.is_empty() {
        ui.label(job(&[("ничего не нашлось", theme::TEXT_MUTED)], &font));
        return;
    }
    let list = |ids: &[AccountId]| -> String {
        ids.iter().map(|id| names.get(id).cloned().unwrap_or_else(|| format!("#{id}"))).collect::<Vec<_>>().join(", ")
    };
    egui::ScrollArea::vertical()
        .id_salt("collection_hits")
        .max_height(height)
        .min_scrolled_height(height.min(hits.len() as f32 * ROW_HEIGHT))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            for hit in hits {
                let kind = match hit.kind {
                    HitKind::Champion => "чемпион",
                    HitKind::Skin => "образ  ",
                };
                let mut parts = vec![
                    (format!("{kind}  "), theme::TEXT_HINT),
                    (hit.name.clone(), theme::TEXT),
                    ("   ".into(), theme::TEXT),
                ];
                if hit.owners.is_empty() {
                    parts.push(("нет ни на одном".into(), theme::TEXT_HINT));
                } else {
                    parts.push(("есть: ".into(), theme::TEXT_HINT));
                    parts.push((list(&hit.owners), theme::GREEN_TEXT));
                }
                if !hit.missing.is_empty() {
                    parts.push(("   нет: ".into(), theme::TEXT_HINT));
                    parts.push((list(&hit.missing), theme::TEXT_MUTED));
                }
                let parts: Vec<(&str, Color32)> = parts.iter().map(|(t, c)| (t.as_str(), *c)).collect();
                let mut text = job(&parts, &font);
                text.wrap.max_width = ui.available_width();
                ui.label(text);
            }
            if hits.len() >= MAX_HITS {
                ui.label(job(&[("показаны первые результаты — уточните запрос", theme::TEXT_HINT)], &font));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riot::{OwnedChampion, OwnedSkin};

    fn inventory(champions: &[(i32, &str)], skins: &[(i32, i32, &str)]) -> Inventory {
        Inventory {
            champion_list: champions
                .iter()
                .map(|(id, name)| OwnedChampion {
                    id: *id,
                    name: (*name).into(),
                })
                .collect(),
            skin_list: skins
                .iter()
                .map(|(id, champion_id, name)| OwnedSkin {
                    id: *id,
                    champion_id: *champion_id,
                    name: (*name).into(),
                })
                .collect(),
            ..Inventory::default()
        }
    }

    #[test]
    fn finds_champions_and_their_skins_on_every_account() {
        let main = inventory(
            &[(103, "Ари"), (245, "Экко"), (21, "Мисс Фортуна"), (55, "Катарина")],
            &[(103_001, 103, "Ари-лисица"), (245_002, 245, "Экко из Звёздной защиты")],
        );
        let alt = inventory(&[(103, "Ари")], &[(103_015, 103, "Звёздная защитница Ари")]);
        let empty = Inventory::default(); // старая запись без списков — не участвует
        let all = [(1, &main), (2, &alt), (3, &empty)];

        let none = Catalog::new();
        let found = search(&all, &none, "ари");
        let seen: Vec<_> = found.iter().map(|h| (h.kind, h.name.as_str(), h.owners.clone(), h.missing.clone())).collect();
        assert_eq!(
            seen,
            [
                (HitKind::Champion, "Ари", vec![1, 2], vec![]),
                // Образы Ари: по названию чемпиона («ари» совпало целиком) и по слову в названии.
                (HitKind::Skin, "Ари-лисица", vec![1], vec![2]),
                (HitKind::Skin, "Звёздная защитница Ари", vec![2], vec![1]),
            ]
        );
        // Не та раскладка: «frr»… → «экк» не нужен; «'rrj» → «экко».
        let found = search(&all, &none, "'rrj");
        assert_eq!(found[0].name, "Экко");
        assert_eq!(found[0].missing, vec![2], "на втором аккаунте Экко нет");
        // По слову внутри названия.
        assert!(search(&all, &none, "фортуна").iter().any(|h| h.name == "Мисс Фортуна"));
        // «ари» внутри «Катарины» не показываем, пока есть совпадения с начала слова…
        assert!(!search(&all, &none, "ари").iter().any(|h| h.name == "Катарина"));
        // …а если других нет — показываем.
        assert_eq!(search(&all, &none, "тарин")[0].name, "Катарина");
        assert!(search(&all, &none, "   ").is_empty());
        assert!(search(&all, &none, "зззз").is_empty());
    }

    /// Клиент на английском: показываем русские названия из каталога «Обоев»,
    /// а искать можно на обоих языках.
    #[test]
    fn catalog_gives_russian_names_and_english_search() {
        let main = inventory(&[(103, "Ahri")], &[(103_001, 103, "Dynasty Ahri")]);
        let all = [(1, &main)];
        let base = Skin::for_test(103_000, "Ари", "Ари", &["ahri", "ари"]);
        let dynasty = Skin::for_test(103_001, "Ари Династии", "Ари", &["dynasty ahri", "ahri", "ари", "ари династии"]);
        let catalog: Catalog = [(base.id, &base), (dynasty.id, &dynasty)].into_iter().collect();
        for query in ["ари", "ahri", "fhb"] {
            let names: Vec<_> = search(&all, &catalog, query).into_iter().map(|h| h.name).collect();
            assert_eq!(names, ["Ари", "Ари Династии"], "запрос {query}");
        }
        let names: Vec<_> = search(&all, &catalog, "династ").into_iter().map(|h| h.name).collect();
        assert_eq!(names, ["Ари Династии"]);
    }

    #[test]
    fn window_draws_and_escape_clears_the_query_first() {
        use crate::riot::{Region, RiotId};
        let account = |id: AccountId, name: &str| Account {
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
            created_at: 0,
            cache: None,
            history: vec![],
            archived_games: 0,
            ranked_games: vec![],
        };
        let accounts = [account(1, "Main#RU1"), account(2, "Alt#RU1")];
        let mut map = HashMap::new();
        map.insert(1, inventory(&[(103, "Ари")], &[]));
        let ctx = egui::Context::default();
        let mut view = CollectionView::new("ари".into());
        let frame = |view: &mut CollectionView, events: Vec<egui::Event>| {
            let mut closed = false;
            ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                closed = view.show(ui.ctx(), &accounts, Some(&map), &[]);
            })
            .drop_without_applying_deltas();
            closed
        };
        let esc = || egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(!frame(&mut view, vec![]));
        assert!(!frame(&mut view, vec![esc()]), "первый Esc очищает поиск");
        assert!(view.query.is_empty());
        assert!(!frame(&mut view, vec![]), "таблица аккаунтов рисуется");
        assert!(frame(&mut view, vec![esc()]), "второй Esc закрывает окно");
    }
}
