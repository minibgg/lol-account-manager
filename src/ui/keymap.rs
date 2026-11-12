//! Горячие клавиши: что делает каждая клавиша и список для окна помощи (F1 / ?).
//!
//! Правила раскладки:
//! - буквы без модификаторов — действия с выбранным аккаунтом, как в почте или vim:
//!   они работают, только пока не идёт ввод текста (поиск, командная строка, форма);
//! - с Ctrl — то, что работает на любом экране: командная строка, поиск, экраны;
//! - Shift — «то же, но для всех» или «в обратную сторону» (Shift+R — обновить все);
//! - Esc всегда закрывает или сбрасывает то, что открыто последним.
//!
//! Буквы работают и в русской раскладке: если символ не латинский, egui сообщает
//! физическую клавишу, поэтому «З» — это та же клавиша P.
//!
//! Чистые функции без egui-контекста — их легко проверить тестами.

use eframe::egui::{self, Key, Modifiers};

use crate::ui::account_list::SortOrder;

/// Экран приложения (вкладка в шапке).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Accounts,
    Lobby,
    Settings,
}

impl Screen {
    pub const ALL: [Self; 3] = [Self::Accounts, Self::Lobby, Self::Settings];

    pub fn label(self) -> &'static str {
        match self {
            Self::Accounts => "аккаунты",
            Self::Lobby => "лобби",
            Self::Settings => "настройки",
        }
    }

    /// Номер для Ctrl+1…3.
    pub fn number(self) -> usize {
        match self {
            Self::Accounts => 1,
            Self::Lobby => 2,
            Self::Settings => 3,
        }
    }
}

/// Что можно сделать с клавиатуры, из командной строки или щелчком по подсказке.
/// Действия с аккаунтом относятся к выбранному (или к названному в командной строке).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    // Список
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Поставить курсор на аккаунт (из командной строки: «dr4g» и Enter).
    Select,
    // Выбранный аккаунт
    /// Enter: «Войти» — логин в буфер и запустить Riot Client; второй Enter — пароль.
    Login,
    CopyLogin,
    CopyPassword,
    /// Скопировать «Имя#TAG» (не секрет).
    CopyRiotId,
    Refresh,
    History,
    /// Подробности игры: 0 — самая новая.
    Game(usize),
    /// Окно «Все игры» — архив игр аккаунта.
    Games,
    OpGg,
    Edit,
    Delete,
    // Список целиком
    RefreshAll,
    Add,
    NextTag,
    PrevTag,
    NextSort,
    Tag(Option<String>),
    Sort(SortOrder),
    // Окно
    Search,
    CommandLine,
    Help,
    Screen(Screen),
    /// Esc: сбросить поиск и фильтр.
    Escape,
}

impl Action {
    /// Нужен ли выбранный аккаунт.
    pub fn needs_account(&self) -> bool {
        matches!(
            self,
            Self::Select
                | Self::Login
                | Self::CopyLogin
                | Self::CopyPassword
                | Self::CopyRiotId
                | Self::Refresh
                | Self::History
                | Self::Game(_)
                | Self::Games
                | Self::OpGg
                | Self::Edit
                | Self::Delete
        )
    }

    /// Работает на любом экране (остальные — только на экране аккаунтов).
    pub fn is_global(&self) -> bool {
        matches!(self, Self::CommandLine | Self::Help | Self::Screen(_) | Self::RefreshAll | Self::Add)
    }
}

/// Нажатая клавиша → действие. Модификаторы сравниваются точно: R и Shift+R — разное.
pub fn for_key(key: Key, mods: Modifiers) -> Option<Action> {
    use Action as A;
    // `command` — это Ctrl на Windows и Linux, Cmd на macOS.
    let ctrl = mods.ctrl || mods.command || mods.mac_cmd;
    let shift = mods.shift;
    if mods.alt {
        return None; // Alt оставляем системе (Alt+F4 и т.п.)
    }
    Some(match (ctrl, shift, key) {
        (true, false, Key::K) => A::CommandLine,
        (true, false, Key::F) => A::Search,
        (true, false, Key::N) => A::Add,
        (true, false, Key::Num1) => A::Screen(Screen::Accounts),
        (true, false, Key::Num2) => A::Screen(Screen::Lobby),
        (true, false, Key::Num3) => A::Screen(Screen::Settings),
        (true, _, _) => return None,

        (false, true, Key::R) => A::RefreshAll,
        (false, true, Key::T) => A::PrevTag,
        (false, true, Key::G) => A::Last,
        (false, true, _) => return None,

        (false, false, key) => match key {
            Key::ArrowUp | Key::K => A::Up,
            Key::ArrowDown | Key::J => A::Down,
            Key::PageUp => A::PageUp,
            Key::PageDown => A::PageDown,
            Key::Home | Key::G => A::First,
            Key::End => A::Last,
            Key::Enter => A::Login,
            Key::L => A::CopyLogin,
            Key::P => A::CopyPassword,
            Key::C => A::CopyRiotId,
            Key::R => A::Refresh,
            Key::F5 => A::RefreshAll,
            Key::H => A::History,
            Key::Num1 => A::Game(0),
            Key::Num2 => A::Game(1),
            Key::Num3 => A::Game(2),
            Key::Num4 => A::Game(3),
            Key::Num5 => A::Game(4),
            Key::O => A::OpGg,
            Key::M => A::Games,
            Key::E => A::Edit,
            Key::Delete => A::Delete,
            Key::N => A::Add,
            Key::T => A::NextTag,
            Key::S => A::NextSort,
            Key::F1 => A::Help,
            Key::Escape => A::Escape,
            _ => return None,
        },
    })
}

