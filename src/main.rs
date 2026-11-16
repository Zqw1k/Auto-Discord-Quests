#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod discord;
mod models;
mod theme;
mod worker;

use eframe::egui;

fn main() -> eframe::Result<()> {
    if let Some(seconds) = game_runner_seconds() {
        // Hidden helper mode: keep a real process alive while a play-time quest
        // is ticking, the same trick discord-quest-helper uses with its runner
        // sidecar. The parent kills this process when the quest ends.
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        return Ok(());
    }

    #[cfg(debug_assertions)]
    if std::env::args().any(|arg| arg == "--detect-tokens") {
        for found in discord::token::detect() {
            let preview: String = found.token.chars().take(6).collect();
            println!("{}: {preview}… ({} chars)", found.label, found.token.len());
        }
        return Ok(());
    }

    let settings = config::load_settings();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 720.0])
            .with_min_inner_size([760.0, 560.0])
            .with_title("QuestPilot")
            .with_icon(theme::app_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "QuestPilot",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, settings)))),
    )
}

/// `questpilot --game-runner <seconds>` runs the dummy play-time process. It has
/// no UI and exits on its own if the parent disappears.
fn game_runner_seconds() -> Option<u64> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--game-runner") {
        return None;
    }
    args.next()?
        .parse::<u64>()
        .ok()
        .map(|value| value.clamp(1, 86_400))
}
