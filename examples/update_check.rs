//! Проверка обновлений из консоли — без окна и без установки.
//!
//! ```text
//! cargo run --example update_check                      есть ли версия новее этой
//! cargo run --example update_check -- 0.0.1             …новее указанной
//! cargo run --example update_check -- 0.0.1 --download  ещё и скачать установщик и сверить SHA-256
//! ```
//!
//! Установщик НЕ запускается: файл кладётся во временную папку, путь печатается.

use lol_account_manager::update::{Updater, Version};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let download = args.iter().any(|a| a == "--download");
    let current = match args.iter().find(|a| !a.starts_with("--")) {
        Some(text) => Version::parse(text).ok_or("версия — три числа через точку, например 0.1.0")?,
        None => Version::current(),
    };

    let dir = std::env::temp_dir().join("lol-account-manager-update-check");
    let updater = Updater::new(dir)?;
    println!("Сравниваю с версией {current}…");
    let Some(update) = updater.check(current).await? else {
        println!("Новее ничего нет.");
        return Ok(());
    };
    println!("Доступна версия {}", update.version);
    println!("Страница:   {}", update.page_url);
    let Some(installer) = &update.installer else {
        println!("Установщика в выпуске нет (или нет контрольных сумм).");
        return Ok(());
    };
    println!("Установщик: {} ({:.1} МБ)", installer.name, installer.size as f64 / 1024.0 / 1024.0);

    if download {
        let mut last_percent = u64::MAX;
        let path = updater
            .download(installer, |received, total| {
                let percent = received * 100 / total.max(1);
                if percent / 10 != last_percent / 10 {
                    println!("  {percent}%");
                }
                last_percent = percent;
            })
            .await?;
        println!("Скачано, SHA-256 совпала: {}", path.display());
    }
    Ok(())
}
