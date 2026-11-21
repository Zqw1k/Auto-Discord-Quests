use crate::config::{self, Settings};
use crate::models::{format_remaining, format_seconds, Quest, User};
use crate::theme::{
    self, tint, Icon, ACCENT, ACCENT_LIGHT, ACTIVITY, BG, BORDER, BORDER_SOFT, CARD, CARD_HOVER,
    DANGER, GAME, INPUT, MUTED, STREAM, SUCCESS, SURFACE, TEXT, TEXT_SOFT, VIDEO, WARNING,
};
use crate::worker::{self, Command, Event, Handle, LogLevel};
use chrono::Local;
use eframe::egui::{
    self, Align, Align2, Button, Color32, CornerRadius, Frame, Key, Layout, Margin, Pos2, Rect,
    RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit, Ui, Vec2,
};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Quests,
    Settings,
    Logs,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Open,
    Done,
}

/// Reward buckets mirror what Discord puts in `rewards_config` (`Nitro`,
/// `Orbs`, `Avatar Decoration`, …). Anything unknown lands in `Other`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RewardFilter {
    Any,
    Nitro,
    Orbs,
    Decoration,
    Other,
}

impl RewardFilter {
    fn label(self) -> &'static str {
        match self {
            Self::Any => "Any reward",
            Self::Nitro => "Nitro",
            Self::Orbs => "Orbs",
            Self::Decoration => "Decoration",
            Self::Other => "Other",
        }
    }

    fn matches(self, reward: &str) -> bool {
        self == Self::Any || reward_kind(reward) == self
    }
}

/// Session kept in RAM so you can hop between accounts without retyping the
/// token. Nothing is written to disk.
#[derive(Clone)]
struct SavedAccount {
    token: String,
    user: User,
}

#[derive(Clone, Copy)]
enum ToastKind {
    Success,
    Error,
    Info,
}

struct Toast {
    text: String,
    kind: ToastKind,
    born: Instant,
}

struct LogEntry {
    time: String,
    level: LogLevel,
    text: String,
}

/// Messages from the background CDP sign-in thread.
enum CdpEvent {
    Status(String),
    Done(Result<String, String>),
}

