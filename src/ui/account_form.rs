//! Форма «Добавить аккаунт» / «Изменить аккаунт» — модальное окно поверх списка.
//!
//! Форма только собирает данные и превращает их в [`Command`]. Проверку Riot ID
//! (через клиент League или Riot API) и сохранение делает фоновый поток, а ответ
//! (`AccountAdded`, `AddFailed`...) окно передаёт обратно в форму.
//!
//! Безопасность: пароль вводится в замаскированное поле (`TextEdit::password`).
//! Сохранённые логин и пароль в форму НЕ загружаются: при редактировании поля пустые,
//! а пустое поле значит «не менять». Так секрет из хранилища вообще не попадает в окно.

use eframe::egui::{self, RichText};

use crate::error::AppError;
use crate::messages::Command;
use crate::riot::{Region, RiotId};
use crate::storage::{Account, AccountId, AccountInput, Credentials, SecretChange};
use crate::ui::account_list::STANDARD_TAGS;
use crate::ui::theme::{self, Weight};
use crate::ui::widgets::{self, Button};

const FORM_WIDTH: f32 = 420.0;
/// Отступ между полями формы.
const FIELD_GAP: f32 = 10.0;

enum Mode {
    Add,
    /// `had_*` — были ли логин и пароль сохранены до редактирования.
    Edit {
        id: AccountId,
        had_login: bool,
        had_password: bool,
    },
}

/// Что окно должно сделать после кадра с формой.
pub enum FormAction {
    /// Ничего, форма остаётся открытой.
    None,
    /// Закрыть без сохранения (кнопка «Отмена» или Esc).
    Close,
    /// Отправить команду в фоновый поток и ждать ответа.
    Submit(Command),
}

pub struct AccountForm {
    mode: Mode,
    riot_id: String,
    region: Region,
    /// Тег и выбран ли он. Вектор пар, а не HashMap: так сохраняется порядок.
    tags: Vec<(String, bool)>,
    note: String,
    login: String,
    password: String,
    clear_login: bool,
    clear_password: bool,
    /// Команда отправлена, ждём ответа фонового потока.
    busy: bool,
    error: Option<String>,
    /// Поставить курсор в поле Riot ID при первом показе.
    focus_pending: bool,
}

impl AccountForm {
    /// Пустая форма добавления.
    pub fn add(region: Region) -> Self {
        Self {
            mode: Mode::Add,
            riot_id: String::new(),
            region,
            tags: tag_list(&[]),
            note: String::new(),
            login: String::new(),
            password: String::new(),
            clear_login: false,
            clear_password: false,
            busy: false,
            error: None,
            focus_pending: true,
        }
    }

    /// Форма редактирования: обычные поля заполнены, логин и пароль — нет.
    pub fn edit(account: &Account) -> Self {
        Self {
            mode: Mode::Edit {
                id: account.id,
                had_login: account.has_login,
                had_password: account.has_password,
            },
            riot_id: account.riot_id.to_string(),
            region: account.region,
            tags: tag_list(&account.tags),
            note: account.note.clone(),
            // `..Self::add(...)` — остальные поля взять из пустой формы.
            ..Self::add(account.region)
        }
    }

    pub fn is_add(&self) -> bool {
        matches!(self.mode, Mode::Add)
    }

    /// Какой аккаунт редактируется (`None` — форма добавления).
    pub fn editing(&self) -> Option<AccountId> {
        match self.mode {
            Mode::Edit { id, .. } => Some(id),
            Mode::Add => None,
        }
    }

    /// Фоновый поток ответил ошибкой: показываем её в форме, данные не теряем.
    pub fn failed(&mut self, error: &AppError) {
        self.busy = false;
        self.error = Some(error.to_string());
    }

    /// Рисует форму. `client` — аккаунт, открытый в клиенте League (для кнопки «Из клиента»).
    pub fn show(&mut self, ctx: &egui::Context, client: Option<(Region, RiotId)>) -> FormAction {
        let modal = egui::Modal::new(egui::Id::unique("account_form"))
            .frame(theme::dialog_frame())
            .show(ctx, |ui| self.contents(ui, client));

        // Клик мимо окна форму НЕ закрывает (легко промахнуться и потерять введённое),
        // а Esc закрывает. `&&` вычисляется слева направо и останавливается на первом false.
        if !self.busy && !modal.backdrop_response.clicked() && modal.should_close() {
            return FormAction::Close;
        }
        modal.inner
    }

