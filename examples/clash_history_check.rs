//! Проверка окна «прошлые Clash» из консоли: по каждому источнику отдельно — Riot API
//! (старые игры), история клиента League (свежие) — и что из них получится вместе.
//!
//!   cargo run --example clash_history_check -- "Имя#TAG"
//!
//! Нужен запущенный клиент League (по нему узнаём сервер). Ключ Riot API — из переменной
//! RIOT_API_KEY или из системного хранилища паролей, как в riot_check; без ключа
//! проверяется только клиент. Только чтение. Сегодняшние раунды (итоги команды) здесь не
//! проверяются — для них нужна команда Clash; их показывает clash_check.

use std::error::Error;

use lol_account_manager::clash;
use lol_account_manager::lcu::{LcuClient, lockfile};
use lol_account_manager::riot::models::MatchSummary;
use lol_account_manager::riot::{ApiKey, RiotClient, RiotId};
use lol_account_manager::storage::Secrets;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let arg = std::env::args()
        .nth(1)
        .ok_or("Использование: cargo run --example clash_history_check -- \"Имя#TAG\"")?;
    let riot_id = RiotId::parse(&arg)?;
    let lcu = LcuClient::new(lockfile::candidate_dirs(None))?;
    let region = lcu.region().await?;
    println!("Игрок {riot_id}, сервер {region:?}");

    let key = match std::env::var("RIOT_API_KEY") {
        Ok(raw) => Some(ApiKey::new(&raw)?),
        Err(_) => Secrets::new().api_key().ok().flatten(),
    };
    let mut api_games = Vec::new();
    match key {
        None => println!("\nRiot API: ключа нет — пропускаю"),
        Some(key) => {
            let riot = RiotClient::new()?.with_api_key(key);
            let result = async {
                let account = riot.account_by_riot_id(&riot_id, region).await?;
                riot.clash_matches(&account.puuid, region, clash::HISTORY_GAMES).await
            }
            .await;
            match result {
                Ok(games) => {
                    print_games("Riot API", &games);
                    api_games = games;
                }
                Err(e) => println!("\nRiot API: ошибка — {e}"),
            }
        }
    }

    let started = std::time::Instant::now();
    let summoner = lcu.summoner_by_riot_id(&riot_id).await?;
    let client_games = lcu.clash_matches(&summoner.puuid).await?;
    print_games(&format!("клиент League ({:.1} с)", started.elapsed().as_secs_f32()), &client_games);

    let merged = clash::merge_history(vec![api_games, client_games], clash::HISTORY_SHOWN);
    print_games("вместе", &merged);
    Ok(())
}

fn print_games(source: &str, games: &[MatchSummary]) {
    println!("\n{source}: игр Clash — {}", games.len());
    for day in clash::history_days(games) {
        println!("  день с {} · {}–{}", day.start(), day.wins, day.losses);
        for game in &day.games {
            println!(
                "    {} {} чемпион #{} {} {}",
                game.id,
                if game.win { "победа   " } else { "поражение" },
                game.me.champion_id,
                if game.me.position.is_empty() { "—" } else { &game.me.position },
                game.kda(),
            );
        }
    }
    let favorites: Vec<String> = clash::history_champions(games)
        .iter()
        .map(|(name, games, wins)| format!("{name} ×{games} ({wins} поб.)"))
        .collect();
    if !favorites.is_empty() {
        println!("  чаще всего: {}", favorites.join(", "));
    }
}