pub struct App {
    worker: Handle,
    token: String,
    pending_token: String,
    accounts: Vec<SavedAccount>,
    show_token: bool,
    user: Option<User>,
    quests: Vec<Quest>,
    quests_error: Option<String>,
    quests_note: Option<String>,
    settings: Settings,
    view: View,
    filter: Filter,
    reward_filter: RewardFilter,
    search: String,
    logs: Vec<LogEntry>,
    running: Option<String>,
    /// True between pressing Stop and the worker reporting the quest as
    /// finished, so the button cannot be pressed twice.
    stopping: bool,
    shown_progress: HashMap<String, f32>,
    logging_in: bool,
    auth_error: Option<String>,
    cdp_rx: Option<Receiver<CdpEvent>>,
    cdp_status: Option<String>,
    confirm_cdp: bool,
    confirm_logout: bool,
    avatars: HashMap<String, egui::TextureHandle>,
    avatar_requests: std::collections::HashSet<String>,
    toast: Option<Toast>,
    show_help: bool,
    was_focused: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, settings: Settings) -> Self {
        theme::setup(&cc.egui_ctx);
        let worker = worker::spawn(cc.egui_ctx.clone(), settings.clone());
        let mut app = Self {
            worker,
            token: String::new(),
            pending_token: String::new(),
            accounts: Vec::new(),
            show_token: false,
            user: None,
            quests: Vec::new(),
            quests_error: None,
            quests_note: None,
            settings,
            view: View::Quests,
            filter: Filter::All,
            reward_filter: RewardFilter::Any,
            search: String::new(),
            logs: Vec::new(),
            running: None,
            stopping: false,
            shown_progress: HashMap::new(),
            logging_in: false,
            auth_error: None,
            cdp_rx: None,
            cdp_status: None,
            confirm_cdp: false,
            confirm_logout: false,
            avatars: HashMap::new(),
            avatar_requests: std::collections::HashSet::new(),
            toast: None,
            show_help: false,
            was_focused: false,
        };
        app.seed_demo();
        app
    }

    /// Development helper: `QUESTPILOT_DEMO=1 cargo run` opens the main UI with
    /// sample data so the interface can be inspected without a token.
    #[cfg(debug_assertions)]
    fn seed_demo(&mut self) {
        let Ok(mode) = std::env::var("QUESTPILOT_DEMO") else {
            return;
        };
        if mode.is_empty() {
            return;
        }
        use crate::models::{Quest, QuestKind};
        let now = chrono::Utc::now();
        let sample = |id: &str,
                      name: &str,
                      app: &str,
                      reward: &str,
                      kind: QuestKind,
                      target: f64,
                      progress: f64,
                      days: i64| Quest {
            id: id.into(),
            name: name.into(),
            description: format!("{name} · sample quest for previewing the UI."),
            app_name: app.into(),
            application_id: "123456789012345678".into(),
            reward: reward.into(),
            kind,
            target,
            progress,
            enrolled: true,
            completed: false,
            claimed: false,
            expires_at: Some(now + chrono::Duration::days(days)),
        };
        self.user = Some(User {
            id: "1".into(),
            username: "questpilot".into(),
            global_name: Some("QuestPilot".into()),
            discriminator: "0".into(),
            avatar: None,
        });
        self.quests = vec![
            sample(
                "1",
                "Watch the latest trailer",
                "YouTube",
                "Orbs",
                QuestKind::Video,
                300.0,
                186.0,
                6,
            ),
            sample(
                "2",
                "Clean Crew",
                "Roblox",
                "Avatar Decoration",
                QuestKind::Game,
                900.0,
                240.0,
                11,
            ),
            sample(
                "3",
                "Cloud Raiders",
                "Fortnite",
                "Orbs",
                QuestKind::Activity,
                600.0,
                600.0,
                3,
            ),
            sample(
                "4",
                "Stream your gameplay",
                "Discord",
                "Nitro",
                QuestKind::Stream,
                1800.0,
                0.0,
                9,
            ),
        ];
        for quest in &mut self.quests {
            if quest.progress >= quest.target {
                quest.completed = true;
            }
        }
        if std::env::var("QUESTPILOT_DEMO_VIEW").as_deref() == Ok("empty") {
            self.quests.clear();
            self.quests_note = Some("Discord returned no available quests (12 excluded).".into());
        }
        if std::env::var("QUESTPILOT_DEMO_VIEW").as_deref() == Ok("help") {
            self.show_help = true;
        }
        if std::env::var("QUESTPILOT_DEMO_VIEW").as_deref() == Ok("logout") {
            self.confirm_logout = true;
        }
        if std::env::var("QUESTPILOT_DEMO_VIEW").as_deref() == Ok("login") {
            self.user = None;
            self.accounts = vec![
                SavedAccount {
                    token: "demo-token-one".into(),
                    user: User {
                        id: "1".into(),
                        username: "questpilot".into(),
                        global_name: Some("QuestPilot".into()),
                        discriminator: "0".into(),
                        avatar: None,
                    },
                },
                SavedAccount {
                    token: "demo-token-two".into(),
                    user: User {
                        id: "2".into(),
                        username: "night_owl".into(),
                        global_name: Some("Night Owl".into()),
                        discriminator: "0".into(),
                        avatar: None,
                    },
                },
            ];
        }
        self.logs.push(LogEntry {
            time: "16:20:01".into(),
            level: LogLevel::Info,
            text: "Loaded 4 quests (3 available).".into(),
        });
        self.logs.push(LogEntry {
            time: "16:20:05".into(),
            level: LogLevel::Success,
            text: "Connected as @questpilot".into(),
        });
        match std::env::var("QUESTPILOT_DEMO_VIEW").as_deref() {
            Ok("settings") => self.view = View::Settings,
            Ok("logs") => self.view = View::Logs,
            _ => self.view = View::Quests,
        }
    }

    #[cfg(not(debug_assertions))]
    fn seed_demo(&mut self) {}

    fn poll_events(&mut self, context: &egui::Context) {
        self.poll_cdp();
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::Log(level, text) => self.push_log(level, text),
                Event::AuthResult(result) => {
                    self.logging_in = false;
                    match result {
                        Ok(user) => {
                            let token = std::mem::take(&mut self.pending_token);
                            self.remember_account(&token, &user);
                            self.token.clear();
                            self.user = Some(user);
                            self.auth_error = None;
                            self.view = View::Quests;
                            self.toast("Account connected.", ToastKind::Success);
                        }
                        Err(error) => {
                            self.auth_error = Some(error.clone());
                            self.toast(error, ToastKind::Error);
                        }
                    }
                }
                Event::Accounts(found) => {
                    let mut added = 0;
                    for account in found {
                        if self
                            .accounts
                            .iter()
                            .any(|existing| existing.token == account.token)
                        {
                            continue;
                        }
                        self.accounts.push(SavedAccount {
                            token: account.token,
                            user: account.user,
                        });
                        added += 1;
                    }
                    if added == 0 {
                        self.toast("No new accounts found.", ToastKind::Info);
                    } else {
                        self.toast(
                            format!("Found {added} account(s) on this device."),
                            ToastKind::Success,
                        );
                    }
                }
                Event::Avatar { url, size, pixels } => {
                    let image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                    let texture =
                        context.load_texture(url.clone(), image, egui::TextureOptions::LINEAR);
                    self.avatars.insert(url, texture);
                }
                Event::Quests { quests, note } => {
                    self.quests = quests;
                    self.quests_error = None;
                    self.quests_note = note;
                    let ids: Vec<String> =
                        self.quests.iter().map(|quest| quest.id.clone()).collect();
                    self.shown_progress.retain(|id, _| ids.contains(id));
                }
                Event::QuestsFailed(error) => {
                    self.quests_error = Some(error.clone());
                    if self.user.is_some() {
                        self.toast(error, ToastKind::Error);
                    }
                }
                Event::Running(id) => {
                    self.running = id;
                    self.stopping = false;
                }
                Event::Progress {
                    quest_id,
                    percent,
                    seconds,
                    target,
                } => {
                    if let Some(quest) = self.quests.iter_mut().find(|quest| quest.id == quest_id) {
                        quest.progress = seconds;
                        quest.target = target;
                    }
                    self.shown_progress
                        .insert(quest_id, (percent / 100.0).clamp(0.0, 1.0) as f32);
                }
                Event::Done { quest_id, claimed } => {
                    let name = self
                        .quests
                        .iter()
                        .find(|quest| quest.id == quest_id)
                        .map(|quest| quest.name.clone())
                        .unwrap_or_else(|| "Quest".into());
                    if claimed {
                        self.toast(
                            format!("\"{name}\" completed and reward claimed."),
                            ToastKind::Success,
                        );
                    } else {
                        self.toast(format!("\"{name}\" completed."), ToastKind::Success);
                    }
                }
                Event::Failed(error) => {
                    self.toast(error, ToastKind::Error);
                }
                Event::Stopped => {
                    self.toast("Quest stopped.", ToastKind::Info);
                }
                Event::Claimed => {
                    self.toast("Reward claimed.", ToastKind::Success);
                }
                Event::ClaimFailed(error) => {
                    self.toast(error, ToastKind::Error);
                }
            }
        }
        if self.logging_in {
            context.request_repaint_after(Duration::from_millis(120));
        }
        self.request_avatars();
    }

    /// Draws the real profile picture when it has been downloaded, otherwise a
    /// coloured circle with the account's initial.
    fn avatar(&self, ui: &mut Ui, user: &User, size: f32) {
        let url = user.avatar_url(64);
        if let Some(texture) = self.avatars.get(&url) {
            let source = egui::load::SizedTexture::new(texture.id(), Vec2::splat(size));
            let radius = CornerRadius::same((size / 2.0).round() as u8);
            ui.add(egui::Image::new(source).corner_radius(radius));
            return;
        }
        fallback_avatar(ui, user, size);
    }

    /// Asks the worker for any profile picture the window is missing. The
    /// results arrive as `Event::Avatar` and become textures.
    fn request_avatars(&mut self) {
        let mut wanted: Vec<String> = Vec::new();
        if let Some(user) = &self.user {
            wanted.push(user.avatar_url(64));
        }
        for account in &self.accounts {
            wanted.push(account.user.avatar_url(64));
        }
        for url in wanted {
            if self.avatars.contains_key(&url) || !self.avatar_requests.insert(url.clone()) {
                continue;
            }
            self.worker.send(Command::FetchAvatar(url));
        }
    }

    fn push_log(&mut self, level: LogLevel, text: String) {
        self.logs.push(LogEntry {
            time: Local::now().format("%H:%M:%S").to_string(),
            level,
            text,
        });
        if self.logs.len() > 400 {
            self.logs.remove(0);
        }
    }

    fn toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast {
            text: text.into(),
            kind,
            born: Instant::now(),
        });
    }

    fn submit_login(&mut self) {
        if self.logging_in {
            return;
        }
        let token = self.token.trim().to_string();
        if token.len() < 20 {
            self.auth_error = Some("Invalid or too-short token.".into());
            return;
        }
        self.auth_error = None;
        self.logging_in = true;
        self.pending_token = token.clone();
        self.worker.send(Command::Login(token));
    }

    /// Runs the CDP sign-in on a background thread so the window keeps
    /// repainting while Discord restarts.
    fn start_cdp_login(&mut self) {
        if self.cdp_rx.is_some() {
            return;
        }
        self.confirm_cdp = false;
        self.auth_error = None;
        self.cdp_status = Some("Closing Discord…".into());

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let progress = |text: &str| {
                let _ = sender.send(CdpEvent::Status(text.to_string()));
            };
            let result = crate::discord::cdp::sign_in(progress);
            let _ = sender.send(CdpEvent::Done(result));
        });
        self.cdp_rx = Some(receiver);
    }

    fn poll_cdp(&mut self) {
        let Some(receiver) = self.cdp_rx.take() else {
            return;
        };
        let mut keep = true;
        loop {
            match receiver.try_recv() {
                Ok(CdpEvent::Status(text)) => self.cdp_status = Some(text),
                Ok(CdpEvent::Done(result)) => {
                    keep = false;
                    self.cdp_status = None;
                    match result {
                        Ok(token) => {
                            self.token = token;
                            self.toast("Token read from the Discord app.", ToastKind::Success);
                            self.submit_login();
                        }
                        Err(error) => {
                            self.auth_error = Some(error.clone());
                            self.toast(error, ToastKind::Error);
                        }
                    }
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    keep = false;
                    self.cdp_status = None;
                    self.auth_error =
                        Some("The Discord sign-in helper stopped unexpectedly.".into());
                    break;
                }
            }
        }
        if keep {
            self.cdp_rx = Some(receiver);
        }
    }

    /// Keeps a working session around so switching accounts is one click. The
    /// list only lives in memory; nothing is persisted.
    fn remember_account(&mut self, token: &str, user: &User) {
        if token.is_empty() || self.accounts.iter().any(|account| account.token == token) {
            return;
        }
        self.accounts.push(SavedAccount {
            token: token.to_string(),
            user: user.clone(),
        });
    }

    /// Pulls tokens straight out of the local Discord profiles (Windows only)
    /// and asks the worker to look up the account behind each one.
    fn detect_accounts(&mut self) {
        let found = crate::discord::token::detect();
        if found.is_empty() {
            self.toast(
                if crate::discord::token::supported() {
                    "No Discord token found on this device."
                } else {
                    "Token detection is Windows-only for now."
                },
                ToastKind::Info,
            );
            return;
        }

        let pending: Vec<(String, String)> = found
            .into_iter()
            .filter(|detected| {
                !self
                    .accounts
                    .iter()
                    .any(|account| account.token == detected.token)
            })
            .map(|detected| (detected.token, detected.label))
            .collect();

        if pending.is_empty() {
            self.toast("Those accounts are already in the list.", ToastKind::Info);
            return;
        }

        self.toast("Checking the detected tokens…", ToastKind::Info);
        self.worker.send(Command::ResolveAccounts(pending));
    }

    fn switch_account(&mut self, index: usize) {
        let Some(account) = self.accounts.get(index) else {
            return;
        };
        self.token = account.token.clone();
        self.auth_error = None;
        self.submit_login();
    }

    fn logout(&mut self) {
        self.worker.send(Command::Logout);
        self.user = None;
        self.quests.clear();
        self.quests_error = None;
        self.quests_note = None;
        self.token.clear();
        self.auth_error = None;
        self.running = None;
        self.stopping = false;
        self.shown_progress.clear();
        self.view = View::Quests;
    }

    fn start_quest(&mut self, quest: Quest) {
        if self.running.is_some() {
            self.toast("Another quest is running. Stop it first.", ToastKind::Info);
            return;
        }
        self.worker
            .send(Command::Start(quest, self.settings.clone()));
    }

    /// Asks the worker to stop and marks the UI as busy until the quest's
    /// finish event arrives.
    fn stop_quest(&mut self) {
        if self.running.is_none() || self.stopping {
            return;
        }
        self.stopping = true;
        self.worker.send(Command::Stop);
    }

    fn start_all(&mut self) {
        let queue: Vec<Quest> = self
            .quests
            .iter()
            .filter(|quest| {
                !quest.completed && !quest.claimed && !quest.is_expired() && quest.kind.supported()
            })
            .cloned()
            .collect();

        if queue.is_empty() {
            self.toast("No quests can be started.", ToastKind::Info);
            return;
        }
        self.toast(format!("Queued {} quests.", queue.len()), ToastKind::Info);
        self.worker
            .send(Command::StartQueue(queue, self.settings.clone()));
    }

    fn save_settings(&mut self) {
        self.settings.clamp();
        config::save_settings(&self.settings);
        self.worker
            .send(Command::UpdateSettings(self.settings.clone()));
    }

    fn animate_progress(&mut self, context: &egui::Context) {
        let mut needs_repaint = false;
        for quest in &self.quests {
            let target = quest.ratio() as f32;
            let entry = self
                .shown_progress
                .entry(quest.id.clone())
                .or_insert(target);
            let delta = target - *entry;
            if delta.abs() > 0.002 {
                *entry += delta * 0.22;
                needs_repaint = true;
            } else {
                *entry = target;
            }
        }
        if needs_repaint {
            context.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn login_screen(&mut self, context: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG))
            .show(context, |ui| {
                let screen = ui.max_rect();
                theme::glow(
                    ui.painter(),
                    Pos2::new(screen.center().x, screen.top() + 40.0),
                    400.0,
                    tint(ACCENT, 44),
                );
                theme::glow(
                    ui.painter(),
                    Pos2::new(screen.center().x, screen.bottom() + 40.0),
                    400.0,
                    tint(ACTIVITY, 18),
                );

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(34.0);
                        ui.vertical_centered(|ui| {
                            ui.allocate_ui_with_layout(
                                Vec2::new(468.0, 0.0),
                                Layout::top_down(Align::Center),
                                |ui| {
                                    Frame::NONE
                                        .fill(SURFACE)
                                        .stroke(Stroke::new(1.0_f32, BORDER))
                                        .corner_radius(CornerRadius::same(20))
                                        .shadow(theme::panel_shadow())
                                        .inner_margin(Margin::same(28))
                                        .show(ui, |ui| {
                                            ui.set_width(408.0);
                                            self.login_form(ui);
                                        });
                                },
                            );
                            ui.add_space(18.0);
                            ui.label(
                                RichText::new("Rust + egui · Your token never leaves this device")
                                    .font(theme::regular(11.0))
                                    .color(MUTED),
                            );
                            ui.add_space(18.0);
                        });
                    });
            });

        self.help_window(context);
    }

    fn login_form(&mut self, ui: &mut Ui) {
        let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(58.0), Sense::hover());
        ui.painter()
            .rect_filled(logo_rect, CornerRadius::same(17), ACCENT);
        theme::paint_icon(
            ui.painter(),
            logo_rect.shrink(15.0),
            Icon::Bolt,
            Color32::WHITE,
        );

        ui.add_space(14.0);
        ui.label(
            RichText::new("QuestPilot")
                .font(theme::bold(29.0))
                .color(TEXT),
        );
        ui.add_space(2.0);
        ui.label(
            RichText::new("Automate Discord Quests, clean and fast.")
                .font(theme::regular(13.0))
                .color(TEXT_SOFT),
        );

        if !self.accounts.is_empty() {
            ui.add_space(16.0);
            ui.label(
                RichText::new("Saved accounts")
                    .font(theme::medium(12.5))
                    .color(TEXT_SOFT),
            );
            ui.add_space(7.0);

            let accounts: Vec<SavedAccount> = self.accounts.clone();
            let mut connect = None;
            let mut remove = None;

            for (index, account) in accounts.iter().enumerate() {
                Frame::NONE
                    .fill(INPUT)
                    .stroke(Stroke::new(1.0_f32, BORDER_SOFT))
                    .corner_radius(CornerRadius::same(11))
                    .inner_margin(Margin::symmetric(10, 5))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            self.avatar(ui, &account.user, 26.0);
                            ui.add_space(4.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(account.user.display_name())
                                        .font(theme::semibold(12.0))
                                        .color(TEXT),
                                );
                                ui.label(
                                    RichText::new(account.user.handle())
                                        .font(theme::regular(10.0))
                                        .color(MUTED),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icon_button(ui, Icon::Close, 24.0, "Remove from this list")
                                    .clicked()
                                {
                                    remove = Some(index);
                                }
                                ui.add_space(4.0);
                                let connected = self
                                    .user
                                    .as_ref()
                                    .map(|user| user.id == account.user.id)
                                    .unwrap_or(false);
                                if connected {
                                    tag(ui, "Connected", SUCCESS, None);
                                } else if pill_button(
                                    ui,
                                    "Connect",
                                    Some(Icon::Play),
                                    ACCENT,
                                    Color32::WHITE,
                                    Stroke::NONE,
                                    0.0,
                                )
                                .clicked()
                                {
                                    connect = Some(index);
                                }
                            });
                        });
                    });
                ui.add_space(6.0);
            }

            if let Some(index) = connect {
                self.switch_account(index);
            }
            if let Some(index) = remove {
                self.accounts.remove(index);
            }

            ui.add_space(8.0);
            ui.label(
                RichText::new("Or sign in with another token")
                    .font(theme::regular(11.0))
                    .color(MUTED),
            );
        }

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Discord token")
                    .font(theme::medium(12.5))
                    .color(TEXT_SOFT),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(
                        Button::new(
                            RichText::new("How to get a token?")
                                .font(theme::medium(11.5))
                                .color(ACCENT_LIGHT),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.show_help = true;
                }
                if crate::discord::token::supported() {
                    ui.add_space(10.0);
                    if ui
                        .add(
                            Button::new(
                                RichText::new("Detect on this device")
                                    .font(theme::medium(11.5))
                                    .color(TEXT_SOFT),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        self.detect_accounts();
                    }
                }
            });
        });
        ui.add_space(6.0);

        Frame::NONE
            .fill(INPUT)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(CornerRadius::same(11))
            .inner_margin(Margin::symmetric(12, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let width = ui.available_width() - 42.0;
                    let response = ui.add_sized(
                        Vec2::new(width, 34.0),
                        TextEdit::singleline(&mut self.token)
                            .password(!self.show_token)
                            .hint_text("Paste your Discord token")
                            .frame(false)
                            .margin(Margin::symmetric(0, 8)),
                    );
                    if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                        self.submit_login();
                    }
                    let label = if self.show_token { "Hide" } else { "Show" };
                    if ui
                        .add(
                            Button::new(
                                RichText::new(label)
                                    .font(theme::medium(11.0))
                                    .color(TEXT_SOFT),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        self.show_token = !self.show_token;
                    }
                });
            });

        if let Some(error) = &self.auth_error {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                icon(ui, Icon::Alert, DANGER, 14.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(error)
                            .font(theme::regular(11.5))
                            .color(DANGER),
                    )
                    .wrap(),
                );
            });
        }

        ui.add_space(12.0);
        let enabled = !self.logging_in && !self.token.trim().is_empty();
        let response = ui.add_enabled(
            enabled,
            Button::new(
                RichText::new(if self.logging_in {
                    "Connecting…"
                } else {
                    "Connect"
                })
                .font(theme::semibold(14.0))
                .color(Color32::WHITE),
            )
            .fill(if enabled { ACCENT } else { CARD })
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(11))
            .min_size(Vec2::new(ui.available_width(), 42.0)),
        );
        if response.clicked() {
            self.submit_login();
        }
        if self.logging_in {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(13.0).color(ACCENT));
                ui.label(
                    RichText::new("Checking your token with Discord…")
                        .font(theme::regular(11.0))
                        .color(MUTED),
                );
            });
        }

        if crate::discord::cdp::supported() {
            ui.add_space(12.0);
            if self.cdp_rx.is_some() {
                Frame::NONE
                    .fill(tint(ACCENT, 14))
                    .stroke(Stroke::new(1.0_f32, tint(ACCENT, 45)))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(13.0).color(ACCENT));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(
                                        self.cdp_status
                                            .clone()
                                            .unwrap_or_else(|| "Talking to Discord…".into()),
                                    )
                                    .font(theme::regular(11.5))
                                    .color(TEXT_SOFT),
                                )
                                .wrap(),
                            );
                        });
                    });
            } else if self.confirm_cdp {
                Frame::NONE
                    .fill(tint(WARNING, 16))
                    .stroke(Stroke::new(1.0_f32, tint(WARNING, 55)))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            icon(ui, Icon::Alert, WARNING, 15.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(
                                        "QuestPilot will close Discord, reopen it with a local debugging port and read the token of the signed-in account. Finish anything you are doing in Discord first.",
                                    )
                                    .font(theme::regular(11.0))
                                    .color(Color32::from_rgb(224, 191, 137)),
                                )
                                .wrap(),
                            );
                        });
                        ui.add_space(9.0);
                        ui.horizontal(|ui| {
                            if pill_button(
                                ui,
                                "Restart Discord and continue",
                                Some(Icon::Refresh),
                                ACCENT,
                                Color32::WHITE,
                                Stroke::NONE,
                                0.0,
                            )
                            .clicked()
                            {
                                self.start_cdp_login();
                            }
                            ui.add_space(6.0);
                            if pill_button(
                                ui,
                                "Cancel",
                                None,
                                CARD,
                                TEXT_SOFT,
                                Stroke::new(1.0_f32, BORDER),
                                0.0,
                            )
                            .clicked()
                            {
                                self.confirm_cdp = false;
                            }
                        });
                    });
            } else {
                let response = ui.add_sized(
                    Vec2::new(ui.available_width(), 38.0),
                    Button::new(
                        RichText::new("Sign in with the Discord app")
                            .font(theme::medium(12.5))
                            .color(TEXT),
                    )
                    .fill(INPUT)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .corner_radius(CornerRadius::same(11)),
                );
                if response.clicked() {
                    self.confirm_cdp = true;
                }
                ui.add_space(5.0);
                ui.label(
                    RichText::new("Windows only · restarts Discord to read its token")
                        .font(theme::regular(10.5))
                        .color(MUTED),
                );
            }
        }

        ui.add_space(10.0);
        ui.label(
            RichText::new("The token is kept in RAM only and never written to disk.")
                .font(theme::regular(10.8))
                .color(MUTED),
        );

        ui.add_space(16.0);
        Frame::NONE
            .fill(tint(WARNING, 16))
            .stroke(Stroke::new(1.0_f32, tint(WARNING, 55)))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    icon(ui, Icon::Alert, WARNING, 15.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "This tool violates Discord's Terms of Service. Your account may lose quest access or be suspended. Use at your own risk.",
                            )
                            .font(theme::regular(11.0))
                            .color(Color32::from_rgb(224, 191, 137)),
                        )
                        .wrap(),
                    );
                });
            });
    }

    fn header(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .exact_height(64.0)
            .frame(Frame::NONE.fill(BG).inner_margin(Margin::symmetric(22, 0)))
            .show(context, |ui| {
                ui.horizontal_centered(|ui| {
                    let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    ui.painter()
                        .rect_filled(logo_rect, CornerRadius::same(9), ACCENT);
                    theme::paint_icon(
                        ui.painter(),
                        logo_rect.shrink(7.5),
                        Icon::Bolt,
                        Color32::WHITE,
                    );
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("QuestPilot")
                            .font(theme::semibold(16.5))
                            .color(TEXT),
                    );
                    ui.label(
                        RichText::new("v0.1")
                            .font(theme::regular(10.0))
                            .color(MUTED),
                    );
                    ui.add_space(24.0);

                    if nav_tab(ui, "Quests", self.view == View::Quests).clicked() {
                        self.view = View::Quests;
                    }
                    if nav_tab(ui, "Settings", self.view == View::Settings).clicked() {
                        self.view = View::Settings;
                    }
                    if nav_tab(ui, "Logs", self.view == View::Logs).clicked() {
                        self.view = View::Logs;
                    }

                    ui.add_space((ui.available_width() - 214.0).max(12.0));

                    if icon_button(ui, Icon::Refresh, 30.0, "Reload quest list").clicked() {
                        self.worker.send(Command::Refresh);
                    }
                    ui.add_space(4.0);

                    if let Some(user) = self.user.clone() {
                        Frame::NONE
                            .fill(tint(TEXT, 5))
                            .stroke(Stroke::new(1.0_f32, BORDER_SOFT))
                            .corner_radius(CornerRadius::same(19))
                            .inner_margin(Margin::symmetric(6, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    self.avatar(ui, &user, 26.0);
                                    ui.add_space(1.0);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            RichText::new(user.display_name())
                                                .font(theme::semibold(12.0))
                                                .color(TEXT),
                                        );
                                        ui.label(
                                            RichText::new(user.handle())
                                                .font(theme::regular(10.0))
                                                .color(MUTED),
                                        );
                                    });
                                    ui.add_space(4.0);
                                    if icon_button(ui, Icon::Logout, 26.0, "Sign out").clicked() {
                                        self.confirm_logout = true;
                                    }
                                });
                            });
                    }
                });
            });

        let rect = Rect::from_min_size(
            Pos2::new(context.screen_rect().left(), 63.0),
            Vec2::new(context.screen_rect().width(), 1.0),
        );
        context
            .layer_painter(egui::LayerId::new(
                egui::Order::Background,
                egui::Id::new("header-line"),
            ))
            .rect_filled(rect, 0, BORDER_SOFT);
    }

    fn footer(&mut self, context: &egui::Context) {
        let mut stop = false;
        egui::TopBottomPanel::bottom("footer")
            .exact_height(58.0)
            .frame(
                Frame::NONE
                    .fill(SURFACE)
                    .inner_margin(Margin::symmetric(22, 0)),
            )
            .show(context, |ui| {
                let rect = ui.max_rect();
                ui.painter().line_segment(
                    [rect.left_top(), rect.right_top()],
                    Stroke::new(1.0_f32, BORDER_SOFT),
                );

                ui.horizontal_centered(|ui| {
                    let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(
                        dot_rect.center(),
                        3.5,
                        if self.user.is_some() { SUCCESS } else { MUTED },
                    );
                    ui.label(
                        RichText::new(if self.user.is_some() {
                            "Connected"
                        } else {
                            "Signed out"
                        })
                        .font(theme::medium(11.5))
                        .color(TEXT_SOFT),
                    );

                    if let Some(running_id) = self.running.clone() {
                        if let Some(quest) = self.quests.iter().find(|quest| quest.id == running_id)
                        {
                            ui.add_space(16.0);
                            ui.add(egui::Spinner::new().size(12.0).color(ACCENT));
                            ui.label(
                                RichText::new(format!("Running: {}", quest.name))
                                    .font(theme::medium(11.5))
                                    .color(TEXT),
                            );
                            let ratio = self.shown_progress.get(&quest.id).copied().unwrap_or(0.0);
                            mini_progress(ui, ratio, ACCENT, 180.0);
                            ui.label(
                                RichText::new(format!("{:.0}%", ratio * 100.0))
                                    .font(theme::semibold(11.0))
                                    .color(TEXT_SOFT),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if self.stopping {
                                    tag(ui, "Stopping…", MUTED, None);
                                } else if pill_button(
                                    ui,
                                    "Stop",
                                    Some(Icon::Stop),
                                    tint(DANGER, 20),
                                    DANGER,
                                    Stroke::new(1.0_f32, tint(DANGER, 80)),
                                    0.0,
                                )
                                .clicked()
                                {
                                    stop = true;
                                }
                            });
                        }
                    } else {
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new("Ready")
                                .font(theme::regular(11.0))
                                .color(MUTED),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} quests · {}",
                                    self.quests.len(),
                                    self.settings_label()
                                ))
                                .font(theme::regular(10.5))
                                .color(MUTED),
                            );
                        });
                    }
                });
            });
        if stop {
            self.stop_quest();
        }
    }

    fn settings_label(&self) -> String {
        if self.settings.safe_pacing {
            "safe mode".into()
        } else {
            format!("speed {:.0}×", self.settings.video_speed)
        }
    }

    fn main_content(&mut self, context: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG))
            .show(context, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        let content_width = width.min(940.0);
                        let padding = ((width - content_width) / 2.0).max(0.0);
                        ui.horizontal_top(|ui| {
                            ui.add_space(padding);
                            ui.vertical(|ui| {
                                ui.set_width(content_width);
                                ui.add_space(24.0);
                                match self.view {
                                    View::Quests => self.quests_page(ui),
                                    View::Settings => self.settings_page(ui),
                                    View::Logs => self.logs_page(ui, context),
                                }
                                ui.add_space(28.0);
                            });
                        });
                    });
            });
    }

    fn quests_page(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Quests").font(theme::bold(25.0)).color(TEXT));
                ui.add_space(1.0);
                ui.label(
                    RichText::new("Pick a quest and let QuestPilot handle the rest.")
                        .font(theme::regular(12.5))
                        .color(TEXT_SOFT),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let has_open = self.quests.iter().any(|quest| {
                    !quest.completed
                        && !quest.claimed
                        && !quest.is_expired()
                        && quest.kind.supported()
                });
                if pill_button(
                    ui,
                    "Do all",
                    Some(Icon::Play),
                    if has_open { ACCENT } else { CARD },
                    if has_open { Color32::WHITE } else { MUTED },
                    Stroke::NONE,
                    0.0,
                )
                .clicked()
                    && has_open
                {
                    self.start_all();
                }
            });
        });

        ui.add_space(18.0);
        ui.horizontal(|ui| {
            if filter_pill(ui, "All", self.filter == Filter::All, self.quests.len()).clicked() {
                self.filter = Filter::All;
            }
            let open = self
                .quests
                .iter()
                .filter(|quest| !quest.completed && !quest.claimed)
                .count();
            if filter_pill(ui, "Open", self.filter == Filter::Open, open).clicked() {
                self.filter = Filter::Open;
            }
            let done = self
                .quests
                .iter()
                .filter(|quest| quest.completed || quest.claimed)
                .count();
            if filter_pill(ui, "Done", self.filter == Filter::Done, done).clicked() {
                self.filter = Filter::Done;
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                Frame::NONE
                    .fill(INPUT)
                    .stroke(Stroke::new(1.0_f32, BORDER_SOFT))
                    .corner_radius(CornerRadius::same(9))
                    .inner_margin(Margin::symmetric(9, 2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            icon(ui, Icon::Search, MUTED, 13.0);
                            ui.add(
                                TextEdit::singleline(&mut self.search)
                                    .hint_text("Search quests…")
                                    .frame(false)
                                    .desired_width(160.0)
                                    .margin(Margin::symmetric(0, 6)),
                            );
                        });
                    });
            });
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            for value in [
                RewardFilter::Any,
                RewardFilter::Nitro,
                RewardFilter::Orbs,
                RewardFilter::Decoration,
                RewardFilter::Other,
            ] {
                let count = self
                    .quests
                    .iter()
                    .filter(|quest| value.matches(&quest.reward))
                    .count();
                if filter_pill(ui, value.label(), self.reward_filter == value, count).clicked() {
                    self.reward_filter = value;
                }
            }
        });

        ui.add_space(14.0);

        let query = self.search.trim().to_lowercase();
        let indices: Vec<usize> = self
            .quests
            .iter()
            .enumerate()
            .filter(|(_, quest)| match self.filter {
                Filter::All => true,
                Filter::Open => !quest.completed && !quest.claimed,
                Filter::Done => quest.completed || quest.claimed,
            })
            .filter(|(_, quest)| self.reward_filter.matches(&quest.reward))
            .filter(|(_, quest)| {
                query.is_empty()
                    || quest.name.to_lowercase().contains(&query)
                    || quest.app_name.to_lowercase().contains(&query)
                    || quest.reward.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect();

        if indices.is_empty() {
            let (title, subtitle) = if let Some(error) = self.quests_error.clone() {
                ("Could not load quests", error)
            } else if self.quests.is_empty() {
                (
                    "Discord returned no quests",
                    self.quests_note.clone().unwrap_or_else(|| {
                        "Press reload to fetch the latest list from Discord.".to_string()
                    }),
                )
            } else {
                (
                    "No quests here",
                    "Change the filter or press reload to fetch the latest list from Discord."
                        .to_string(),
                )
            };
            self.empty_state(ui, title, &subtitle);
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                if pill_button(
                    ui,
                    "Reload",
                    Some(Icon::Refresh),
                    ACCENT,
                    Color32::WHITE,
                    Stroke::NONE,
                    0.0,
                )
                .clicked()
                {
                    self.worker.send(Command::Refresh);
                }
            });
            return;
        }

        for index in indices {
            let quest = self.quests[index].clone();
            self.quest_card(ui, &quest);
        }
    }

    fn empty_state(&mut self, ui: &mut Ui, title: &str, subtitle: &str) {
        Frame::NONE
            .fill(CARD)
            .stroke(Stroke::new(1.0_f32, BORDER_SOFT))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::symmetric(24, 46))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::hover());
                    ui.painter()
                        .circle_filled(rect.center(), 19.0, tint(ACCENT, 24));
                    theme::paint_icon(ui.painter(), rect.shrink(10.0), Icon::Sparkle, ACCENT_LIGHT);
                    ui.add_space(10.0);
                    ui.label(RichText::new(title).font(theme::semibold(14.0)).color(TEXT));
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(subtitle)
                            .font(theme::regular(11.5))
                            .color(MUTED),
                    );
                });
            });
    }

    fn quest_card(&mut self, ui: &mut Ui, quest: &Quest) {
        let kind_color = kind_color(quest.kind);
        let running = self.running.as_deref() == Some(quest.id.as_str());
        let busy = self.running.is_some() && !running;
        let ratio = self.shown_progress.get(&quest.id).copied().unwrap_or(0.0);

        let mut action: Option<Action> = None;

        let stroke = if running {
            Stroke::new(1.0_f32, tint(ACCENT, 150))
        } else {
            Stroke::new(1.0_f32, BORDER_SOFT)
        };

        let card = Frame::NONE
            .fill(if running { CARD_HOVER } else { CARD })
            .stroke(stroke)
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::symmetric(18, 15))
            .show(ui, |ui| {
                let width = ui.available_width();
                ui.horizontal(|ui| {
                    app_tile(ui, quest, kind_color, 44.0);

                    let right_width = 152.0;
                    ui.allocate_ui_with_layout(
                        Vec2::new((width - 44.0 - 12.0 - right_width).max(180.0), 44.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.horizontal(|ui| {
                                let title = RichText::new(&quest.name)
                                    .font(theme::semibold(14.5))
                                    .color(TEXT);
                                ui.add(egui::Label::new(title).truncate());
                                kind_chip(ui, quest);
                                if running {
                                    ui.add(egui::Spinner::new().size(11.0).color(ACCENT));
                                }
                            });
                            ui.add_space(2.0);
                            let subtitle = if quest.app_name.is_empty() {
                                quest.reward.clone()
                            } else {
                                format!("{} · {}", quest.app_name, quest.reward)
                            };
                            ui.add(
                                egui::Label::new(
                                    RichText::new(subtitle)
                                        .font(theme::regular(11.5))
                                        .color(TEXT_SOFT),
                                )
                                .truncate(),
                            );
                        },
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if quest.claimed {
                            tag(ui, "Claimed", SUCCESS, Some(Icon::Check));
                        } else if quest.completed {
                            if pill_button(
                                ui,
                                "Claim reward",
                                Some(Icon::Gift),
                                ACCENT,
                                Color32::WHITE,
                                Stroke::NONE,
                                0.0,
                            )
                            .clicked()
                            {
                                action = Some(Action::Claim(quest.id.clone()));
                            }
                        } else if quest.is_expired() {
                            tag(ui, "Expired", MUTED, Some(Icon::Clock));
                        } else if !quest.kind.supported() {
                            let response = tag(ui, "Unsupported", MUTED, None);
                            response.on_hover_text(quest.kind.hint());
                        } else if running {
                            if self.stopping {
                                tag(ui, "Stopping…", MUTED, None);
                            } else if pill_button(
                                ui,
                                "Stop",
                                Some(Icon::Stop),
                                tint(DANGER, 18),
                                DANGER,
                                Stroke::new(1.0_f32, tint(DANGER, 70)),
                                0.0,
                            )
                            .clicked()
                            {
                                action = Some(Action::Stop);
                            }
                        } else if busy {
                            tag(ui, "Busy", MUTED, None);
                        } else {
                            if pill_button(
                                ui,
                                "Start",
                                Some(Icon::Play),
                                ACCENT,
                                Color32::WHITE,
                                Stroke::NONE,
                                0.0,
                            )
                            .clicked()
                            {
                                action = Some(Action::Start(quest.clone()));
                            }
                        }
                    });
                });

                ui.add_space(12.0);
                progress_bar(ui, ratio, kind_color);

                ui.add_space(7.0);
                ui.horizontal(|ui| {
                    let progress_text = if quest.completed || quest.claimed {
                        "Completed".to_string()
                    } else if quest.target > 0.0 {
                        format!(
                            "{} / {} · {:.0}%",
                            format_seconds(quest.progress),
                            format_seconds(quest.target),
                            ratio * 100.0
                        )
                    } else {
                        quest.kind.hint().to_string()
                    };
                    ui.label(
                        RichText::new(progress_text)
                            .font(theme::regular(10.8))
                            .color(if quest.completed { SUCCESS } else { MUTED }),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let remaining = if quest.completed && !quest.claimed {
                            "Reward available".to_string()
                        } else {
                            format_remaining(quest.expires_at)
                        };
                        ui.label(
                            RichText::new(remaining)
                                .font(theme::regular(10.8))
                                .color(MUTED),
                        );
                        icon(ui, Icon::Clock, MUTED, 12.0);
                    });
                });
            });

        if !quest.description.is_empty() {
            card.response.on_hover_text(&quest.description);
        }

        match action {
            Some(Action::Start(quest)) => self.start_quest(quest),
            Some(Action::Claim(id)) => self.worker.send(Command::Claim(id)),
            Some(Action::Stop) => self.stop_quest(),
            None => {}
        }
        ui.add_space(10.0);
    }

    fn settings_page(&mut self, ui: &mut Ui) {
        let before = self.settings.clone();

        ui.label(
            RichText::new("Settings")
                .font(theme::bold(25.0))
                .color(TEXT),
        );
        ui.add_space(1.0);
        ui.label(
            RichText::new("Tune speed and how QuestPilot handles quests.")
                .font(theme::regular(12.5))
                .color(TEXT_SOFT),
        );
        ui.add_space(18.0);

        settings_card(ui, "Video speed", "Applies to WATCH_VIDEO quests.", |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Advance per tick")
                        .font(theme::medium(12.5))
                        .color(TEXT_SOFT),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{:.0} seconds", self.settings.video_speed))
                            .font(theme::semibold(12.0))
                            .color(TEXT),
                    );
                    ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(120.0);
                    ui.add(
                        egui::Slider::new(&mut self.settings.video_speed, 1.0..=30.0)
                            .show_value(false)
                            .step_by(1.0),
                    );
                });
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Delay between requests")
                        .font(theme::medium(12.5))
                        .color(TEXT_SOFT),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} seconds", self.settings.video_interval))
                            .font(theme::semibold(12.0))
                            .color(TEXT),
                    );
                    ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(120.0);
                    ui.add(
                        egui::Slider::new(&mut self.settings.video_interval, 2..=30)
                            .show_value(false),
                    );
                });
            });
            ui.add_space(12.0);
            toggle_row(
                ui,
                "Safe mode (1×)",
                "Caps progress to real time. Slower but more natural.",
                &mut self.settings.safe_pacing,
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Higher speeds finish faster, but skipping too quickly may look abnormal to Discord.",
                )
                .font(theme::regular(10.8))
                .color(MUTED),
            );
        });

        ui.add_space(12.0);
        settings_card(ui, "Automation", "Reduce the manual steps.", |ui| {
            toggle_row(
                ui,
                "Auto-claim rewards",
                "Claim the reward as soon as a quest completes.",
                &mut self.settings.auto_claim,
            );
            ui.add_space(10.0);
            toggle_row(
                ui,
                "Reload on window focus",
                "Refresh the quest list when you return to the app.",
                &mut self.settings.refresh_on_focus,
            );
        });

        ui.add_space(12.0);
        settings_card(
            ui,
            "How it works",
            "QuestPilot uses the same API as the Discord desktop client.",
            |ui| {
                mechanism_row(
                    ui,
                    VIDEO,
                    "Video",
                    "Sends video progress in timestamp steps with slight jitter to look natural.",
                );
                ui.add_space(9.0);
                mechanism_row(
                    ui,
                    GAME,
                    "Play game",
                    "Fetches app metadata, builds a valid executable, then sends a heartbeat every 60 seconds.",
                );
                ui.add_space(9.0);
                mechanism_row(
                    ui,
                    ACTIVITY,
                    "Activity",
                    "Sends activity heartbeats and reads progress from Discord.",
                );
                ui.add_space(9.0);
                mechanism_row(
                    ui,
                    STREAM,
                    "Stream",
                    "Unsupported: Discord requires a real stream with someone else in the voice channel.",
                );
            },
        );

        ui.add_space(12.0);
        Frame::NONE
            .fill(tint(WARNING, 14))
            .stroke(Stroke::new(1.0_f32, tint(WARNING, 45)))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    icon(ui, Icon::Alert, WARNING, 16.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Note")
                                .font(theme::semibold(12.5))
                                .color(WARNING),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(
                                    "Automating quests violates Discord's Terms of Service. The token stays in RAM only and is lost when the app closes. Never share your token.",
                                )
                                .font(theme::regular(11.2))
                                .color(Color32::from_rgb(221, 195, 150)),
                            )
                            .wrap(),
                        );
                    });
                });
            });

        if self.settings != before && !ui.ctx().input(|input| input.pointer.any_down()) {
            self.save_settings();
        }
    }

    fn logs_page(&mut self, ui: &mut Ui, context: &egui::Context) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Logs").font(theme::bold(25.0)).color(TEXT));
                ui.add_space(1.0);
                ui.label(
                    RichText::new("Follow every step QuestPilot takes.")
                        .font(theme::regular(12.5))
                        .color(TEXT_SOFT),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pill_button(
                    ui,
                    "Clear",
                    Some(Icon::Close),
                    CARD,
                    TEXT_SOFT,
                    Stroke::new(1.0_f32, BORDER),
                    0.0,
                )
                .clicked()
                {
                    self.logs.clear();
                }
                if pill_button(
                    ui,
                    "Copy",
                    None,
                    CARD,
                    TEXT_SOFT,
                    Stroke::new(1.0_f32, BORDER),
                    0.0,
                )
                .clicked()
                {
                    let text = self
                        .logs
                        .iter()
                        .map(|entry| format!("[{}] {:?}: {}", entry.time, entry.level, entry.text))
                        .collect::<Vec<_>>()
                        .join("\n");
                    context.copy_text(text);
                    self.toast("Logs copied.", ToastKind::Success);
                }
            });
        });

        ui.add_space(16.0);
        if self.logs.is_empty() {
            self.empty_state(ui, "No logs yet", "QuestPilot's steps will appear here.");
            return;
        }

        Frame::NONE
            .fill(CARD)
            .stroke(Stroke::new(1.0_f32, BORDER_SOFT))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                for entry in self.logs.iter().rev() {
                    let color = match entry.level {
                        LogLevel::Info => TEXT_SOFT,
                        LogLevel::Success => SUCCESS,
                        LogLevel::Warn => WARNING,
                        LogLevel::Error => DANGER,
                    };
                    ui.horizontal_top(|ui| {
                        ui.label(
                            RichText::new(&entry.time)
                                .font(egui::FontId::monospace(10.5))
                                .color(MUTED),
                        );
                        let (dot_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                        ui.painter().circle_filled(dot_rect.center(), 3.0, color);
                        ui.add(
                            egui::Label::new(
                                RichText::new(&entry.text).font(theme::regular(11.5)).color(
                                    if matches!(entry.level, LogLevel::Info) {
                                        TEXT_SOFT
                                    } else {
                                        color
                                    },
                                ),
                            )
                            .wrap(),
                        );
                    });
                    ui.add_space(2.0);
                }
            });
    }

    /// Signing out stops a running quest, so confirm first.
    fn logout_window(&mut self, context: &egui::Context) {
        if !self.confirm_logout {
            return;
        }
        let Some(user) = self.user.clone() else {
            self.confirm_logout = false;
            return;
        };

        let mut open = true;
        let mut sign_out = false;
        let mut cancel = false;

        egui::Window::new("Sign out")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(
                Frame::NONE
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .corner_radius(CornerRadius::same(16))
                    .shadow(theme::panel_shadow())
                    .inner_margin(Margin::same(22)),
            )
            .show(context, |ui| {
                ui.set_width(360.0);
                ui.horizontal(|ui| {
                    self.avatar(ui, &user, 42.0);
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("Sign out of {}?", user.display_name()))
                                .font(theme::semibold(14.5))
                                .color(TEXT),
                        );
                        ui.label(
                            RichText::new(user.handle())
                                .font(theme::regular(11.0))
                                .color(MUTED),
                        );
                    });
                });

                ui.add_space(12.0);
                let note = if self.running.is_some() {
                    "The running quest will be stopped. This account stays in the saved list and you can connect again with one click."
                } else {
                    "This account stays in the saved list, so you can connect again with one click."
                };
                ui.add(
                    egui::Label::new(
                        RichText::new(note).font(theme::regular(11.5)).color(TEXT_SOFT),
                    )
                    .wrap(),
                );

                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if pill_button(
                            ui,
                            "Sign out",
                            Some(Icon::Logout),
                            tint(DANGER, 20),
                            DANGER,
                            Stroke::new(1.0_f32, tint(DANGER, 80)),
                            0.0,
                        )
                        .clicked()
                        {
                            sign_out = true;
                        }
                        ui.add_space(6.0);
                        if pill_button(
                            ui,
                            "Cancel",
                            None,
                            tint(TEXT, 8),
                            TEXT_SOFT,
                            Stroke::new(1.0_f32, BORDER_SOFT),
                            0.0,
                        )
                        .clicked()
                        {
                            cancel = true;
                        }
                    });
                });
            });

        if sign_out {
            self.logout();
            self.confirm_logout = false;
        } else if cancel || !open {
            self.confirm_logout = false;
        }
    }

    fn help_window(&mut self, context: &egui::Context) {
        if !self.show_help {
            return;
        }
        let mut open = true;
        egui::Window::new("Get your Discord token")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(
                Frame::NONE
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .corner_radius(CornerRadius::same(16))
                    .shadow(theme::panel_shadow())
                    .inner_margin(Margin::same(22)),
            )
            .show(context, |ui| {
                ui.set_width(470.0);
                ui.label(
                    RichText::new("What is a token?")
                        .font(theme::semibold(14.0))
                        .color(TEXT),
                );
                ui.add_space(2.0);
                ui.label(
                    RichText::new(
                        "A token is the login key for your Discord account. It is not a bot token and must never be shared.",
                    )
                    .font(theme::regular(11.5))
                    .color(TEXT_SOFT),
                );

                ui.add_space(14.0);
                ui.label(
                    RichText::new("Option 1 · Discord desktop")
                        .font(theme::semibold(12.5))
                        .color(TEXT),
                );
                ui.add_space(4.0);
                for step in [
                    "Open Discord desktop and press Ctrl + Shift + I.",
                    "Switch to the Console tab, paste the snippet below, then press Enter.",
                    "Copy the token that appears and paste it into QuestPilot.",
                    "If it prints undefined or an object, reload Discord (Ctrl + R) and run it again.",
                ] {
                    ui.horizontal_top(|ui| {
                        let (dot, _) = ui.allocate_exact_size(Vec2::splat(6.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 2.5, ACCENT);
                        ui.add(
                            egui::Label::new(
                                RichText::new(step)
                                    .font(theme::regular(11.2))
                                    .color(TEXT_SOFT),
                            )
                            .wrap(),
                        );
                    });
                }

                ui.add_space(8.0);
                let snippet = r#"(() => { let t; webpackChunkdiscord_app.push([[Symbol()], {}, o => { for (const e of Object.values(o.c ?? {})) { try { const x = e.exports; if (!x || x === window) continue; for (const c of [x, ...Object.values(x)]) { try { const v = c?.getToken?.(); if (typeof v === "string" && v.length > 50 && !/\s/.test(v) && v.split(".").length > 1) { t = v; return; } } catch {} } } catch {} } }]); webpackChunkdiscord_app.pop(); return t })()"#;
                Frame::NONE
                    .fill(INPUT)
                    .stroke(Stroke::new(1.0_f32, BORDER_SOFT))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(snippet)
                                        .font(egui::FontId::monospace(9.5))
                                        .color(TEXT_SOFT),
                                )
                                .wrap(),
                            );
                        });
                    });
                if pill_button(
                    ui,
                    "Copy snippet",
                    None,
                    tint(ACCENT, 28),
                    ACCENT_LIGHT,
                    Stroke::new(1.0_f32, tint(ACCENT, 80)),
                    0.0,
                )
                .clicked()
                {
                    context.copy_text(snippet.to_string());
                }

                ui.add_space(14.0);
                ui.label(
                    RichText::new("Option 2 · DevTools Network")
                        .font(theme::semibold(12.5))
                        .color(TEXT),
                );
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "Open DevTools → Network → select a request to discord.com/api → check the Authorization header. That value is your token.",
                        )
                        .font(theme::regular(11.2))
                        .color(TEXT_SOFT),
                    )
                    .wrap(),
                );

                ui.add_space(14.0);
                Frame::NONE
                    .fill(tint(WARNING, 14))
                    .stroke(Stroke::new(1.0_f32, tint(WARNING, 45)))
                    .corner_radius(CornerRadius::same(9))
                    .inner_margin(Margin::symmetric(10, 9))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(
                                    "Never paste your token into untrusted sites. QuestPilot sends it only to discord.com.",
                                )
                                .font(theme::regular(10.8))
                                .color(Color32::from_rgb(224, 191, 137)),
                            )
                            .wrap(),
                        );
                    });
            });
        self.show_help = open;
    }

    fn toast_ui(&mut self, context: &egui::Context) {
        let Some(toast) = &self.toast else {
            return;
        };
        if toast.born.elapsed() > Duration::from_secs(5) {
            self.toast = None;
            return;
        }
        let (color, icon_kind) = match toast.kind {
            ToastKind::Success => (SUCCESS, Icon::Check),
            ToastKind::Error => (DANGER, Icon::Alert),
            ToastKind::Info => (ACCENT_LIGHT, Icon::Sparkle),
        };
        let text = toast.text.clone();

        egui::Area::new(egui::Id::new("questpilot-toast"))
            .order(egui::Order::Foreground)
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-22.0, -74.0))
            .show(context, |ui| {
                Frame::NONE
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0_f32, tint(color, 90)))
                    .corner_radius(CornerRadius::same(12))
                    .shadow(theme::panel_shadow())
                    .inner_margin(Margin::symmetric(14, 11))
                    .show(ui, |ui| {
                        ui.set_max_width(360.0);
                        ui.horizontal_top(|ui| {
                            icon(ui, icon_kind, color, 15.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(text).font(theme::medium(12.0)).color(TEXT),
                                )
                                .wrap(),
                            );
                        });
                    });
            });
        context.request_repaint_after(Duration::from_millis(120));
    }
}

