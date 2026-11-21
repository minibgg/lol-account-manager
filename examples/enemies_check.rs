//! Проверка анализа противников из консоли: запускает фоновый поток приложения на временной
//! базе и печатает команду противника, когда в клиенте идёт загрузка или игра.
//!
//!   cargo run --example enemies_check
//!
//! Только чтение (GET к клиенту League); ваша настоящая база и пароли не используются.

use std::time::{Duration, Instant};

use eframe::egui;
use lol_account_manager::backend::{Backend, BackendConfig};
use lol_account_manager::lobby::LobbyState;
use lol_account_manager::messages::Event;
use lol_account_manager::storage::{Secrets, Storage};

fn main() {
    let dir = std::env::temp_dir().join(format!("lolam-enemies-check-{}", std::process::id()));
    let db = dir.join("accounts.db");
    std::fs::create_dir_all(&dir).expect("временная папка");
    let config = BackendConfig {
        open_storage: Box::new(move || Storage::open(&db, Secrets::with_service("lol-account-manager-enemies-check"))),
        clipboard_clear_after: Duration::from_secs(30),
        image_cache_dir: dir.join("images"),
        update_dir: dir.join("updates"),
        auto_update_check: false,
    };
    let mut backend = Backend::start_with(egui::Context::default(), config);

    // Профили и история догружаются за несколько опросов — ждём, пока загрузятся все.
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut last: Option<LobbyState> = None;
    'wait: while Instant::now() < deadline {
        while let Some(event) = backend.try_recv() {
            if let Event::Lobby(state) = event {
                let done = !state.enemies.is_empty()
                    && state.enemies.iter().all(|e| e.riot_id.is_some() && e.form.is_some());
                last = Some(state);
                if done {
                    break 'wait;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(backend);
    let _ = std::fs::remove_dir_all(&dir);

    let Some(state) = last else {
        println!("Состояние лобби не пришло за 40 с");
        return;
    };
    println!("Этап: {:?}, противников: {}", state.phase, state.enemies.len());
    for e in &state.enemies {
        let name = e.riot_id.as_ref().map_or("(не загрузился)".to_string(), ToString::to_string);
        let solo = e
            .profile
            .as_ref()
            .and_then(|p| p.ranked.solo.as_ref())
            .map_or("без ранга".to_string(), |s| format!("{:?} {:?} {} LP", s.tier, s.division, s.lp));
        let form = e.form.map_or("история не загрузилась".to_string(), |f| {
            format!(
                "на чемпионе {}/{} побед · всего {}% из {} · kda {:.1} · серия {:+}",
                f.champion_wins,
                f.champion_games,
                f.winrate().unwrap_or(0),
                f.games,
                f.kda(),
                f.streak
            )
        });
        println!(
            "  {:<10} {name} · {} · {solo} · {form}",
            e.position.map_or("—", |p| p.label()),
            e.champion.as_deref().unwrap_or("—"),
        );
    }
}
