//! Библиотечная часть приложения: вся логика живёт здесь.
//!
//! Её используют:
//! - `src/main.rs` — окно приложения;
//! - `examples/riot_check.rs` — консольная проверка Riot-клиента (шаг 2);
//! - тесты (`cargo test`).
//!
//! Снаружи модули доступны как `lol_account_manager::riot::RiotClient` и т.д.

pub mod app;
pub mod backend;
pub mod clipboard;
pub mod error;
pub mod history;
pub mod images;
pub mod launcher;
pub mod lcu;
pub mod lobby;
pub mod messages;
pub mod riot;
pub mod storage;
pub mod transfer;
pub mod ui;