enum Action {
    Start(Quest),
    Claim(String),
    Stop,
}

impl Drop for App {
    fn drop(&mut self) {
        // Closing the window must not leave a quest running. The runner process
        // also exits on its own when this process goes away, because it holds
        // the other end of the runner's stdin pipe.
        self.worker.send(Command::Stop);
        std::thread::sleep(Duration::from_millis(150));
    }
}

impl eframe::App for App {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_events(context);
        self.animate_progress(context);

        // The CDP helper reports from its own thread; keep the spinner alive.
        if self.cdp_rx.is_some() {
            context.request_repaint_after(Duration::from_millis(120));
        }

        let focused = context.input(|input| input.focused);
        if focused
            && !self.was_focused
            && self.settings.refresh_on_focus
            && self.user.is_some()
            && self.running.is_none()
        {
            self.worker.send(Command::Refresh);
        }
        self.was_focused = focused;

        if self.user.is_none() {
            self.login_screen(context);
            self.toast_ui(context);
            return;
        }

        self.header(context);
        self.footer(context);
        self.main_content(context);
        self.toast_ui(context);
        self.help_window(context);
        self.logout_window(context);
    }
}

fn nav_tab(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let font = theme::medium(13.0);
    let color = if selected { TEXT } else { TEXT_SOFT };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, color);
    let width = galley.size().x + 26.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), tint(ACCENT, 32));
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), tint(TEXT, 5));
    }
    let text_color = if selected { ACCENT_LIGHT } else { color };
    let pos = Pos2::new(
        rect.center().x - galley.size().x / 2.0,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(pos, galley, text_color);
    response
}

