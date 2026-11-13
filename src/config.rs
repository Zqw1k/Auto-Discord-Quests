use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// User facing behaviour settings. The Discord token is deliberately never
/// stored: it only lives in memory for the duration of the process.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// How many seconds of video progress to report per interval.
    pub video_speed: f64,
    /// Seconds between video-progress requests.
    pub video_interval: u64,
    /// When enabled, video progress is capped to roughly real playback speed.
    pub safe_pacing: bool,
    /// Automatically claim rewards once a quest completes.
    pub auto_claim: bool,
    /// Refresh the quest list when the window regains focus.
    pub refresh_on_focus: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            video_speed: 5.0,
            video_interval: 5,
            safe_pacing: false,
            auto_claim: true,
            refresh_on_focus: true,
        }
    }
}

impl Settings {
    pub fn clamp(&mut self) {
        self.video_speed = self.video_speed.clamp(1.0, 30.0);
        self.video_interval = self.video_interval.clamp(2, 30);
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("settings.json")
}

fn config_dir() -> PathBuf {
    if cfg!(windows) {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("QuestPilot");
        }
    }
    if cfg!(target_os = "macos") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("QuestPilot");
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("QuestPilot");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join("QuestPilot");
    }
    PathBuf::from(".questpilot")
}

pub fn load_settings() -> Settings {
    let mut settings = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<Settings>(&raw).ok())
        .unwrap_or_default();
    settings.clamp();
    settings
}

pub fn save_settings(settings: &Settings) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(path, raw);
    }
}
