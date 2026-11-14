//! Проверка Windows Hello из консоли.
//!
//!   cargo run --example hello_check            # настроен ли Windows Hello (окно не показывается)
//!   cargo run --example hello_check -- --ask   # показать окно подтверждения и вывести итог

use lol_account_manager::hello;

fn main() {
    if !hello::is_supported() {
        println!("Windows Hello есть только в Windows");
        return;
    }
    match hello::is_available() {
        Ok(true) => println!("Windows Hello настроен"),
        Ok(false) => println!("Windows Hello не настроен (нужен хотя бы PIN-код)"),
        Err(e) => println!("не удалось узнать: {e}"),
    }
    if std::env::args().any(|arg| arg == "--ask") {
        match hello::verify("LoL Account Manager: проверка Windows Hello") {
            Ok(()) => println!("подтверждено"),
            Err(e) => println!("не подтверждено: {e}"),
        }
    }
}