fn icon(ui: &mut Ui, kind: Icon, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    theme::paint_icon(ui.painter(), rect, kind, color);
}

fn icon_button(ui: &mut Ui, kind: Icon, size: f32, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), tint(TEXT, 8));
    }
    theme::paint_icon(
        ui.painter(),
        rect.shrink(size * 0.28),
        kind,
        if response.hovered() { TEXT } else { TEXT_SOFT },
    );
    response.on_hover_text(tooltip)
}

fn pill_button(
    ui: &mut Ui,
    label: &str,
    icon_kind: Option<Icon>,
    fill: Color32,
    text_color: Color32,
    stroke: Stroke,
    min_width: f32,
) -> egui::Response {
    let font = theme::semibold(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, text_color);
    let icon_space = if icon_kind.is_some() { 21.0 } else { 0.0 };
    let width = (galley.size().x + icon_space + 28.0).max(min_width);
    let height = 32.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    let hovered = response.hovered();
    let fill = if hovered { lighten(fill, 12) } else { fill };
    ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
    if stroke.width > 0.0 {
        ui.painter()
            .rect_stroke(rect, CornerRadius::same(9), stroke, StrokeKind::Inside);
    }

    let content_width = galley.size().x + icon_space;
    let start = rect.center().x - content_width / 2.0;
    if let Some(icon_kind) = icon_kind {
        let icon_rect =
            Rect::from_center_size(Pos2::new(start + 7.5, rect.center().y), Vec2::splat(14.0));
        theme::paint_icon(ui.painter(), icon_rect, icon_kind, text_color);
    }
    ui.painter().galley(
        Pos2::new(start + icon_space, rect.center().y - galley.size().y / 2.0),
        galley,
        text_color,
    );

    response
}

