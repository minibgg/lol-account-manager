//! Шаг 3: проверка хранилища (SQLite + системное хранилище паролей) из консоли.
//!
//!   cargo run --example storage_check                          # что сохранено
//!   cargo run --example storage_check -- self-test             # автопроверка на временных данных
//!   cargo run --example storage_check -- set-key               # ключ из $env:RIOT_API_KEY → хранилище
//!   cargo run --example storage_check -- delete-key            # удалить сохранённый ключ
//!   cargo run --example storage_check -- add "Имя#TAG" euw [теги...]
//!   cargo run --example storage_check -- refresh               # «Обновить все»
//!   cargo run --example storage_check -- remove 3              # удалить аккаунт №3
//!
//! `add` и `refresh` работают через тот же фоновый поток, что и окно приложения:
//! данные берутся из запущенного клиента League, а для других серверов — из Riot API.
//!
//! Логин и пароль через командную строку НЕ принимаются: они остались бы в истории
//! терминала. В приложении их можно будет ввести в окне (шаг 5), а `self-test`
//! проверяет их хранение на временных данных.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use eframe::egui;
use lol_account_manager::backend::Backend;
use lol_account_manager::error::AppError;
use lol_account_manager::messages::{Command, Event};
use lol_account_manager::riot::{ApiKey, DataSource, RankedStats, Region, RiotClient, RiotId};
use lol_account_manager::storage::{
    AccountInput, Credentials, Db, SecretChange, Secrets, Storage,
};
use tracing_subscriber::EnvFilter;

type CliResult = Result<(), Box<dyn Error>>;

const USAGE: &str = "Команды: status | self-test | set-key | delete-key | add \"Имя#TAG\" <регион> [теги...] | refresh | remove <номер>";

