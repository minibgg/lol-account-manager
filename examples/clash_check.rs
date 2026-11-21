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
use lol_account_manager::clash::{ClashPlayer, ClashTeam};
use lol_account_manager::messages::Event;
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
    for (title, team) in [("Соперник", &state.opponent), ("Ваша команда", &state.team)] {
        if let Some(team) = team {
            println!("\n{title}: {} [{}]", team.name, team.short_name);
            for player in &team.players {
                print_player(player);
            }
        }
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
