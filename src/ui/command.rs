//! Командная строка (`:` или Ctrl+K): «войти dr4g», «пароль benjm», «обновить все»,
//! «тег smurf», «сортировка имя». По мере ввода показываются подсказки; Tab дописывает
//! выбранную, Enter выполняет.
//!
//! Команды понимаются по-русски и по-английски, и даже если набраны не в той раскладке:
//! «djqnb» — это «войти», набранное латиницей. Аккаунт ищется по части Riot ID.
//!
//! Только чистые функции — без egui, их легко проверить тестами.

use crate::storage::AccountId;
use crate::ui::account_list::SortOrder;
use crate::ui::keymap::{Action, Screen};

/// Сколько подсказок показывать.
pub const MAX_SUGGESTIONS: usize = 7;

/// Аккаунт, к которому можно обратиться по имени.
#[derive(Debug, Clone)]
pub struct Target {
    pub id: AccountId,
    /// «Имя#TAG».
    pub name: String,
}

/// Что выполнить: действие и аккаунт (`None` — выбранный курсором).
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub action: Action,
    pub account: Option<AccountId>,
}

impl Invocation {
    pub fn new(action: Action) -> Self {
        Self { action, account: None }
    }

    pub fn on(action: Action, account: AccountId) -> Self {
        Self {
            action,
            account: Some(account),
        }
    }
}

/// Одна подсказка под командной строкой.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// Как показать: «войти dr4gm4shroom#1933».
    pub label: String,
    /// Пояснение серым справа.
    pub hint: &'static str,
    /// Что подставить в строку по Tab.
    pub completion: String,
    /// Что выполнить по Enter; `None` — команду надо дописать (Enter дописывает, как Tab).
    pub run: Option<Invocation>,
}

/// Чего ждёт команда после себя.
#[derive(Clone, Copy, PartialEq)]
enum Arg {
    None,
    Account,
    /// Аккаунт или «все».
    AccountOrAll,
    Tag,
    Sort,
}

#[derive(Clone, Copy)]
enum Kind {
    Login,
    Password,
    UserName,
    RiotId,
    Refresh,
    History,
    Games,
    OpGg,
    Edit,
    Delete,
    Add,
    Tag,
    Sort,
    Go(Screen),
    Help,
}

struct Spec {
    kind: Kind,
    /// Первое имя — основное (его показываем), остальные — синонимы.
    names: &'static [&'static str],
    hint: &'static str,
}

const COMMANDS: &[Spec] = &[
    Spec { kind: Kind::Login, names: &["войти", "login", "enter"], hint: "логин в буфер и открыть Riot Client" },
    Spec { kind: Kind::Password, names: &["пароль", "password", "pass"], hint: "скопировать пароль" },
    Spec { kind: Kind::UserName, names: &["логин", "user"], hint: "скопировать логин" },
    Spec { kind: Kind::RiotId, names: &["ник", "riotid", "nick"], hint: "скопировать Riot ID" },
    Spec { kind: Kind::Refresh, names: &["обновить", "refresh", "update"], hint: "обновить ранги" },
    Spec { kind: Kind::History, names: &["история", "history"], hint: "история ранга" },
    Spec { kind: Kind::Games, names: &["игры", "games", "матчи"], hint: "все игры аккаунта (архив)" },
    Spec { kind: Kind::OpGg, names: &["opgg", "опгг"], hint: "профиль на op.gg" },
    Spec { kind: Kind::Edit, names: &["изменить", "edit"], hint: "изменить аккаунт" },
    Spec { kind: Kind::Delete, names: &["удалить", "delete"], hint: "удалить аккаунт" },
    Spec { kind: Kind::Add, names: &["добавить", "add", "new"], hint: "новый аккаунт" },
    Spec { kind: Kind::Tag, names: &["тег", "tag"], hint: "показать только аккаунты с тегом" },
    Spec { kind: Kind::Sort, names: &["сортировка", "sort"], hint: "порядок списка" },
    Spec { kind: Kind::Go(Screen::Accounts), names: &["аккаунты", "accounts"], hint: "экран аккаунтов" },
    Spec { kind: Kind::Go(Screen::Lobby), names: &["лобби", "lobby"], hint: "экран лобби" },
    Spec { kind: Kind::Go(Screen::Settings), names: &["настройки", "settings"], hint: "экран настроек" },
    Spec { kind: Kind::Help, names: &["помощь", "help", "?"], hint: "горячие клавиши" },
];