fn filter_pill(ui: &mut Ui, label: &str, selected: bool, count: usize) -> egui::Response {
    let text = format!("{label}  {count}");
    let color = if selected { TEXT } else { TEXT_SOFT };
    let font = theme::medium(11.5);
    let galley = ui.painter().layout_no_wrap(text, font, color);
    let width = galley.size().x + 24.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), tint(ACCENT, 34));
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, tint(ACCENT, 95)),
            StrokeKind::Inside,
        );
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), tint(TEXT, 5));
    }
    let color = if selected { ACCENT_LIGHT } else { color };
    ui.painter().galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
    response
}

fn app_tile(ui: &mut Ui, quest: &Quest, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(11), tint(color, 28));
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(11),
        Stroke::new(1.0_f32, tint(color, 65)),
        StrokeKind::Inside,
    );
    let letter = quest
        .app_name
        .chars()
        .next()
        .or_else(|| quest.name.chars().next())
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    ui.painter().text(
        rect.center() + Vec2::new(0.0, 0.5),
        Align2::CENTER_CENTER,
        letter,
        theme::bold(size * 0.38),
        color,
    );
}

fn kind_chip(ui: &mut Ui, quest: &Quest) {
    let color = kind_color(quest.kind);
    let text = quest.kind.label();
    let font = theme::medium(9.8);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let width = galley.size().x + 14.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 18.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(6), tint(color, 26));
    ui.painter().galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
    response.on_hover_text(quest.kind.hint());
}

