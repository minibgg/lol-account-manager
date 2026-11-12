//! Экспорт и импорт списка через фоновый поток: база → файл → другая база.
//!
//! Работает на временных папках и временном имени сервиса в хранилище паролей.
//! Логины и пароли не используются — в хранилище ничего не записывается.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use lol_account_manager::backend::{Backend, BackendConfig};
use lol_account_manager::messages::{Command, Event, ImportReport};
use lol_account_manager::riot::{Region, RiotId};
use lol_account_manager::storage::{AccountInput, Credentials, Secrets, Storage};

fn wait_for(backend: &mut Backend, what: &str, mut pred: impl FnMut(&Event) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        while let Some(event) = backend.try_recv() {
            if pred(&event) {
                return event;
            }
        }
        assert!(Instant::now() < deadline, "не дождались события: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Временная папка удаляется даже при панике в assert.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn start(dir: &std::path::Path, db: &str, secrets: &Secrets) -> Backend {
    let db_path = dir.join(db);
    let secrets = secrets.clone();
    let config = BackendConfig {
        open_storage: Box::new(move || Storage::open(&db_path, secrets)),
        clipboard_clear_after: Duration::from_secs(30),
        image_cache_dir: dir.join("images"),
    };
    Backend::start_with(egui::Context::default(), config)
}

fn input(id: &str, region: Region, tags: &[&str], note: &str) -> AccountInput {
    AccountInput {
        riot_id: RiotId::parse(id).unwrap(),
        region,
        note: note.into(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
    }
}

#[test]
fn export_then_import_into_another_database() {
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let dir = TempDir(std::env::temp_dir().join(format!("lolam-transfer-{tag}")));
    std::fs::create_dir_all(&dir.0).unwrap();
    let secrets = Secrets::with_service(&format!("lol-account-manager-transfer-{tag}"));

    // 1. Исходная база с двумя аккаунтами (без логинов и паролей).
    {
        let mut storage = Storage::open(&dir.0.join("a.db"), secrets.clone()).unwrap();
        storage
            .add_account(&input("Шрек#RU1", Region::Ru, &["main"], "Основной"), Credentials::default())
            .unwrap();
        storage
            .add_account(&input("TransferTest#EUW", Region::Euw, &[], ""), Credentials::default())
            .unwrap();
    }

    // 2. Экспорт.
    let file = dir.0.join("lol-accounts.json");
    let mut a = start(&dir.0, "a.db", &secrets);
    wait_for(&mut a, "Ready", |e| matches!(e, Event::Ready { .. }));
    a.send(Command::ExportAccounts(file.clone()));
    let exported = wait_for(&mut a, "Exported", |e| matches!(e, Event::Exported(_)));
    assert!(matches!(exported, Event::Exported(Ok(2))), "{exported:?}");
    drop(a);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("Шрек#RU1") && !text.contains("secret_ref"), "{text}");

    // 3. Импорт в пустую базу: оба аккаунта добавлены, теги и заметка на месте.
    let mut b = start(&dir.0, "b.db", &secrets);
    wait_for(&mut b, "Ready", |e| matches!(e, Event::Ready { .. }));
    b.send(Command::ImportAccounts(file.clone()));
    let accounts = wait_for(&mut b, "Accounts", |e| matches!(e, Event::Accounts(list) if !list.is_empty()));
    let Event::Accounts(accounts) = accounts else { unreachable!() };
    let names: Vec<String> = accounts.iter().map(|a| a.riot_id.to_string()).collect();
    assert_eq!(names, ["Шрек#RU1", "TransferTest#EUW"]);
    assert_eq!(accounts[0].tags, ["main"]);
    assert_eq!(accounts[0].note, "Основной");
    assert!(!accounts[0].has_login && !accounts[0].has_password, "логины и пароли не переносятся");
    let imported = wait_for(&mut b, "Imported", |e| matches!(e, Event::Imported(_)));
    let expected = ImportReport { added: 2, duplicates: 0, invalid: 0 };
    assert!(matches!(&imported, Event::Imported(Ok(r)) if *r == expected), "{imported:?}");

    // 4. Повторный импорт того же файла ничего не дублирует.
    b.send(Command::ImportAccounts(file));
    let again = wait_for(&mut b, "Imported #2", |e| matches!(e, Event::Imported(_)));
    let expected = ImportReport { added: 0, duplicates: 2, invalid: 0 };
    assert!(matches!(&again, Event::Imported(Ok(r)) if *r == expected), "{again:?}");

    // 5. Чужой файл — понятная ошибка.
    let foreign = dir.0.join("foreign.json");
    std::fs::write(&foreign, r#"{"hello": "world"}"#).unwrap();
    b.send(Command::ImportAccounts(foreign));
    let failed = wait_for(&mut b, "Imported #3", |e| matches!(e, Event::Imported(_)));
    assert!(
        matches!(failed, Event::Imported(Err(lol_account_manager::error::AppError::NotAnExportFile))),
        "{failed:?}"
    );
}