    fn contents(&mut self, ui: &mut egui::Ui, client: Option<(Region, RiotId)>) -> FormAction {
        ui.set_width(FORM_WIDTH);
        let (title, subtitle) = if self.is_add() {
            ("Новый аккаунт", "Riot ID проверяется через клиент League или Riot API")
        } else {
            ("Изменить аккаунт", "Пустые поля логина и пароля — оставить как есть")
        };
        ui.label(RichText::new(title).size(16.0).semibold().color(theme::TEXT));
        ui.label(RichText::new(subtitle).size(11.5).color(theme::TEXT_MUTED));
        ui.add_space(14.0);

        let mut enter_pressed = false;
        // Пока ждём ответа, поля заблокированы: add_enabled_ui выключает всё внутри.
        ui.add_enabled_ui(!self.busy, |ui| {
            enter_pressed |= self.account_fields(ui, client);
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(10.0);
            enter_pressed |= self.credential_fields(ui);
        });

        if let Some(error) = &self.error {
            ui.add_space(10.0);
            ui.add(egui::Label::new(RichText::new(error).size(12.0).color(theme::RED_TEXT)).wrap());
        }

        ui.add_space(18.0);
        let mut action = FormAction::None;
        // Кнопки справа: первая добавленная («Добавить») — самая правая.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let submit_text = if self.is_add() { "Добавить" } else { "Сохранить" };
            let can_submit = !self.busy && !self.riot_id.trim().is_empty();
            let submit = ui.add_enabled(can_submit, Button::primary(submit_text).min_width(110.0));
            // `if A && let Some(x) = ...` — условие и разбор Option в одной строке
            // (let-цепочки, Rust 2024).
            if (submit.clicked() || (enter_pressed && can_submit))
                && let Some(command) = self.submit()
            {
                action = FormAction::Submit(command);
            }
            if ui.add_enabled(!self.busy, Button::outline("Отмена")).clicked() {
                action = FormAction::Close;
            }
            if self.busy {
                let text = if self.is_add() { "Проверяю Riot ID…" } else { "Сохраняю…" };
                ui.label(RichText::new(text).size(12.0).color(theme::TEXT_MUTED));
                widgets::spinner(ui, 14.0);
            }
        });
        action
    }

    /// Riot ID, регион, теги, заметка. Возвращает true, если в поле нажали Enter.
    fn account_fields(&mut self, ui: &mut egui::Ui, client: Option<(Region, RiotId)>) -> bool {
        let mut enter_pressed = false;

        ui.horizontal(|ui| {
            ui.label(theme::caps("Riot ID"));
            // Подставить аккаунт, открытый в клиенте League (только при добавлении).
            if let (true, Some((region, riot_id))) = (self.is_add(), client) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let button = ui
                        .add(Button::outline(format!("Из клиента: {riot_id}")).small())
                        .on_hover_text("Подставить аккаунт, в который выполнен вход в клиенте League");
                    if button.clicked() {
                        self.riot_id = riot_id.to_string();
                        self.region = region;
                    }
                });
            }
        });
        let field = ui.add(
            egui::TextEdit::singleline(&mut self.riot_id)
                .hint_text("Имя#TAG")
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut self.focus_pending) {
            field.request_focus();
        }
        enter_pressed |= submitted(ui, &field);
        ui.add_space(FIELD_GAP);

        ui.label(theme::caps("Регион"));
        egui::ComboBox::from_id_salt("account_form_region")
            .width(ui.available_width())
            .height(320.0)
            .selected_text(region_text(self.region))
            .show_ui(ui, |ui| {
                for region in Region::ALL {
                    ui.selectable_value(&mut self.region, region, region_text(region));
                }
            });
        ui.add_space(FIELD_GAP);

        ui.label(theme::caps("Теги"));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // `(tag, selected)` — разбираем пару; `selected` — изменяемая ссылка на bool.
            for (tag, selected) in &mut self.tags {
                widgets::toggle_chip(ui, selected, tag.as_str());
            }
        });
        ui.add_space(FIELD_GAP);

        ui.label(theme::caps("Заметка"));
        ui.add(
            egui::TextEdit::multiline(&mut self.note)
                .hint_text("Например: для ARAM, ранг на конец сезона…")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        enter_pressed
    }

    /// Логин и пароль. Возвращает true, если в поле нажали Enter.
    fn credential_fields(&mut self, ui: &mut egui::Ui) -> bool {
        let (had_login, had_password) = match self.mode {
            Mode::Edit {
                had_login,
                had_password,
                ..
            } => (had_login, had_password),
            Mode::Add => (false, false),
        };

        ui.label(RichText::new("Данные для входа").size(13.0).semibold().color(theme::TEXT));
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Необязательно. Хранятся только в системном хранилище паролей Windows, \
                     не в файлах приложения. На строке аккаунта появятся кнопки «Войти» и копирования.",
                )
                .size(11.5)
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );
        ui.add_space(10.0);

        let mut enter_pressed = false;
        ui.label(theme::caps("Логин"));
        enter_pressed |= secret_field(ui, &mut self.login, &mut self.clear_login, had_login, false);
        ui.add_space(FIELD_GAP);
        ui.label(theme::caps("Пароль"));
        enter_pressed |= secret_field(ui, &mut self.password, &mut self.clear_password, had_password, true);
        enter_pressed
    }

    /// Проверяет поля и собирает команду. `None` — ошибка, она показана в форме.
    fn submit(&mut self) -> Option<Command> {
        let riot_id = match RiotId::parse(&self.riot_id) {
            Ok(riot_id) => riot_id,
            Err(error) => {
                self.error = Some(error.to_string());
                return None;
            }
        };
        let input = AccountInput {
            riot_id,
            region: self.region,
            note: self.note.trim().to_string(),
            // filter + map: оставить выбранные теги и взять их названия.
            tags: self
                .tags
                .iter()
                .filter(|(_, selected)| *selected)
                .map(|(tag, _)| tag.clone())
                .collect(),
        };

        self.busy = true;
        self.error = None;
        Some(match self.mode {
            Mode::Add => Command::AddAccount {
                input,
                credentials: Credentials::new(
                    Some(self.login.clone()),
                    Some(self.password.clone()),
                ),
            },
            Mode::Edit { id, .. } => Command::UpdateAccount {
                id,
                input,
                login: secret_change(self.login.trim(), self.clear_login),
                // Пароль не обрезаем: пробелы по краям могут быть его частью.
                password: secret_change(&self.password, self.clear_password),
            },
        })
    }
}

