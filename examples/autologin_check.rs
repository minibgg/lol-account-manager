//! Проверка частей автовхода (Shift+Enter) из консоли — БЕЗ ввода логина и пароля:
//! `cargo run --example autologin_check`.
//!
//! Показывает, что отвечает Riot Client о состоянии входа, и раз в секунду (10 секунд) —
//! видит ли программа окно Riot Client на переднем плане. Переключитесь на окно
//! Riot Client, чтобы убедиться, что оно распознаётся.

use std::time::Duration;

use lol_account_manager::autologin;
use lol_account_manager::lcu::{LcuClient, lockfile};

#[tokio::main]
async fn main() {
    let lcu = match LcuClient::new(lockfile::candidate_dirs(None)) {
        Ok(lcu) => lcu,
        Err(e) => return println!("ошибка: {e}"),
    };
    match lcu.riot_session().await {
        Some(session) => println!("Riot Client: {session:?}"),
        None => println!("Riot Client: не запущен или не отвечает"),
    }
    for second in 1..=10 {
        println!("{second:>2} с: окно Riot Client на переднем плане — {}", autologin::riot_window_focused());
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
