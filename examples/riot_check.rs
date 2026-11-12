//! Шаг 2: проверка Riot-клиента из консоли.
//!
//! PowerShell:
//!   $env:RIOT_API_KEY = "RGAPI-..."
//!   cargo run --example riot_check -- "Имя#TAG" euw
//!   cargo run --example riot_check -- "Имя#TAG" euw --repeat 40   # проверка лимитера
//!
//! bash:
//!   RIOT_API_KEY=RGAPI-... cargo run --example riot_check -- "Имя#TAG" euw
//!
//! Ключ берётся из переменной окружения RIOT_API_KEY, а если её нет — из системного
//! хранилища паролей (сохраняется командой `cargo run --example storage_check -- set-key`).
//! Подробный лог каждого запроса: добавьте `$env:RUST_LOG="debug"`.

use std::error::Error;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use lol_account_manager::riot::{ApiKey, RankedStats, Region, RiotClient, RiotId};
use lol_account_manager::storage::Secrets;
use tokio::task::JoinSet;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "Использование: cargo run --example riot_check -- \"Имя#TAG\" <регион> [--repeat N]
Регионы: EUW EUNE NA KR TR RU BR LAN LAS OCE JP ME SG TW VN";

/// `#[tokio::main]` создаёт tokio-runtime и запускает в нём async main.
/// В окне приложения так не получится (eframe сам владеет главным потоком),
/// там на шаге 4 runtime будет жить в отдельном потоке.
#[tokio::main]
async fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,lol_account_manager=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\nОшибка: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `Box<dyn Error>` — «любая ошибка». `?` сам упакует в неё и AppError, и текст.
async fn run() -> Result<(), Box<dyn Error>> {
    // ----- Аргументы командной строки -----
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (riot_id_arg, region_arg) = match (args.first(), args.get(1)) {
        (Some(id), Some(region)) => (id, region),
        _ => return Err(USAGE.into()),
    };
    let repeat: usize = match args.iter().position(|a| a == "--repeat") {
        Some(i) => args.get(i + 1).ok_or(USAGE)?.parse()?,
        None => 0,
    };

    let riot_id = RiotId::parse(riot_id_arg)?;
    let region: Region = region_arg.parse()?;

    // ----- Ключ: из переменной окружения, иначе из системного хранилища -----
    let key = match std::env::var("RIOT_API_KEY") {
        Ok(raw) => {
            println!("Ключ:      из переменной RIOT_API_KEY");
            ApiKey::new(&raw)?
        }
        Err(_) => {
            let key = Secrets::new().api_key()?.ok_or(
                "ключ не найден: сохраните его командой \
                 `cargo run --example storage_check -- set-key` или задайте $env:RIOT_API_KEY",
            )?;
            println!("Ключ:      из системного хранилища паролей");
            key
        }
    };

    let client = RiotClient::new()?
        .with_api_key(key)
        .on_rate_limit_wait(|wait| {
            println!("  ⏳ Лимит запросов, повтор через {} сек", wait.as_secs());
        });

    println!("Riot ID:   {riot_id}");
    println!(
        "Регион:    {region} — {} (платформа {}, account-v1 через {})",
        region.full_name(),
        region.platform(),
        region.account_route().host()
    );

    // ----- Основная проверка: 3 запроса -----
    let started = Instant::now();
    let snapshot = client.fetch_by_riot_id(&riot_id, region).await?;
    let elapsed = started.elapsed();

    // Data Dragon — отдельно: если CDN недоступен, это не повод падать.
    let icon = match client.ddragon_latest_version().await {
        Ok(version) => RiotClient::profile_icon_url(&version, snapshot.profile_icon_id),
        Err(e) => format!("(Data Dragon недоступен: {e})"),
    };

    println!("PUUID:     {}", shorten(&snapshot.puuid));
    if snapshot.riot_id != riot_id {
        println!("Точный Riot ID по данным Riot: {}", snapshot.riot_id);
    }
    println!("Уровень:   {}", snapshot.summoner_level);
    println!("Иконка:    {} — {icon}", snapshot.profile_icon_id);
    println!("Solo/Duo:  {}", rank_line(snapshot.ranked.solo.as_ref()));
    println!("Flex:      {}", rank_line(snapshot.ranked.flex.as_ref()));
    match &snapshot.matches {
        None => println!("Игры:      не загрузились"),
        Some(matches) if matches.is_empty() => println!("Игры:      нет"),
        Some(matches) => {
            println!("Последние игры (match-v5):");
            for m in matches {
                let result = if m.remake { "ремейк" } else if m.win { "победа" } else { "поражение" };
                println!("  {:<15} {:<9} чемпион #{:<4} {:>8}  {}", m.id, result, m.me.champion_id, m.kda(), m.queue_label());
            }
        }
    }
    // Запросы: account-v1, summoner-v4, league-v4, список игр и по запросу на каждую игру.
    let requests = 4 + snapshot.matches.as_ref().map_or(0, Vec::len);
    println!("Готово за {:.2} с ({requests} запросов к Riot API)", elapsed.as_secs_f64());

    // ----- Необязательно: проверка лимитера -----
    if repeat > 0 {
        stress_rate_limiter(&client, &snapshot.puuid, region, repeat).await?;
    }
    Ok(())
}

