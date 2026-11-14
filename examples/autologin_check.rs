//! Проверка связи с локальным API Riot Client для входа (Shift+Enter) — БЕЗ входа:
//! `cargo run --example autologin_check`.
//!
//! Показывает, запущен ли Riot Client и выполнен ли в нём вход. Сам вход не начинает
//! (чтобы не трогать текущий сеанс) — запустите его в приложении через Shift+Enter.

use lol_account_manager::lcu::{LcuClient, RiotSession, lockfile};

#[tokio::main]
async fn main() {
    let lcu = match LcuClient::new(lockfile::candidate_dirs(None)) {
        Ok(lcu) => lcu,
        Err(e) => return println!("ошибка: {e}"),
    };
    match lcu.riot_session().await {
        None => println!("Riot Client не запущен или ещё не отвечает"),
        Some(RiotSession::Authenticated) => println!("Riot Client: выполнен вход (для входа в другой аккаунт сначала выйдите)"),
        Some(RiotSession::NeedsLogin) => println!("Riot Client: показано окно входа — можно входить через Shift+Enter"),
        Some(RiotSession::Multifactor) => println!("Riot Client: ждёт код подтверждения"),
    }
}
