//! Сквозная проверка фонового потока (шаг 4): команды → события.
//!
//! Добавление аккаунта с логином и паролем, копирование в буфер обмена, автоочистка
//! буфера, удаление аккаунта вместе с записью в хранилище паролей.
//!
//! Использует НАСТОЯЩИЕ буфер обмена и хранилище паролей (с отдельным временным именем
//! сервиса и временной базой), поэтому по умолчанию пропускается. Запуск:
//!   cargo test --test backend_e2e -- --ignored
//! Текст, который был в буфере обмена до теста, в конце возвращается на место.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use lol_account_manager::backend::{Backend, BackendConfig};
use lol_account_manager::error::AppError;
use lol_account_manager::messages::{Command, CredentialKind, Event};
use lol_account_manager::riot::{Region, RiotId};
use lol_account_manager::storage::{AccountInput, Credentials, Secrets, Storage};

const LOGIN: &str = "e2e-login";
const PASSWORD: &str = "e2e пароль ✓ с пробелами ";

/// Ждёт событие, для которого `pred` вернёт true; остальные пропускает.
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

/// Все события за `duration`.
fn collect_for(backend: &mut Backend, duration: Duration) -> Vec<Event> {
    let deadline = Instant::now() + duration;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        while let Some(event) = backend.try_recv() {
            events.push(event);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    events
}

/// Уборка при любом исходе теста (даже при панике в assert): Drop вызывается всегда.
struct Cleanup {
    dir: PathBuf,
    secrets: Secrets,
    secret_ref: Option<String>,
    previous_clipboard: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(secret_ref) = &self.secret_ref {
            let _ = self.secrets.delete_credentials(secret_ref);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(self.previous_clipboard.clone());
        }
    }
}

#[test]
#[ignore = "использует системный буфер обмена и хранилище паролей"]
fn add_copy_autoclear_delete() {
    let mut clipboard = arboard::Clipboard::new().expect("буфер обмена");
    // Если в буфере не текст (например, картинка), вернуть его потом не получится —
    // не рискуем чужими данными и пропускаем тест.
    let Ok(previous_clipboard) = clipboard.get_text() else {
        eprintln!("В буфере обмена не текст — тест пропущен, чтобы не потерять его содержимое.");
        return;
    };

    let tag = uuid::Uuid::new_v4().simple().to_string();
    let dir = std::env::temp_dir().join(format!("lolam-e2e-{tag}"));
    std::fs::create_dir_all(&dir).unwrap();
    let secrets = Secrets::with_service(&format!("lol-account-manager-e2e-{tag}"));
    let mut cleanup = Cleanup {
        dir: dir.clone(),
        secrets: secrets.clone(),
        secret_ref: None,
        previous_clipboard,
    };

    let db_path = dir.join("accounts.db");
    let config = BackendConfig {
        open_storage: Box::new(move || Storage::open(&db_path, secrets)),
        clipboard_clear_after: Duration::from_secs(1),
        image_cache_dir: dir.join("images"),
    };
    let mut backend = Backend::start_with(egui::Context::default(), config);

    // 1. Запуск: в тестовом хранилище ключа нет.
    let ready = wait_for(&mut backend, "Ready", |e| matches!(e, Event::Ready { .. }));
    assert!(matches!(ready, Event::Ready { has_api_key: false, .. }));

    // 2. Добавление. Регион EUW: клиент League (если запущен) на другом сервере,
    //    ключа нет — аккаунт сохраняется без данных, с понятной причиной.
    backend.send(Command::AddAccount {
        input: AccountInput {
            riot_id: RiotId::parse("E2eTest#E2E").unwrap(),
            region: Region::Euw,
            note: String::new(),
            tags: vec!["test".into()],
        },
        credentials: Credentials::new(Some(LOGIN.into()), Some(PASSWORD.into())),
    });
    let added = wait_for(&mut backend, "AccountAdded", |e| {
        matches!(e, Event::AccountAdded { .. } | Event::AddFailed(_))
    });
    let Event::AccountAdded {
        account,
        fetch_error,
    } = added
    else {
        panic!("добавление не удалось: {added:?}");
    };
    cleanup.secret_ref = Some(account.secret_ref.clone());
    assert!(account.has_login && account.has_password);
    assert!(account.cache.is_none());
    assert!(
        matches!(
            fetch_error,
            Some(AppError::NoDataSource | AppError::WrongClientRegion { .. })
        ),
        "причина без данных: {fetch_error:?}"
    );
    let id = account.id;

    // 3. Пароль → буфер обмена → автоочистка через 1 с.
    backend.send(Command::CopyCredential {
        id,
        kind: CredentialKind::Password,
    });
    wait_for(&mut backend, "CredentialCopied", |e| {
        matches!(e, Event::CredentialCopied { kind: CredentialKind::Password, .. })
    });
    assert_eq!(clipboard.get_text().unwrap(), PASSWORD, "пароль в буфере, пробелы сохранены");
    wait_for(&mut backend, "ClipboardCleared", |e| matches!(e, Event::ClipboardCleared));
    assert_ne!(clipboard.get_text().unwrap_or_default(), PASSWORD, "буфер очищен");

    // 4. Пользователь успел скопировать своё — таймер это НЕ стирает.
    backend.send(Command::CopyCredential {
        id,
        kind: CredentialKind::Login,
    });
    wait_for(&mut backend, "CredentialCopied", |e| matches!(e, Event::CredentialCopied { .. }));
    assert_eq!(clipboard.get_text().unwrap(), LOGIN);
    clipboard.set_text("текст пользователя").unwrap();
    let events = collect_for(&mut backend, Duration::from_millis(1800));
    assert!(
        !events.iter().any(|e| matches!(e, Event::ClipboardCleared)),
        "чужой текст не должен стираться: {events:?}"
    );
    assert_eq!(clipboard.get_text().unwrap(), "текст пользователя");

    // 5. Два копирования подряд: таймер первого не стирает второе раньше времени.
    backend.send(Command::CopyCredential {
        id,
        kind: CredentialKind::Password,
    });
    backend.send(Command::CopyCredential {
        id,
        kind: CredentialKind::Login,
    });
    let events = collect_for(&mut backend, Duration::from_millis(2500));
    let cleared = events.iter().filter(|e| matches!(e, Event::ClipboardCleared)).count();
    assert_eq!(cleared, 1, "очистка ровно одна — по последнему копированию: {events:?}");

    // 6. Удаление: аккаунт и запись в хранилище паролей исчезают.
    backend.send(Command::DeleteAccount(id));
    wait_for(&mut backend, "AccountDeleted", |e| matches!(e, Event::AccountDeleted(i) if *i == id));
    let left = cleanup
        .secrets
        .credentials(&account.secret_ref)
        .expect("чтение хранилища");
    assert!(left.is_empty(), "запись в хранилище паролей удалена");

    // 7. Копирование удалённого аккаунта — понятная ошибка, а не паника.
    backend.send(Command::CopyCredential {
        id,
        kind: CredentialKind::Password,
    });
    let failed = wait_for(&mut backend, "CopyFailed", |e| matches!(e, Event::CopyFailed(_)));
    assert!(matches!(failed, Event::CopyFailed(AppError::AccountMissing)), "{failed:?}");

    drop(backend); // закрытие: фоновый поток должен завершиться (Drop ждёт его)
}