/// Забирает из событий кадра нажатия, у которых есть действие, и возвращает действия
/// по порядку. Забранные нажатия виджеты уже не увидят: иначе Enter, например, нажал бы
/// ещё и кнопку в фокусе. Удержание клавиши повторяет только движение курсора — зажатый
/// Enter не запустит «Войти» десять раз. На каждом экране — свои клавиши: см. [`works_on`].
pub fn take_actions(ctx: &egui::Context, screen: Screen) -> Vec<Action> {
    ctx.input_mut(|i| {
        let mut found = Vec::new();
        i.events.retain(|event| {
            let action = match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } => for_key(*key, *modifiers).filter(|a| {
                    !*repeat || matches!(a, Action::Up | Action::Down | Action::PageUp | Action::PageDown)
                }),
                egui::Event::Text(text) => for_text(text),
                _ => None,
            };
            match action {
                Some(action) if works_on(&action, screen) => {
                    found.push(action);
                    false // забрали
                }
                _ => true,
            }
        });
        found
    })
}

/// Работает ли действие на этом экране: на «Аккаунтах» — все; на «Лобби» — общие и
/// `c` / `o` (скопировать ники, мультипоиск op.gg); в «Настройках» — только общие.
pub fn works_on(action: &Action, screen: Screen) -> bool {
    match screen {
        Screen::Accounts => true,
        Screen::Lobby => action.is_global() || matches!(action, Action::CopyRiotId | Action::OpGg),
        Screen::Settings => action.is_global(),
    }
}

/// Символы, которые удобнее ловить как текст: в разных раскладках они на разных клавишах.
pub fn for_text(text: &str) -> Option<Action> {
    match text {
        ":" => Some(Action::CommandLine),
        "/" => Some(Action::Search),
        "?" => Some(Action::Help),
        _ => None,
    }
}

/// Раздел окна помощи: заголовок и строки «клавиши — что делают».
pub struct HelpSection {
    pub title: &'static str,
    pub rows: &'static [(&'static str, &'static str)],
}