/// Сортировки: (порядок, русское слово, английское).
const SORTS: [(SortOrder, &str, &str); 4] = [
    (SortOrder::Rank, "ранг", "rank"),
    (SortOrder::Level, "уровень", "level"),
    (SortOrder::Name, "имя", "name"),
    (SortOrder::Added, "дата", "added"),
];

impl Kind {
    fn arg(self) -> Arg {
        match self {
            Self::Login | Self::Password | Self::UserName | Self::RiotId | Self::History | Self::Games
            | Self::OpGg | Self::Edit | Self::Delete => Arg::Account,
            Self::Refresh => Arg::AccountOrAll,
            Self::Tag => Arg::Tag,
            Self::Sort => Arg::Sort,
            Self::Add | Self::Go(_) | Self::Help => Arg::None,
        }
    }

    /// Действие для команды с аккаунтом.
    fn account_action(self) -> Action {
        match self {
            Self::Login => Action::Login,
            Self::Password => Action::CopyPassword,
            Self::UserName => Action::CopyLogin,
            Self::RiotId => Action::CopyRiotId,
            Self::Refresh => Action::Refresh,
            Self::History => Action::History,
            Self::Games => Action::Games,
            Self::OpGg => Action::OpGg,
            Self::Edit => Action::Edit,
            Self::Delete => Action::Delete,
            // Остальные аккаунт не принимают — сюда не попадают.
            Self::Add => Action::Add,
            Self::Tag | Self::Sort | Self::Go(_) | Self::Help => Action::Help,
        }
    }

    /// Действие для команды без аргумента.
    fn plain_action(self) -> Option<Action> {
        match self {
            Self::Add => Some(Action::Add),
            Self::Go(screen) => Some(Action::Screen(screen)),
            Self::Help => Some(Action::Help),
            _ => None,
        }
    }
}

