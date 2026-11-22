//! Проверка вкладки «Клеш» из консоли: запускает фоновый поток приложения на временной
//! базе и печатает первое состояние турнира Clash, которое он соберёт из клиента League.
//!
//!   cargo run --example clash_check
//!
//! Нужен запущенный клиент League с выполненным входом. Только чтение (GET к клиенту);
//! ваша настоящая база и пароли не используются.

use std::time::{Duration, Instant};

use eframe::egui;
use lol_account_manager::backend::{Backend, BackendConfig};
use lol_account_manager::clash::{self, ClashHistory, ClashPlayer, ClashTeam, HistoryRequest};
use lol_account_manager::messages::{Command, Event};
use lol_account_manager::storage::{Secrets, Storage};

fn main() {
    let dir = std::env::temp_dir().join(format!("lolam-clash-check-{}", std::process::id()));
    let db = dir.join("accounts.db");
    std::fs::create_dir_all(&dir).expect("временная папка");
    let config = BackendConfig {
        open_storage: Box::new(move || Storage::open(&db, Secrets::with_service("lol-account-manager-clash-check"))),
        clipboard_clear_after: Duration::from_secs(30),
        image_cache_dir: dir.join("images"),
        update_dir: dir.join("updates"),
        auto_update_check: false,
    };
    let mut backend = Backend::start_with(egui::Context::default(), config);

    // Первое событие может прийти, пока ранги ещё грузятся, — ждём заполненное.
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut state = None;
    while Instant::now() < deadline {
        while let Some(event) = backend.try_recv() {
            if let Event::Clash(s) = event {
                let loaded = |t: &Option<ClashTeam>| {
                    t.as_ref().is_none_or(|t| t.players.iter().all(|p| p.riot_id.is_some()))
                };
                let done = loaded(&s.team) && loaded(&s.opponent);
                state = Some(s);
                if done {
                    break;
                }
            }
        }
        if state.as_ref().is_some_and(|s| s.team.is_some() || s.problem.is_some()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // `-- --history`: окно «прошлые Clash» для каждого игрока соперника (как по щелчку на ник).
    if std::env::args().any(|a| a == "--history")
        && let Some(opponent) = state.as_ref().and_then(|s| s.opponent.clone())
    {
        print_histories(&mut backend, &opponent);
    }
    drop(backend);
    let _ = std::fs::remove_dir_all(&dir);

    let Some(state) = state else {
        println!("Состояние Clash не пришло за 40 с");
        return;
    };
    println!(
        "Этап: {} · раунд {:?} · счёт {}–{} · сетка на {}",
        state.phase.label(),
        state.round,
        state.wins,
        state.losses,
        state.bracket_size
    );
    if let Some(problem) = &state.problem {
        println!("Проблема: {problem}");
    }
    if state.opponent_pending {
        println!("Соперник ещё не определился");
    }
    if state.schedule.is_empty() {
        println!("Расписание турниров: пусто");
    }
    for day in &state.schedule {
        println!(
            "Турнир {} «{}»: {}, начало {}, подтверждение {}{}",
            day.tournament_id,
            day.name,
            day.day_label(),
            day.start,
            day.lock_in,
            if day.registered { " · вы в команде" } else { "" }
        );
    }
    if let Some(opponent) = &state.opponent {
        let bans: Vec<String> = clash::ban_suggestions(opponent).iter().map(|b| b.label()).collect();
        let (main, reserve) = bans.split_at(bans.len().min(clash::BAN_SUGGESTIONS));
        println!("Кого банить: {}", if main.is_empty() { "—".to_string() } else { main.join(" · ") });
        if !reserve.is_empty() {
            println!("Запасные: {}", reserve.join(" · "));
        }
        for role in clash::role_bans(opponent) {
            let bans: Vec<String> = role.bans.iter().map(|b| format!("{} ({})", b.champion, b.stats())).collect();
            println!(
                "  {:<10} {}{} · {}",
                role.role.label(),
                role.player.as_deref().unwrap_or("—"),
                if role.unsure { " ?" } else { "" },
                bans.join(", ")
            );
        }
    }
    for (title, team) in [("Соперник", &state.opponent), ("Ваша команда", &state.team)] {
        if let Some(team) = team {
            println!("\n{title}: {} [{}]", team.name, team.short_name);
            for player in &team.players {
                print_player(player);
            }
        }
    }
}

/// Прошлые Clash игроков команды через фоновый поток приложения. Ключа Riot API у этой
/// временной копии нет — видны клиент League и сегодняшние раунды (API проверяет
/// clash_history_check).
fn print_histories(backend: &mut Backend, team: &ClashTeam) {
    let mut waiting = 0;
    for player in &team.players {
        if let Some(riot_id) = player.riot_id.clone() {
            backend.send(Command::LoadClashHistory(HistoryRequest {
                riot_id,
                puuid: player.puuid.clone(),
                roster_id: team.roster_id,
                summoner_id: player.summoner_id,
            }));
            waiting += 1;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    while waiting > 0 && Instant::now() < deadline {
        while let Some(event) = backend.try_recv() {
            if let Event::ClashHistoryLoaded { riot_id, result } = event {
                waiting -= 1;
                match result {
                    Err(e) => println!("\nПрошлые Clash {riot_id}: ошибка — {e}"),
                    Ok(history) => {
                        println!("\nПрошлые Clash {riot_id}: игр {}", history.games.len());
                        for game in &history.games {
                            println!(
                                "  {} {} {} {} {}",
                                game.id,
                                if game.win { "победа   " } else { "поражение" },
                                game.me.champion,
                                if game.me.position.is_empty() { "—" } else { &game.me.position },
                                if ClashHistory::is_brief(game) { "без k/d/a".to_string() } else { game.kda() }
                            );
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn print_player(p: &ClashPlayer) {
    let name = p.riot_id.as_ref().map_or("(не загрузился)".to_string(), ToString::to_string);
    let position = p.position.map_or("любая", |p| p.label());
    let solo = p
        .profile
        .as_ref()
        .and_then(|pr| pr.ranked.solo.as_ref())
        .map_or("без ранга".to_string(), |s| format!("{:?} {:?} {} LP", s.tier, s.division, s.lp));
    let champions: Vec<String> = p.champions.iter().map(|c| format!("{} ({})", c.name, c.games)).collect();
    println!(
        "  {position:<10} {name}{}{} · {solo} · {}",
        if p.is_captain { " ★" } else { "" },
        if p.is_self { " (вы)" } else { "" },
        champions.join(", ")
    );
}