#[tokio::main]
async fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,lol_account_manager=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    // `args.get(1..)` — срез аргументов после команды (или None, если их нет).
    let rest = args.get(1..).unwrap_or_default();
    let result = match args.first().map(String::as_str) {
        None | Some("status") => status(),
        Some("self-test") => self_test(),
        Some("set-key") => set_key().await,
        Some("delete-key") => delete_key(),
        Some("add") => add(rest).await,
        Some("refresh") => refresh_all().await,
        Some("remove") => remove(rest),
        Some(other) => Err(format!("неизвестная команда «{other}»\n{USAGE}").into()),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

fn status() -> CliResult {
    let path = Storage::default_db_path()?;
    let storage = Storage::open_default()?;

    println!("База:      {}", path.display());
    println!(
        "API-ключ:  {}",
        match storage.api_key()? {
            Some(_) => "сохранён в системном хранилище паролей",
            None => "не задан (сохраните: cargo run --example storage_check -- set-key)",
        }
    );

    let accounts = storage.list_accounts()?;
    println!("Аккаунтов: {}", accounts.len());
    for a in &accounts {
        println!("\n#{} {} [{}]", a.id, a.riot_id, a.region);
        if !a.tags.is_empty() {
            println!("   теги:     {}", a.tags.join(", "));
        }
        if !a.note.is_empty() {
            println!("   заметка:  {}", a.note);
        }
        println!(
            "   логин:    {}, пароль: {}",
            yes_no(a.has_login),
            yes_no(a.has_password)
        );
        match &a.cache {
            Some(c) => {
                println!(
                    "   уровень {}, иконка {}, обновлено {}",
                    c.summoner_level,
                    c.profile_icon_id,
                    ago(c.fetched_at)
                );
                println!("   Solo/Duo: {}", rank_line(c.ranked.solo.as_ref()));
                println!("   Flex:     {}", rank_line(c.ranked.flex.as_ref()));
            }
            None => println!("   данных пока нет"),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// API-ключ
// ---------------------------------------------------------------------------

async fn set_key() -> CliResult {
    let raw = std::env::var("RIOT_API_KEY")
        .map_err(|_| "сначала задайте в этом окне терминала: $env:RIOT_API_KEY = \"RGAPI-...\"")?;
    let key = ApiKey::new(&raw)?;

    println!("Проверяю ключ в Riot API...");
    let client = RiotClient::new()?.with_api_key(key);
    match client.check_api_key(Region::Euw).await {
        Ok(()) => println!("Ключ рабочий."),
        Err(AppError::InvalidApiKey) => {
            return Err("Riot отклонил ключ: он недействителен или истёк. Ничего не сохранено.".into());
        }
        Err(e) => println!("Проверить не удалось ({e}) — сохраняю без проверки."),
    }

    Storage::open_default()?.set_api_key(&raw)?;
    println!("✓ Ключ сохранён в системном хранилище паролей (запись «riot-api-key.lol-account-manager»).");
    println!("  Переменная окружения больше не нужна: Remove-Item Env:RIOT_API_KEY");
    Ok(())
}

fn delete_key() -> CliResult {
    Storage::open_default()?.delete_api_key()?;
    println!("✓ Ключ удалён из системного хранилища паролей.");
    Ok(())
}

// ---------------------------------------------------------------------------
// add / remove
// ---------------------------------------------------------------------------

/// Добавление идёт через тот же фоновый поток, что и в окне приложения: отправляем
/// команду в канал и ждём событие с результатом. Источник данных выбирается так же —
/// клиент League (если запущен на этом сервере), иначе Riot API (если сохранён ключ).
async fn add(args: &[String]) -> CliResult {
    // Сопоставление со срезом: первые два элемента — Riot ID и регион, остальное — теги.
    let [riot_id_arg, region_arg, tags @ ..] = args else {
        return Err(USAGE.into());
    };
    let input = AccountInput {
        riot_id: RiotId::parse(riot_id_arg)?,
        region: region_arg.parse()?,
        note: String::new(),
        tags: tags.to_vec(),
    };
    println!("Добавляю {} [{}] (через клиент League или Riot API)...", input.riot_id, input.region);

    // Context egui без окна: фоновый поток будет «будить» его, но здесь это ни на что не влияет.
    let mut backend = Backend::start(egui::Context::default());
    backend.send(Command::AddAccount {
        input,
        credentials: Credentials::default(),
    });

    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        while let Some(event) = backend.try_recv() {
            match event {
                Event::StartupFailed(e) | Event::AddFailed(e) => return Err(e.into()),
                Event::AccountAdded {
                    account,
                    fetch_error,
                } => {
                    println!("✓ Добавлен аккаунт #{}: {}", account.id, account.riot_id);
                    match (&account.cache, fetch_error) {
                        (Some(cache), _) => {
                            let source = if account.lcu_puuid.is_some() {
                                "клиент League"
                            } else {
                                "Riot API"
                            };
                            println!("  данные получены через: {source}");
                            println!("  Solo/Duo: {}", rank_line(cache.ranked.solo.as_ref()));
                        }
                        (None, Some(e)) => println!("  без данных: {e}"),
                        (None, None) => {}
                    }
                    return Ok(());
                }
                _ => {} // остальные события (состояние клиента League и т.п.) здесь не важны
            }
        }
        if Instant::now() > deadline {
            return Err("нет ответа от фонового потока за 90 с".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// «Обновить все» — та же команда, что и кнопка в окне, через тот же фоновый поток.
async fn refresh_all() -> CliResult {
    let mut backend = Backend::start(egui::Context::default());
    backend.send(Command::RefreshAll);
    println!("Обновляю все аккаунты...");

    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        while let Some(event) = backend.try_recv() {
            match event {
                Event::StartupFailed(e) => return Err(e.into()),
                Event::AccountRefreshed { account, source } => {
                    let via = match source {
                        DataSource::Lcu => "клиент League",
                        DataSource::RiotApi => "Riot API",
                    };
                    let solo = account.cache.as_ref().and_then(|c| c.ranked.solo.as_ref());
                    println!("  ✓ {} [{}] через {via}: {}", account.riot_id, account.region, rank_line(solo));
                }
                Event::RefreshFailed { id, error } => println!("  ✗ аккаунт #{id}: {error}"),
                Event::RateLimitWait { secs } => println!("  ⏳ лимит Riot API, ждём {secs} с"),
                Event::Warning(e) => println!("  ! {e}"),
                Event::RefreshAllDone { ok, failed } => {
                    println!("Готово: обновлено {ok}, с ошибками {failed}");
                    return Ok(());
                }
                _ => {}
            }
        }
        if Instant::now() > deadline {
            return Err("обновление не закончилось за 5 минут".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn remove(args: &[String]) -> CliResult {
    let id: i64 = args.first().ok_or(USAGE)?.parse()?;
    let mut storage = Storage::open_default()?;
    let account = storage.get_account(id)?;
    storage.delete_account(id)?;
    println!(
        "✓ Удалён аккаунт #{id}: {} [{}] вместе с логином и паролем из хранилища",
        account.riot_id, account.region
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// self-test
// ---------------------------------------------------------------------------

/// Уборка после самопроверки. `Drop` вызывается автоматически, когда переменная
/// выходит из области видимости — в том числе при раннем выходе через `?` с ошибкой.
/// Так временные записи в хранилище паролей удалятся при любом исходе.
struct Cleanup {
    dir: PathBuf,
    db_path: PathBuf,
    secrets: Secrets,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Ok(db) = Db::open(&self.db_path)
            && let Ok(accounts) = db.list_accounts()
        {
            for a in accounts {
                let _ = self.secrets.delete_credentials(&a.secret_ref);
            }
        }
        let _ = self.secrets.delete_api_key();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn self_test() -> CliResult {
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let dir = std::env::temp_dir().join(format!("lolam-selftest-{tag}"));
    std::fs::create_dir_all(&dir)?;
    let db_path = dir.join("selftest.db");
    // Отдельное имя сервиса — настоящие записи приложения не затрагиваются.
    let secrets = Secrets::with_service(&format!("lol-account-manager-selftest-{tag}"));

    // Объявлена ДО storage → уничтожится ПОСЛЕ него (обратный порядок), когда база уже закрыта.
    let _cleanup = Cleanup {
        dir: dir.clone(),
        db_path: db_path.clone(),
        secrets: secrets.clone(),
    };
    let mut storage = Storage::open(&db_path, secrets.clone())?;
    ok("временная база создана, миграции применены");

    // Уникальные значения, чтобы потом искать их байты в файле базы.
    let login = format!("selftest-login-{tag}");
    let password = format!("P@ss пароль {tag} 🔑");
    let input = AccountInput {
        riot_id: RiotId::parse("Тест Ёжик#SELF")?,
        region: Region::Euw,
        note: "проверка".into(),
        tags: vec!["main".into()],
    };

    let account = storage.add_account(
        &input,
        Credentials::new(Some(login.clone()), Some(password.clone())),
    )?;
    check(account.has_login && account.has_password, "аккаунт с логином и паролем добавлен")?;

    let creds = storage.credentials(account.id)?;
    check(
        creds.login.as_deref() == Some(login.as_str())
            && creds.password.as_deref() == Some(password.as_str()),
        "логин и пароль читаются из системного хранилища",
    )?;

    check(
        dir_contains(&dir, "Тест Ёжик")?,
        "обычные данные (Riot ID) лежат в файле базы",
    )?;
    check(
        !dir_contains(&dir, &login)? && !dir_contains(&dir, &password)?,
        "логина и пароля НЕТ в файлах базы (искали в UTF-8 и UTF-16)",
    )?;

    let new_password = format!("новый-{tag}");
    storage.update_account(
        account.id,
        &input,
        SecretChange::Keep,
        SecretChange::Set(new_password.clone()),
    )?;
    let creds = storage.credentials(account.id)?;
    check(
        creds.login.as_deref() == Some(login.as_str())
            && creds.password.as_deref() == Some(new_password.as_str()),
        "смена пароля: пароль новый, логин прежний",
    )?;

    let updated =
        storage.update_account(account.id, &input, SecretChange::Clear, SecretChange::Keep)?;
    check(
        !updated.has_login && updated.has_password,
        "удаление только логина",
    )?;

    let duplicate = AccountInput {
        riot_id: RiotId::parse("тест ёжик#self")?,
        ..input.clone()
    };
    check(
        matches!(
            storage.add_account(&duplicate, Credentials::default()),
            Err(AppError::DuplicateAccount)
        ),
        "дубликат (другой регистр букв) не добавляется",
    )?;

    let secret_ref = account.secret_ref.clone();
    storage.delete_account(account.id)?;
    check(
        secrets.credentials(&secret_ref)?.is_empty() && storage.list_accounts()?.is_empty(),
        "удаление аккаунта удаляет и его логин/пароль из хранилища",
    )?;

    storage.set_api_key(&format!("RGAPI-selftest-{tag}"))?;
    let saved = storage.api_key()?.is_some();
    storage.delete_api_key()?;
    check(
        saved && storage.api_key()?.is_none(),
        "API-ключ сохраняется и удаляется",
    )?;

    println!("\nВсе проверки пройдены. Временные данные удалены.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Мелочи
// ---------------------------------------------------------------------------

fn ok(what: &str) {
    println!("  ✓ {what}");
}

fn check(condition: bool, what: &str) -> CliResult {
    if condition {
        ok(what);
        Ok(())
    } else {
        Err(format!("НЕ ПРОЙДЕНО: {what}").into())
    }
}

/// Есть ли строка в каком-нибудь файле папки — в кодировках UTF-8 и UTF-16LE
/// (во втором виде строки хранит Windows).
fn dir_contains(dir: &Path, needle: &str) -> std::io::Result<bool> {
    let utf8 = needle.as_bytes().to_vec();
    let utf16: Vec<u8> = needle.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    for entry in std::fs::read_dir(dir)? {
        let bytes = std::fs::read(entry?.path())?;
        if contains(&bytes, &utf8) || contains(&bytes, &utf16) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn yes_no(v: bool) -> &'static str {
    if v { "есть" } else { "нет" }
}

fn rank_line(stats: Option<&RankedStats>) -> String {
    stats.map_or_else(|| "без ранга".to_string(), |s| s.to_string())
}

fn ago(ts: i64) -> String {
    lol_account_manager::ui::format::relative_time(ts)
}