fn tag(ui: &mut Ui, label: &str, color: Color32, icon_kind: Option<Icon>) -> egui::Response {
    let font = theme::medium(11.0);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, color);
    let icon_space = if icon_kind.is_some() { 17.0 } else { 0.0 };
    let width = galley.size().x + icon_space + 20.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(8), tint(color, 14));
    let start = rect.center().x - (galley.size().x + icon_space) / 2.0;
    if let Some(icon_kind) = icon_kind {
        let icon_rect =
            Rect::from_center_size(Pos2::new(start + 5.5, rect.center().y), Vec2::splat(12.0));
        theme::paint_icon(ui.painter(), icon_rect, icon_kind, color);
    }
    ui.painter().galley(
        Pos2::new(start + icon_space, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    response
}

fn progress_bar(ui: &mut Ui, ratio: f32, color: Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 7.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(4), Color32::from_rgb(31, 35, 46));
    if ratio > 0.001 {
        let filled_width = (rect.width() * ratio.clamp(0.0, 1.0)).max(7.0);
        let filled = Rect::from_min_size(rect.min, Vec2::new(filled_width, rect.height()));
        ui.painter()
            .rect_filled(filled, CornerRadius::same(4), color);
    }
}

fn mini_progress(ui: &mut Ui, ratio: f32, color: Color32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 5.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(3), Color32::from_rgb(30, 34, 44));
    if ratio > 0.001 {
        let filled = Rect::from_min_size(
            rect.min,
            Vec2::new((width * ratio.clamp(0.0, 1.0)).max(5.0), rect.height()),
        );
        ui.painter()
            .rect_filled(filled, CornerRadius::same(3), color);
    }
}

