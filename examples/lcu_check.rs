//! Проверка локального API клиента League (LCU) из консоли.
//!
//!   cargo run --example lcu_check                  # запущен ли клиент, сервер, текущий аккаунт
//!   cargo run --example lcu_check -- "Имя#TAG"     # найти игрока через клиент и показать ранги
//!
//! Нужен запущенный клиент League of Legends с выполненным входом.
//! Только чтение: программа ничего не вводит в клиент и ничего в нём не нажимает.
//! Если клиент установлен не в стандартную папку: $env:LOLAM_LEAGUE_DIR = "D:\Games\League of Legends"

use std::error::Error;
use std::process::ExitCode;
use std::time::Instant;

use lol_account_manager::lcu::{LcuClient, LcuStatus, lockfile};
use lol_account_manager::lobby::LiveGame;
use lol_account_manager::riot::{RankedStats, RiotId};
use tracing_subscriber::EnvFilter;

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

async fn run() -> Result<(), Box<dyn Error>> {
    let dirs = lockfile::candidate_dirs(None);
    println!("Ищу клиент в папках:");
    for dir in &dirs {
        let found = dir.join("lockfile").exists();
        println!("  {} {}", if found { "✓" } else { "·" }, dir.display());
    }

    let lcu = LcuClient::new(dirs)?;
    let (region, me) = match lcu.status().await {
        LcuStatus::Connected {
            region, riot_id, ..
        } => (region, riot_id),
        LcuStatus::Loading => return Err("клиент запущен, но ещё загружается или вход не выполнен".into()),
        LcuStatus::NotRunning => return Err("клиент League не запущен".into()),
    };
    println!("Клиент:    подключён, сервер {region} — {}", region.full_name());
    println!("В клиенте: {me}");

    // Кого искать: аргумент или сам вошедший аккаунт.
    let target = match std::env::args().nth(1) {
        Some(arg) => RiotId::parse(&arg)?,
        None => me,
    };
    let started = Instant::now();
    let snapshot = lcu.fetch_by_riot_id(&target, region).await?;
    println!("\nИгрок:     {}", snapshot.riot_id);
    println!("Уровень:   {}", snapshot.summoner_level);
    println!("Иконка:    {}", snapshot.profile_icon_id);
    println!("Solo/Duo:  {}", rank_line(snapshot.ranked.solo.as_ref()));
    println!("Flex:      {}", rank_line(snapshot.ranked.flex.as_ref()));
    match &snapshot.matches {
        None => println!("Игры:      не загрузились"),
        Some(matches) if matches.is_empty() => println!("Игры:      нет"),
        Some(matches) => {
            println!("Последние игры:");
            for m in matches {
                let result = if m.remake { "ремейк" } else if m.win { "победа" } else { "поражение" };
                println!(
                    "  {:<9} чемпион #{:<4} {:>8}  {} мин  {}",
                    result,
                    m.me.champion_id,
                    m.kda(),
                    m.duration_secs / 60,
                    m.queue_label()
                );
            }
        }
    }
    // Инвентарь и текущая игра — только для аккаунта, в который выполнен вход.
    match lcu.inventory(0).await {
        Ok(i) => println!(
            "Инвентарь: чемпионов {}, образов {}, синей эссенции {}, оранжевой {}, RP {}",
            i.champions, i.skins, i.blue_essence, i.orange_essence, i.rp
        ),
        Err(e) => println!("Инвентарь: не загрузился ({e})"),
    }
    let my_puuid = lcu.current_summoner().await?.puuid;
    let champions = lcu.champion_names().await.unwrap_or_default();
    match lcu.gameflow_session().await? {
        Some(session) => match LiveGame::from_session(&session, &my_puuid, &champions, 0) {
            Some(game) => println!(
                "Сейчас:    игра {} — очередь {}, чемпион «{}», позиция «{}»",
                game.game_id, game.queue_id, game.champion, game.position
            ),
            None => println!("Сейчас:    игры нет"),
        },
        None => println!("Сейчас:    игры нет"),
    }
    println!("Готово за {:.2} с (без API-ключа)", started.elapsed().as_secs_f64());
    Ok(())
}

fn rank_line(stats: Option<&RankedStats>) -> String {
    stats.map_or_else(|| "без ранга".to_string(), |s| s.to_string())
}
