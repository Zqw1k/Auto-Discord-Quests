#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod discord;
mod models;
mod theme;
mod worker;

use eframe::egui;

fn main() -> eframe::Result<()> {
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