fn fallback_avatar(ui: &mut Ui, user: &User, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), size / 2.0, color_from_str(&user.id));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        user.initial(),
        theme::semibold(size * 0.42),
        Color32::WHITE,
    );
}

fn color_from_str(value: &str) -> Color32 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    let palette = [ACCENT, VIDEO, ACTIVITY, SUCCESS, STREAM];
    palette[(hasher.finish() as usize) % palette.len()]
}

fn settings_card(ui: &mut Ui, title: &str, description: &str, contents: impl FnOnce(&mut Ui)) {
    Frame::NONE
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, BORDER_SOFT))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.label(RichText::new(title).font(theme::semibold(14.0)).color(TEXT));
            ui.add_space(1.0);
            ui.label(
                RichText::new(description)
                    .font(theme::regular(11.2))
                    .color(MUTED),
            );
            ui.add_space(14.0);
            contents(ui);
        });
}

fn toggle_row(ui: &mut Ui, title: &str, description: &str, value: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 56.0).max(140.0);
        ui.allocate_ui_with_layout(Vec2::new(width, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.label(RichText::new(title).font(theme::medium(12.5)).color(TEXT));
            ui.add_space(1.0);
            ui.add(
                egui::Label::new(
                    RichText::new(description)
                        .font(theme::regular(10.8))
                        .color(MUTED),
                )
                .wrap(),
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if switch(ui, value).changed() {
                changed = true;
            }
        });
    });
    changed
}

