//! Проверка уведомлений Windows: `cargo run --example notify_check`.
//!
//! Показывает одно тестовое уведомление и ждёт 10 секунд: щёлкните по нему —
//! в консоли появится «щелчок получен».

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lol_account_manager::notify::{Message, Notifier};

fn main() {
    if !lol_account_manager::notify::is_supported() {
        println!("уведомления есть только в Windows");
        return;
    }
    // Arc<AtomicBool> — флаг, который можно менять из другого потока.
    let clicked = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&clicked);
    let notifier = Notifier::new(move || flag.store(true, Ordering::SeqCst));
    notifier.show(
        Message::new("Победа · Экко 14/5/7", "+18 LP → Золото II · 54 LP").detail("Проверка уведомлений · Name#RU1"),
    );
    println!("уведомление отправлено; жду 10 секунд…");
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(500));
        if clicked.load(Ordering::SeqCst) {
            println!("щелчок получен");
            return;
        }
    }
    println!("по уведомлению не щёлкали");
}