/// Всё, что показывает окно помощи. Держите рядом с [`for_key`]: тест ниже проверяет,
/// что каждая клавиша отсюда действительно что-то делает.
pub const HELP: &[HelpSection] = &[
    HelpSection {
        title: "список",
        rows: &[
            ("↑ ↓  j k", "выбрать аккаунт"),
            ("PgUp PgDn", "на 5 строк"),
            ("Home End  g G", "в начало / в конец"),
            ("/  Ctrl+F", "поиск по нику, заметке, региону"),
            ("t  Shift+T", "следующий / предыдущий тег"),
            ("s", "сортировка: ранг → уровень → имя → дата"),
            ("Esc", "сбросить поиск и тег"),
        ],
    },
    HelpSection {
        title: "аккаунт",
        rows: &[
            ("Enter", "войти: логин в буфер и открыть Riot Client; ещё Enter — пароль"),
            ("l", "скопировать логин"),
            ("p", "скопировать пароль"),
            ("c", "скопировать Riot ID"),
            ("r", "обновить ранги"),
            ("h", "история ранга"),
            ("1 … 5", "подробности последних игр"),
            ("m", "все игры аккаунта — архив без ограничений"),
            ("o", "профиль на op.gg"),
            ("e", "изменить"),
            ("Delete", "удалить (спросит подтверждение)"),
        ],
    },
    HelpSection {
        title: "лобби",
        rows: &[("c", "скопировать ники всех игроков"), ("o", "мультипоиск op.gg")],
    },
    HelpSection {
        title: "везде",
        rows: &[
            (":  Ctrl+K", "командная строка: «войти dr4g», «пароль benjm», «обновить все»"),
            ("n  Ctrl+N", "добавить аккаунт"),
            ("Shift+R  F5", "обновить все аккаунты"),
            ("Ctrl+1 2 3", "аккаунты / лобби / настройки"),
            ("?  F1", "эта справка"),
            ("Esc", "закрыть окно или командную строку"),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers::NONE;

    #[test]
    fn letters_and_modifiers_are_exact() {
        assert_eq!(for_key(Key::R, NONE), Some(Action::Refresh));
        assert_eq!(for_key(Key::R, Modifiers::SHIFT), Some(Action::RefreshAll));
        assert_eq!(for_key(Key::K, NONE), Some(Action::Up));
        assert_eq!(for_key(Key::K, Modifiers::CTRL), Some(Action::CommandLine));
        assert_eq!(for_key(Key::Num2, NONE), Some(Action::Game(1)));
        assert_eq!(for_key(Key::Num2, Modifiers::CTRL), Some(Action::Screen(Screen::Lobby)));
        assert_eq!(for_key(Key::Enter, NONE), Some(Action::Login));
        // Чужие сочетания не перехватываем: Ctrl+C / Ctrl+V — копирование и вставка.
        assert_eq!(for_key(Key::C, Modifiers::CTRL), None);
        assert_eq!(for_key(Key::V, Modifiers::CTRL), None);
        assert_eq!(for_key(Key::F4, Modifiers::ALT), None);
        assert_eq!(for_key(Key::P, Modifiers::SHIFT), None);
    }

    #[test]
    fn symbols_come_as_text() {
        assert_eq!(for_text(":"), Some(Action::CommandLine));
        assert_eq!(for_text("/"), Some(Action::Search));
        assert_eq!(for_text("?"), Some(Action::Help));
        assert_eq!(for_text("j"), None);
    }

    #[test]
    fn help_lists_only_working_keys() {
        // Однобуквенные клавиши из справки должны что-то делать.
        let single_letters = HELP
            .iter()
            .flat_map(|s| s.rows)
            .flat_map(|(keys, _)| keys.split_whitespace())
            .filter(|k| k.len() == 1 && k.chars().all(|c| c.is_ascii_lowercase()));
        for letter in single_letters {
            let key = Key::from_name(&letter.to_uppercase()).unwrap_or_else(|| panic!("нет клавиши {letter}"));
            assert!(for_key(key, NONE).is_some(), "клавиша {letter} из справки ничего не делает");
        }
    }

    /// Один кадр egui без окна с этими событиями; возвращает (действия, сколько событий осталось).
    fn frame(events: Vec<egui::Event>, screen: Screen) -> (Vec<Action>, usize) {
        let ctx = egui::Context::default();
        let (mut actions, mut left) = (Vec::new(), 0);
        ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
            actions = take_actions(ui.ctx(), screen);
            left = ui.input(|i| i.events.len());
        })
        .drop_without_applying_deltas();
        (actions, left)
    }

    fn press(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn takes_actions_from_frame_events() {
        let events = vec![
            press(Key::ArrowDown, NONE),
            press(Key::ArrowDown, NONE), // зажата — повтор, курсор едет дальше
            press(Key::Enter, NONE),
            press(Key::Enter, NONE), // повтор Enter второй раз не входит
            egui::Event::Text(":".into()),
            press(Key::R, Modifiers::SHIFT),
            press(Key::X, NONE), // ничего не делает — остаётся виджетам
        ];
        let (actions, left) = frame(events, Screen::Accounts);
        use Action as A;
        assert_eq!(actions, [A::Down, A::Down, A::Login, A::CommandLine, A::RefreshAll]);
        assert_eq!(left, 2, "повтор Enter и X не забраны");

        // На лобби и в настройках буквы не работают, общие клавиши — работают.
        let (actions, left) = frame(vec![press(Key::L, NONE), press(Key::Num1, Modifiers::CTRL)], Screen::Settings);
        assert_eq!(actions, [A::Screen(Screen::Accounts)]);
        assert_eq!(left, 1);

        // На лобби — ещё `c` и `o` (ники и мультипоиск), но не `l`.
        let (actions, left) = frame(vec![press(Key::C, NONE), press(Key::O, NONE), press(Key::L, NONE)], Screen::Lobby);
        assert_eq!(actions, [A::CopyRiotId, A::OpGg]);
        assert_eq!(left, 1);
    }

    #[test]
    fn global_and_account_actions() {
        assert!(Action::Login.needs_account());
        assert!(!Action::RefreshAll.needs_account());
        assert!(Action::Screen(Screen::Settings).is_global());
        assert!(!Action::Down.is_global());
        assert_eq!(Screen::ALL.map(Screen::number), [1, 2, 3]);
    }
}
