//! Библиотечная часть приложения: вся логика живёт здесь.
//!
//! Её используют:
//! - `src/main.rs` — окно приложения;
//! - `examples/riot_check.rs` — консольная проверка Riot-клиента (шаг 2);
//! - тесты (`cargo test`).
//!
//! Снаружи модули доступны как `lol_account_manager::riot::RiotClient` и т.д.

pub mod app;
pub mod autostart;
pub mod backend;
pub mod clipboard;
pub mod decay;
pub mod error;
pub mod hello;
pub mod history;
pub mod images;
pub mod captcha;
pub mod launcher;
pub mod lcu;
pub mod lobby;
pub mod messages;
pub mod notify;
pub mod overview;
pub mod profile;
pub mod riot;
pub mod riot_login;
pub mod single_instance;
pub mod storage;
pub mod transfer;
pub mod tray;
pub mod ui;
pub mod update;
pub mod vault;
pub mod wallpapers;