/// Запускает `n` запросов league-v4 одновременно и показывает, как лимитер их растягивает.
async fn stress_rate_limiter(
    client: &RiotClient,
    puuid: &str,
    region: Region,
    n: usize,
) -> Result<(), Box<dyn Error>> {
    println!("\nПроверка лимитера: {n} одновременных запросов к {}...", region.platform());
    let started = Instant::now();

    // JoinSet — набор параллельных задач tokio.
    let mut tasks = JoinSet::new();
    for i in 1..=n {
        // Каждая задача получает СВОЮ копию клиента и puuid: `move` переносит их
        // в замыкание во владение. Задача может пережить текущую функцию, поэтому
        // заимствовать (`&client`) отсюда нельзя — компилятор это запретит.
        let client = client.clone();
        let puuid = puuid.to_string();
        tasks.spawn(async move {
            let result = client.ranked_by_puuid(&puuid, region).await;
            (i, result)
        });
    }

    let mut errors = 0;
    // join_next() отдаёт задачи по мере завершения.
    while let Some(joined) = tasks.join_next().await {
        let (i, result) = joined?;
        let t = started.elapsed();
        match result {
            Ok(_) => println!("  #{i:>3} готов через {:>5.1} с", t.as_secs_f64()),
            Err(e) => {
                errors += 1;
                println!("  #{i:>3} ОШИБКА через {:>5.1} с: {e}", t.as_secs_f64());
            }
        }
    }

    let total = started.elapsed();
    println!(
        "Итого: {n} запросов за {:.1} с, ошибок: {errors}. Ожидаемо: первые ~28 быстро \
         (до 10 в секунду; 2 запроса уже ушли выше), дальше примерно по одному в 1,8 с.",
        total.as_secs_f64()
    );
    if total < Duration::from_secs(1) && n > 20 {
        println!("Подозрительно быстро — лимитер мог не сработать!");
    }
    Ok(())
}

fn rank_line(stats: Option<&RankedStats>) -> String {
    match stats {
        Some(s) => s.to_string(),
        None => "без ранга".to_string(),
    }
}

/// PUUID длинный (78 символов) — показываем начало и конец.
/// Срез `&s[..8]` режет по байтам; PUUID состоит только из ASCII, поэтому это безопасно
/// (для кириллицы так делать нельзя — можно попасть в середину символа и получить панику).
fn shorten(s: &str) -> String {
    if s.len() <= 20 {
        return s.to_string();
    }
    format!("{}…{}", &s[..8], &s[s.len() - 8..])
}