/// Поле логина или пароля. Если значение уже сохранено, рядом галочка «стереть».
fn secret_field(
    ui: &mut egui::Ui,
    value: &mut String,
    clear: &mut bool,
    saved: bool,
    is_password: bool,
) -> bool {
    let mut enter_pressed = false;
    ui.horizontal(|ui| {
        let hint = match (saved, *clear) {
            (true, true) => "будет удалён",
            (true, false) => "сохранён — пусто = не менять",
            (false, _) => "",
        };
        // Место справа под галочку «стереть», если значение уже сохранено.
        let width = if saved { ui.available_width() - 90.0 } else { ui.available_width() };
        let field = ui.add_enabled(
            !*clear,
            egui::TextEdit::singleline(value)
                .password(is_password) // точки вместо символов
                .hint_text(hint)
                .desired_width(width),
        );
        enter_pressed = submitted(ui, &field);
        if saved
            && ui
                .checkbox(clear, RichText::new("стереть").size(12.0).color(theme::TEXT_MUTED))
                .changed()
            && *clear
        {
            value.clear();
        }
    });
    enter_pressed
}

/// Пустое поле — оставить как есть; галочка «стереть» — удалить; иначе заменить.
fn secret_change(value: &str, clear: bool) -> SecretChange {
    if clear {
        SecretChange::Clear
    } else if value.is_empty() {
        SecretChange::Keep
    } else {
        SecretChange::Set(value.to_string())
    }
}

/// Enter в однострочном поле: egui снимает с него фокус, а клавиша отмечена нажатой.
fn submitted(ui: &egui::Ui, field: &egui::Response) -> bool {
    field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

fn region_text(region: Region) -> String {
    format!("{} — {}", region.label(), region.full_name())
}

/// Стандартные теги + свои теги аккаунта (например, добавленные из консоли).
fn tag_list(selected: &[String]) -> Vec<(String, bool)> {
    let is_selected = |tag: &str| selected.iter().any(|s| s.eq_ignore_ascii_case(tag));
    let mut tags: Vec<(String, bool)> = STANDARD_TAGS
        .iter()
        .map(|tag| (tag.to_string(), is_selected(tag)))
        .collect();
    for tag in selected {
        if !STANDARD_TAGS.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            tags.push((tag.clone(), true));
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_custom_tags_and_marks_selected() {
        let tags = tag_list(&["Smurf".into(), "aram".into()]);
        let names: Vec<_> = tags.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        assert_eq!(
            names,
            [("main", false), ("smurf", true), ("other", false), ("aram", true)]
        );
    }

    #[test]
    fn secret_change_rules() {
        assert!(matches!(secret_change("", false), SecretChange::Keep));
        assert!(matches!(secret_change("x", true), SecretChange::Clear));
        assert!(matches!(secret_change(" p ", false), SecretChange::Set(p) if p == " p "));
    }

    #[test]
    fn submit_builds_commands() {
        let mut form = AccountForm::add(Region::Ru);
        form.riot_id = "  Имя#RU1 ".into();
        form.login = " login ".into();
        form.password = " pass ".into();
        form.tags[1].1 = true; // smurf
        let Some(Command::AddAccount { input, credentials }) = form.submit() else {
            panic!("ожидалась команда AddAccount");
        };
        assert_eq!(input.riot_id.to_string(), "Имя#RU1");
        assert_eq!(input.tags, ["smurf"]);
        assert_eq!(credentials.login.as_deref(), Some("login"));
        assert_eq!(credentials.password.as_deref(), Some(" pass "));
        assert!(form.busy);

        let mut form = AccountForm::add(Region::Ru);
        form.riot_id = "без решётки".into();
        assert!(form.submit().is_none());
        assert!(form.error.is_some() && !form.busy);
    }
}