fn switch(ui: &mut Ui, value: &mut bool) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(40.0, 22.0), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let t = ui.ctx().animate_bool(response.id, *value);
    let background = lerp_color(Color32::from_rgb(40, 45, 58), ACCENT, t);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(11), background);
    let knob_x = rect.left() + 11.0 + t * (rect.width() - 22.0);
    ui.painter()
        .circle_filled(Pos2::new(knob_x, rect.center().y), 8.0, Color32::WHITE);
    response
}

fn mechanism_row(ui: &mut Ui, color: Color32, title: &str, description: &str) {
    ui.horizontal_top(|ui| {
        let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
        ui.painter().circle_filled(dot.center(), 4.0, color);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).font(theme::medium(12.2)).color(TEXT));
            ui.add(
                egui::Label::new(
                    RichText::new(description)
                        .font(theme::regular(10.8))
                        .color(MUTED),
                )
                .wrap(),
            );
        });
    });
}

fn reward_kind(reward: &str) -> RewardFilter {
    let lower = reward.to_ascii_lowercase();
    if lower.contains("nitro") {
        RewardFilter::Nitro
    } else if lower.contains("orb") {
        RewardFilter::Orbs
    } else if lower.contains("decor") {
        RewardFilter::Decoration
    } else {
        RewardFilter::Other
    }
}

fn kind_color(kind: crate::models::QuestKind) -> Color32 {
    use crate::models::QuestKind;
    match kind {
        QuestKind::Video => VIDEO,
        QuestKind::Game => GAME,
        QuestKind::Activity => ACTIVITY,
        QuestKind::Stream => STREAM,
        QuestKind::Achievement => WARNING,
        QuestKind::Unsupported => MUTED,
    }
}

fn lighten(color: Color32, amount: u8) -> Color32 {
    Color32::from_rgb(
        color.r().saturating_add(amount),
        color.g().saturating_add(amount),
        color.b().saturating_add(amount),
    )
}

fn lerp_color(from: Color32, to: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgb(
        (from.r() as f32 + (to.r() as f32 - from.r() as f32) * t).round() as u8,
        (from.g() as f32 + (to.g() as f32 - from.g() as f32) * t).round() as u8,
        (from.b() as f32 + (to.b() as f32 - from.b() as f32) * t).round() as u8,
    )
}