/// Латиница ↔ кириллица по расположению клавиш (ЙЦУКЕН над QWERTY).
const LATIN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`";
const CYRILLIC: &str = "йцукенгшщзхъфывапролджэячсмитьбюё";

/// Тот же текст, набранный в другой раскладке: «djqnb» → «войти», «пароль» → «gfhjkm».
pub fn swap_layout(text: &str) -> String {
    text.chars()
        .map(|c| {
            if let Some(i) = LATIN.chars().position(|l| l == c) {
                CYRILLIC.chars().nth(i).unwrap_or(c)
            } else if let Some(i) = CYRILLIC.chars().position(|r| r == c) {
                LATIN.chars().nth(i).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// Насколько имя подходит под запрос: 0 — начинается с него, 1 — содержит,
/// `None` — не подходит. Запрос пробуем и в другой раскладке.
fn score(name: &str, query: &str) -> Option<u8> {
    let name = name.to_lowercase();
    let check = |q: &str| {
        if name.starts_with(q) {
            Some(0)
        } else if name.contains(q) {
            Some(1)
        } else {
            None
        }
    };
    if query.is_empty() {
        return Some(2);
    }
    check(query).or_else(|| check(&swap_layout(query)))
}

/// Аккаунты под запрос: лучшие совпадения первыми; при пустом запросе — выбранный первым.
fn matching<'a>(targets: &'a [Target], query: &str, selected: Option<AccountId>) -> Vec<&'a Target> {
    let mut found: Vec<(u8, usize, &Target)> = targets
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let s = score(&t.name, query)?;
            // Выбранный курсором — чуть выше остальных с той же оценкой.
            let s = s * 2 + u8::from(Some(t.id) != selected);
            Some((s, i, t))
        })
        .collect();
    found.sort_by_key(|(s, i, _)| (*s, *i));
    found.into_iter().map(|(_, _, t)| t).collect()
}

/// Команда, имя которой совпадает с `word` целиком (в любой раскладке).
fn exact(word: &str) -> Option<&'static Spec> {
    let swapped = swap_layout(word);
    COMMANDS
        .iter()
        .find(|spec| spec.names.iter().any(|n| *n == word || *n == swapped))
}

/// Подсказки для того, что сейчас набрано.
///
/// `targets` — аккаунты в порядке списка, `tags` — теги для «тег …»,
/// `selected` — аккаунт под курсором (подставляется, когда аккаунт не назван).
pub fn suggest(input: &str, targets: &[Target], tags: &[String], selected: Option<AccountId>) -> Vec<Suggestion> {
    let lower = input.trim_start().to_lowercase();
    let current = selected.and_then(|id| targets.iter().find(|t| t.id == id));
    let mut out = Vec::new();

    if lower.trim().is_empty() {
        // Самое частое — для выбранного аккаунта.
        if let Some(t) = current {
            for kind in [Kind::Login, Kind::Password, Kind::History] {
                out.push(for_account(spec(kind), t));
            }
        }
        out.push(refresh_all());
        out.push(plain(spec(Kind::Add)));
        out.push(plain(spec(Kind::Help)));
        return out;
    }

    // Первое слово и остальное: «войти dr4g» → («войти», «dr4g»).
    let (head, rest) = match lower.split_once(char::is_whitespace) {
        Some((head, rest)) => (head, Some(rest.trim_start())),
        None => (lower.as_str(), None),
    };

    if let Some(spec) = exact(head)
        && (rest.is_some() || spec.kind.arg() == Arg::None)
    {
        arguments(spec, rest.unwrap_or(""), targets, tags, selected, &mut out);
    } else if rest.is_none() {
        // Команда ещё набирается: «пар» → «пароль <выбранный>».
        let swapped = swap_layout(head);
        for spec in COMMANDS {
            if spec.names.iter().any(|n| n.starts_with(head) || n.starts_with(&swapped)) {
                out.push(match (spec.kind.arg(), current) {
                    (Arg::Account | Arg::AccountOrAll, Some(t)) => for_account(spec, t),
                    (Arg::None, _) => plain(spec),
                    _ => needs_argument(spec),
                });
            }
        }
    }

    // Весь текст как часть Riot ID — перейти к аккаунту.
    for t in matching(targets, lower.trim(), None) {
        out.push(Suggestion {
            label: format!("→ {}", t.name),
            hint: "выбрать",
            completion: t.name.clone(),
            run: Some(Invocation::on(Action::Select, t.id)),
        });
    }
    // Одинаковые подсказки (имя совпало и с командой, и с аккаунтом) — один раз.
    out.dedup_by(|a, b| a.label == b.label);
    out.truncate(MAX_SUGGESTIONS);
    out
}

/// Подсказки для уже набранной команды: аккаунты, теги или сортировки.
fn arguments(
    spec: &'static Spec,
    rest: &str,
    targets: &[Target],
    tags: &[String],
    selected: Option<AccountId>,
    out: &mut Vec<Suggestion>,
) {
    let name = spec.names[0];
    match spec.kind.arg() {
        Arg::None => out.push(plain(spec)),
        Arg::Account | Arg::AccountOrAll => {
            if spec.kind.arg() == Arg::AccountOrAll && ["все", "all"].iter().any(|w| w.starts_with(rest)) {
                out.push(refresh_all());
            }
            out.extend(matching(targets, rest, selected).into_iter().map(|t| for_account(spec, t)));
        }
        Arg::Tag => {
            let all = std::iter::once(("все".to_string(), None));
            let values = all.chain(tags.iter().map(|t| (t.clone(), Some(t.clone()))));
            for (label, tag) in values {
                if label.to_lowercase().starts_with(rest) || (tag.is_none() && "all".starts_with(rest)) {
                    out.push(Suggestion {
                        label: format!("{name} {label}"),
                        hint: spec.hint,
                        completion: format!("{name} {label}"),
                        run: Some(Invocation::new(Action::Tag(tag))),
                    });
                }
            }
        }
        Arg::Sort => {
            let swapped = swap_layout(rest);
            for (order, ru, en) in SORTS {
                if [ru, en].iter().any(|w| w.starts_with(rest) || w.starts_with(&swapped)) {
                    out.push(Suggestion {
                        label: format!("{name} {ru}"),
                        hint: order.label(),
                        completion: format!("{name} {ru}"),
                        run: Some(Invocation::new(Action::Sort(order))),
                    });
                }
            }
        }
    }
}

fn spec(kind: Kind) -> &'static Spec {
    COMMANDS
        .iter()
        .find(|s| std::mem::discriminant(&s.kind) == std::mem::discriminant(&kind))
        .expect("каждый Kind есть в COMMANDS")
}

fn for_account(spec: &Spec, target: &Target) -> Suggestion {
    let text = format!("{} {}", spec.names[0], target.name);
    Suggestion {
        label: text.clone(),
        hint: spec.hint,
        completion: text,
        run: Some(Invocation::on(spec.kind.account_action(), target.id)),
    }
}

fn plain(spec: &Spec) -> Suggestion {
    Suggestion {
        label: spec.names[0].to_string(),
        hint: spec.hint,
        completion: spec.names[0].to_string(),
        run: spec.kind.plain_action().map(Invocation::new),
    }
}

fn needs_argument(spec: &Spec) -> Suggestion {
    Suggestion {
        label: format!("{} …", spec.names[0]),
        hint: spec.hint,
        completion: format!("{} ", spec.names[0]),
        run: None,
    }
}

fn refresh_all() -> Suggestion {
    Suggestion {
        label: "обновить все".to_string(),
        hint: "обновить ранги всех аккаунтов",
        completion: "обновить все".to_string(),
        run: Some(Invocation::new(Action::RefreshAll)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets() -> Vec<Target> {
        ["benjm netanyahu#01irl", "ДемоСмурф#KR1", "dr4gm4shroom#1933", "Cäps#EUW"]
            .iter()
            .enumerate()
            .map(|(i, name)| Target {
                id: i as AccountId + 1,
                name: name.to_string(),
            })
            .collect()
    }

    fn first(input: &str) -> Option<Invocation> {
        let tags = vec!["main".to_string(), "smurf".to_string()];
        suggest(input, &targets(), &tags, Some(3)).into_iter().next().and_then(|s| s.run)
    }

    #[test]
    fn layout_swap_both_ways() {
        assert_eq!(swap_layout("djqnb"), "войти");
        assert_eq!(swap_layout("пароль"), "gfhjkm");
        assert_eq!(swap_layout("dr4g"), "вк4п");
    }

    #[test]
    fn command_with_account() {
        assert_eq!(first("войти dr4"), Some(Invocation::on(Action::Login, 3)));
        assert_eq!(first("пароль benjm"), Some(Invocation::on(Action::CopyPassword, 1)));
        // Не та раскладка: и команда, и ник.
        assert_eq!(first("djqnb dr4"), Some(Invocation::on(Action::Login, 3)));
        assert_eq!(first("login демо"), Some(Invocation::on(Action::Login, 2)));
        // Часть ника из середины.
        assert_eq!(first("история shroom"), Some(Invocation::on(Action::History, 3)));
        assert_eq!(first("игры benjm"), Some(Invocation::on(Action::Games, 1)));
        // Без ника — выбранный курсором (id 3) первым.
        assert_eq!(first("войти "), Some(Invocation::on(Action::Login, 3)));
    }

    #[test]
    fn partial_command_uses_selected_account() {
        let list = suggest("пар", &targets(), &[], Some(3));
        assert_eq!(list[0].label, "пароль dr4gm4shroom#1933");
        assert_eq!(list[0].run, Some(Invocation::on(Action::CopyPassword, 3)));
        // Сортировке нужно слово — Enter её только дописывает.
        let sort = suggest("сорт", &targets(), &[], Some(3));
        assert_eq!(sort[0].completion, "сортировка ");
        assert_eq!(sort[0].run, None);
    }

    #[test]
    fn refresh_tags_sorts_and_screens() {
        assert_eq!(first("обновить все"), Some(Invocation::new(Action::RefreshAll)));
        assert_eq!(first("обновить "), Some(Invocation::new(Action::RefreshAll)));
        assert_eq!(first("обновить ben"), Some(Invocation::on(Action::Refresh, 1)));
        assert_eq!(first("тег sm"), Some(Invocation::new(Action::Tag(Some("smurf".into())))));
        assert_eq!(first("тег все"), Some(Invocation::new(Action::Tag(None))));
        assert_eq!(first("сортировка им"), Some(Invocation::new(Action::Sort(SortOrder::Name))));
        assert_eq!(first("sort level"), Some(Invocation::new(Action::Sort(SortOrder::Level))));
        assert_eq!(first("лобби"), Some(Invocation::new(Action::Screen(Screen::Lobby))));
        assert_eq!(first("help"), Some(Invocation::new(Action::Help)));
    }

    #[test]
    fn plain_text_jumps_to_account() {
        assert_eq!(first("benjm"), Some(Invocation::on(Action::Select, 1)));
        assert_eq!(first("cäps"), Some(Invocation::on(Action::Select, 4)));
        assert!(suggest("нет такого", &targets(), &[], None).is_empty());
    }

    #[test]
    fn empty_input_offers_common_commands() {
        let list = suggest("", &targets(), &[], Some(3));
        assert_eq!(list[0].label, "войти dr4gm4shroom#1933");
        assert!(list.iter().any(|s| s.run == Some(Invocation::new(Action::RefreshAll))));
        assert!(list.len() <= MAX_SUGGESTIONS);
        // Без выбранного аккаунта — только общие команды.
        assert!(suggest("", &targets(), &[], None).iter().all(|s| s.run.as_ref().is_none_or(|r| r.account.is_none())));
    }
}
