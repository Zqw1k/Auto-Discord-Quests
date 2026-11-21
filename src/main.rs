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
        // sidecar. The app holds our stdin open; EOF means the app is gone, so
        // exit instead of lingering as an orphan process.
        std::thread::spawn(|| {
            use std::io::Read;
            let mut stdin = std::io::stdin();
            let mut buffer = [0u8; 64];
            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) | Err(_) => std::process::exit(0),
                    Ok(_) => {}
                }
            }
        });
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

    // Debug-only: show the account behind every token found on this device.
    #[cfg(debug_assertions)]
    if std::env::args().any(|arg| arg == "--detect-accounts") {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        for found in discord::token::detect() {
            let user = runtime.block_on(async {
                discord::DiscordApi::new(&found.token)
                    .ok()?
                    .get_me()
                    .await
                    .ok()
            });
            match user {
                Some(user) => println!(
                    "{}: {} ({}) avatar={}",
                    found.label,
                    user.handle(),
                    user.display_name(),
                    user.avatar_url(64)
                ),
                None => println!("{}: token is no longer valid", found.label),
            }
        }
        return Ok(());
    }

    // Debug-only: exercise the DevTools sign-in from a terminal. This closes
    // and relaunches the Discord client.
    #[cfg(debug_assertions)]
    if std::env::args().any(|arg| arg == "--cdp-signin") {
        match discord::cdp::sign_in(|status| println!("… {status}")) {
            Ok(token) => {
                let preview: String = token.chars().take(6).collect();
                println!("token: {preview}… ({} chars)", token.len());
            }
            Err(error) => println!("error: {error}"),
        }
        return Ok(());
    }

    if !claim_single_instance() {
        #[cfg(windows)]
        focus_existing_window();
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

/// Only one window may run at a time. A second copy would spawn its own worker
/// and could leave a quest running with no visible controls, so the first
/// process holds a named mutex until it exits.
fn claim_single_instance() -> bool {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn CreateMutexW(
                attributes: *const std::ffi::c_void,
                initial_owner: i32,
                name: *const u16,
            ) -> *mut std::ffi::c_void;
            fn GetLastError() -> u32;
        }

        const ERROR_ALREADY_EXISTS: u32 = 183;

        let name: Vec<u16> = "Local\\QuestPilot-SingleInstance\0"
            .encode_utf16()
            .collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return true;
        }
        // The handle is intentionally never closed: it is released when the
        // process exits, which is exactly when another copy may start.
        unsafe { GetLastError() != ERROR_ALREADY_EXISTS }
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Brings the window of the already-running copy to the front.
#[cfg(windows)]
fn focus_existing_window() {
    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> *mut std::ffi::c_void;
        fn ShowWindow(window: *mut std::ffi::c_void, command: i32) -> i32;
        fn SetForegroundWindow(window: *mut std::ffi::c_void) -> i32;
    }

    const SW_RESTORE: i32 = 9;

    let title: Vec<u16> = "QuestPilot\0".encode_utf16().collect();
    let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
    if window.is_null() {
        return;
    }
    unsafe {
        ShowWindow(window, SW_RESTORE);
        SetForegroundWindow(window);
    }
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
