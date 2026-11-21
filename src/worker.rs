use crate::config::Settings;
use crate::discord::fingerprint::{format_executable_path, synthesize_fingerprint};
use crate::discord::DiscordApi;
use crate::models::{Quest, QuestKind, User};
use anyhow::{anyhow, bail, Result};
use eframe::egui;
use rand::Rng;
use std::collections::VecDeque;
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

#[derive(Clone, Copy, Debug)]
pub enum LogLevel {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug)]
pub enum Event {
    Log(LogLevel, String),
    AuthResult(std::result::Result<User, String>),
    Accounts(Vec<DetectedAccount>),
    Avatar {
        url: String,
        size: [usize; 2],
        pixels: Vec<u8>,
    },
    Quests {
        quests: Vec<Quest>,
        note: Option<String>,
    },
    QuestsFailed(String),
    Running(Option<String>),
    Progress {
        quest_id: String,
        percent: f64,
        seconds: f64,
        target: f64,
    },
    Done {
        quest_id: String,
        claimed: bool,
    },
    Failed(String),
    Stopped,
    Claimed,
    ClaimFailed(String),
}

pub enum Command {
    Login(String),
    Logout,
    Refresh,
    /// `(token, source label)` pairs found on disk; resolved to real users.
    ResolveAccounts(Vec<(String, String)>),
    FetchAvatar(String),
    Start(Quest, Settings),
    StartQueue(Vec<Quest>, Settings),
    Stop,
    Claim(String),
    UpdateSettings(Settings),
}

/// A detected token paired with the account it belongs to.
#[derive(Clone, Debug)]
pub struct DetectedAccount {
    pub token: String,
    pub user: User,
}

pub struct Handle {
    commands: UnboundedSender<Command>,
    pub events: Receiver<Event>,
}

impl Handle {
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

pub fn spawn(context: egui::Context, settings: Settings) -> Handle {
    let (command_tx, command_rx) = unbounded_channel();
    let (event_tx, event_rx) = channel();

    let thread_context = context.clone();
    std::thread::Builder::new()
        .name("questpilot-worker".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("create tokio runtime");
            runtime.block_on(async move {
                let mut worker = Worker::new(command_rx, event_tx, thread_context, settings);
                worker.run().await;
            });
        })
        .expect("spawn worker thread");

    Handle {
        commands: command_tx,
        events: event_rx,
    }
}

#[derive(Clone)]
struct Emitter {
    events: Sender<Event>,
    context: egui::Context,
}

impl Emitter {
    fn send(&self, event: Event) {
        let _ = self.events.send(event);
        self.context.request_repaint();
    }

    fn log(&self, level: LogLevel, message: impl Into<String>) {
        self.send(Event::Log(level, message.into()));
    }
}

struct Active {
    cancel: Arc<AtomicBool>,
    /// Shared with the running task so that Stop can kill the helper process
    /// even while the task is waiting on the network.
    runner: Arc<Mutex<Option<GameRunner>>>,
}

/// A real, idle process kept alive while a play-time quest runs. Discord's
/// client only reports playtime for processes it sees, so the helper mode is
/// spawned from this same binary (`questpilot --game-runner <seconds>`).
struct GameRunner {
    child: Child,
    /// Kept open for the lifetime of this handle. The helper exits as soon as
    /// it reads EOF, which also happens if the app is killed without cleanup.
    _pipe: Option<std::process::ChildStdin>,
}

impl GameRunner {
    fn spawn(seconds: f64) -> Option<Self> {
        let executable = std::env::current_exe().ok()?;
        // Outlive the quest by a margin: the heartbeat loop may keep the
        // session open a little longer while Discord credits the last beat.
        let lifetime = (seconds.max(60.0) + 900.0).ceil() as u64;
        let mut child = ProcessCommand::new(executable)
            .arg("--game-runner")
            .arg(lifetime.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let pipe = child.stdin.take();
        Some(Self { child, _pipe: pipe })
    }
}

impl Drop for GameRunner {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Discord credits playtime in heartbeat-sized windows: a beat more than 60
/// seconds apart loses the extra seconds, which is what leaves a quest one
/// percent short. Jitter stays inside that window.
fn heartbeat_wait() -> u64 {
    54 + rand::thread_rng().gen_range(0..6)
}

fn percent(progress: f64, target: f64) -> f64 {
    if target <= 0.0 {
        0.0
    } else {
        (progress / target * 100.0).clamp(0.0, 100.0)
    }
}

/// Sends the same numbers the window shows, so the bar always reflects what
/// Discord has credited instead of a local estimate.
fn emit_progress(emitter: &Emitter, quest_id: &str, seconds: f64, target: f64) {
    emitter.send(Event::Progress {
        quest_id: quest_id.to_string(),
        percent: percent(seconds, target),
        seconds,
        target,
    });
}

enum Finish {
    Done { quest_id: String, claimed: bool },
    Failed(String),
    Stopped,
}

struct Worker {
    commands: UnboundedReceiver<Command>,
    emitter: Emitter,
    finish_tx: UnboundedSender<Finish>,
    finish_rx: UnboundedReceiver<Finish>,
    api: Option<Arc<DiscordApi>>,
    user: Option<User>,
    settings: Settings,
    active: Option<Active>,
    queue: VecDeque<Quest>,
}

impl Worker {
    fn new(
        commands: UnboundedReceiver<Command>,
        events: Sender<Event>,
        context: egui::Context,
        settings: Settings,
    ) -> Self {
        let (finish_tx, finish_rx) = unbounded_channel();
        Self {
            commands,
            emitter: Emitter { events, context },
            finish_tx,
            finish_rx,
            api: None,
            user: None,
            settings,
            active: None,
            queue: VecDeque::new(),
        }
    }

    async fn run(&mut self) {
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                command = self.commands.recv() => {
                    match command {
                        Some(command) => self.handle(command).await,
                        None => break,
                    }
                }
                finish = self.finish_rx.recv() => {
                    if let Some(finish) = finish {
                        self.on_finish(finish).await;
                    }
                }
                _ = ticker.tick() => {}
            }
        }
    }

    async fn handle(&mut self, command: Command) {
        match command {
            Command::Login(token) => self.login(token).await,
            Command::Logout => {
                self.cancel_active();
                self.queue.clear();
                self.api = None;
                self.user = None;
                self.emitter.send(Event::Running(None));
                self.emitter.log(LogLevel::Info, "Signed out.");
            }
            Command::Refresh => self.refresh_quests().await,
            Command::ResolveAccounts(found) => self.resolve_accounts(found).await,
            Command::FetchAvatar(url) => {
                let emitter = self.emitter.clone();
                tokio::spawn(async move {
                    if let Some((size, pixels)) = download_avatar(&url).await {
                        emitter.send(Event::Avatar { url, size, pixels });
                    }
                });
            }
            Command::Start(quest, settings) => {
                self.settings = settings;
                self.queue.clear();
                self.start_quest(quest);
            }
            Command::StartQueue(mut quests, settings) => {
                self.settings = settings;
                self.queue.clear();
                self.queue.extend(quests.drain(..));
                let count = self.queue.len();
                self.emitter.log(
                    LogLevel::Info,
                    format!("Starting a queue of {count} quests."),
                );
                self.start_next_in_queue();
            }
            Command::Stop => {
                if self.active.is_some() {
                    self.cancel_active();
                    self.queue.clear();
                    self.emitter
                        .log(LogLevel::Warn, "Stopping the current quest…");
                } else {
                    // Nothing is running. Tell the window anyway so a stale
                    // "running" state can always be cleared with Stop.
                    self.queue.clear();
                    self.emitter.send(Event::Running(None));
                    self.emitter.send(Event::Stopped);
                }
            }
            Command::Claim(quest_id) => self.claim(quest_id).await,
            Command::UpdateSettings(settings) => self.settings = settings,
        }
    }

    /// Turns raw tokens found on disk into real accounts, so the window can
    /// show the right name and avatar instead of the Discord build they came
    /// from.
    async fn resolve_accounts(&mut self, found: Vec<(String, String)>) {
        let total = found.len();
        let mut resolved = Vec::new();
        for (token, source) in found {
            let api = match DiscordApi::new(&token) {
                Ok(api) => api,
                Err(error) => {
                    self.emitter.log(
                        LogLevel::Warn,
                        format!(
                            "Could not read the {source} token: {}",
                            friendly_error(&error)
                        ),
                    );
                    continue;
                }
            };
            match api.get_me().await {
                Ok(user) => {
                    self.emitter.log(
                        LogLevel::Info,
                        format!("Found {} ({source}).", user.handle()),
                    );
                    resolved.push(DetectedAccount { token, user });
                }
                Err(error) => {
                    self.emitter.log(
                        LogLevel::Warn,
                        format!(
                            "The {source} token is no longer valid: {}",
                            friendly_error(&error)
                        ),
                    );
                }
            }
        }
        if resolved.is_empty() {
            self.emitter.log(
                LogLevel::Warn,
                format!("None of the {total} detected token(s) could be used."),
            );
        }
        self.emitter.send(Event::Accounts(resolved));
    }

    async fn login(&mut self, token: String) {
        let token = token.trim();
        let token = token
            .trim_matches(|c: char| matches!(c, '"' | '\'' | '`'))
            .trim();
        let token = token
            .strip_prefix("Bearer ")
            .map(str::trim)
            .unwrap_or(token)
            .to_string();
        if token.is_empty() {
            self.emitter.send(Event::AuthResult(Err(
                "Enter your Discord token first.".into()
            )));
            return;
        }

        self.emitter.log(LogLevel::Info, "Validating token…");
        let api = match DiscordApi::new(&token) {
            Ok(api) => Arc::new(api),
            Err(error) => {
                self.emitter
                    .send(Event::AuthResult(Err(friendly_error(&error))));
                return;
            }
        };

        match api.get_me().await {
            Ok(user) => {
                self.emitter
                    .log(LogLevel::Success, format!("Connected as {}", user.handle()));
                self.user = Some(user.clone());
                self.api = Some(api);
                self.emitter.send(Event::AuthResult(Ok(user)));
                self.refresh_quests().await;
            }
            Err(error) => {
                let message = friendly_error(&error);
                self.emitter.log(LogLevel::Error, message.clone());
                self.emitter.send(Event::AuthResult(Err(message)));
            }
        }
    }

    async fn refresh_quests(&mut self) {
        let Some(api) = self.api.clone() else {
            return;
        };
        match api.get_quest_feed().await {
            Ok(feed) => {
                let raw = feed.raw;
                let dropped = feed.dropped;
                let excluded = feed.excluded;
                let blocked_until = feed.blocked_until;
                let mut quests = feed.quests;
                let before = quests.len();
                quests.retain(|quest| !quest.is_expired());
                let expired = before - quests.len();
                sort_quests(&mut quests);
                let open = quests
                    .iter()
                    .filter(|quest| !quest.completed && !quest.claimed && !quest.is_expired())
                    .count();

                let mut details = Vec::new();
                if excluded > 0 {
                    details.push(format!("{excluded} excluded quests"));
                }
                if dropped > 0 {
                    details.push(format!("{dropped} unreadable entries"));
                }
                if expired > 0 {
                    details.push(format!("{expired} expired quests hidden"));
                }
                if let Some(until) = &blocked_until {
                    details.push(format!("enrollment blocked until {until}"));
                }

                let note = if quests.is_empty() {
                    if details.is_empty() {
                        Some(
                            "Discord returned an empty list. This account may not be eligible for quests in this region."
                                .to_string(),
                        )
                    } else {
                        Some(format!(
                            "Discord returned no available quests ({}).",
                            details.join(" · ")
                        ))
                    }
                } else {
                    None
                };

                let suffix = if details.is_empty() {
                    String::new()
                } else {
                    format!(" {}", details.join(" · "))
                };
                self.emitter.log(
                    LogLevel::Info,
                    format!(
                        "Loaded {} quests ({} available, {} raw entries).{suffix}",
                        quests.len(),
                        open,
                        raw
                    ),
                );
                self.emitter.send(Event::Quests { quests, note });
            }
            Err(error) => {
                let message = friendly_error(&error);
                self.emitter.log(LogLevel::Error, message.clone());
                self.emitter.send(Event::QuestsFailed(message));
            }
        }
    }

    fn start_next_in_queue(&mut self) {
        if self.active.is_some() {
            return;
        }
        while let Some(quest) = self.queue.pop_front() {
            if quest.completed || quest.claimed || quest.is_expired() || !quest.kind.supported() {
                continue;
            }
            self.start_quest(quest);
            return;
        }
        if self.queue.is_empty() {
            self.emitter.log(LogLevel::Success, "Queue finished.");
        }
    }

    fn start_quest(&mut self, quest: Quest) {
        let Some(api) = self.api.clone() else {
            self.emitter
                .log(LogLevel::Error, "Sign in before starting.");
            return;
        };
        let Some(user) = self.user.clone() else {
            self.emitter
                .log(LogLevel::Error, "Account details not found.");
            return;
        };
        if self.active.is_some() {
            self.emitter.log(
                LogLevel::Warn,
                "Another quest is already running. Run one quest at a time.",
            );
            return;
        }
        if !quest.kind.supported() {
            self.emitter.log(
                LogLevel::Warn,
                format!(
                    "Quest \"{}\" uses unsupported type {}.",
                    quest.name,
                    quest.kind.label()
                ),
            );
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let runner = Arc::new(Mutex::new(None));
        self.active = Some(Active {
            cancel: cancel.clone(),
            runner: runner.clone(),
        });
        self.emitter.send(Event::Running(Some(quest.id.clone())));
        self.emitter.log(
            LogLevel::Info,
            format!("Starting \"{}\" ({}).", quest.name, quest.kind.label()),
        );

        let settings = self.settings.clone();
        let emitter = self.emitter.clone();
        let finish_tx = self.finish_tx.clone();
        tokio::spawn(async move {
            let finish = run_quest(api, user.id, quest, settings, emitter, cancel, runner).await;
            let _ = finish_tx.send(finish);
        });
    }

    fn cancel_active(&mut self) {
        if let Some(active) = &self.active {
            active.cancel.store(true, Ordering::SeqCst);
            // Kill the helper process right now instead of waiting for the
            // async task to notice the cancellation flag.
            if let Ok(mut slot) = active.runner.lock() {
                slot.take();
            }
        }
    }

    async fn on_finish(&mut self, finish: Finish) {
        if let Some(active) = &self.active {
            if let Ok(mut slot) = active.runner.lock() {
                slot.take();
            }
        }
        self.active = None;
        self.emitter.send(Event::Running(None));

        match finish {
            Finish::Done { quest_id, claimed } => {
                self.emitter.send(Event::Done { quest_id, claimed });
                if claimed {
                    self.emitter.log(LogLevel::Success, "Reward claimed.");
                }
            }
            Finish::Failed(error) => {
                self.emitter.log(LogLevel::Error, error.clone());
                self.emitter.send(Event::Failed(error));
            }
            Finish::Stopped => {
                self.emitter.log(LogLevel::Warn, "Quest stopped.");
                self.emitter.send(Event::Stopped);
            }
        }

        self.refresh_quests().await;

        if !self.queue.is_empty() {
            tokio::time::sleep(Duration::from_millis(800)).await;
            self.start_next_in_queue();
        }
    }

    async fn claim(&mut self, quest_id: String) {
        let Some(api) = self.api.clone() else {
            return;
        };
        match api.claim_reward(&quest_id).await {
            Ok(_) => {
                self.emitter.log(LogLevel::Success, "Reward claimed.");
                self.emitter.send(Event::Claimed);
            }
            Err(error) => {
                let message = friendly_error(&error);
                self.emitter.log(LogLevel::Error, message.clone());
                self.emitter.send(Event::ClaimFailed(message));
            }
        }
        self.refresh_quests().await;
    }
}

async fn run_quest(
    api: Arc<DiscordApi>,
    user_id: String,
    quest: Quest,
    settings: Settings,
    emitter: Emitter,
    cancel: Arc<AtomicBool>,
    runner: Arc<Mutex<Option<GameRunner>>>,
) -> Finish {
    let quest_id = quest.id.clone();
    let result = match quest.kind {
        QuestKind::Video => run_video(&api, &quest, &settings, &emitter, &cancel).await,
        QuestKind::Game => run_game(&api, &user_id, &quest, &emitter, &cancel, &runner).await,
        QuestKind::Activity => run_activity(&api, &quest, &emitter, &cancel).await,
        QuestKind::Stream => run_stream(&api, &quest, &emitter, &cancel).await,
        _ => Err(anyhow!(
            "Quest type {} is not supported.",
            quest.kind.label()
        )),
    };

    if let Err(error) = result {
        if cancel.load(Ordering::SeqCst) {
            return Finish::Stopped;
        }
        return Finish::Failed(friendly_error(&error));
    }

    if cancel.load(Ordering::SeqCst) {
        return Finish::Stopped;
    }

    let mut claimed = false;
    if settings.auto_claim {
        match api.claim_reward(&quest.id).await {
            Ok(_) => {
                claimed = true;
                emitter.log(LogLevel::Success, "Reward claimed automatically.");
            }
            Err(error) => {
                emitter.log(
                    LogLevel::Warn,
                    format!(
                        "Quest finished, but the reward could not be claimed: {}",
                        friendly_error(&error)
                    ),
                );
            }
        }
    }

    Finish::Done { quest_id, claimed }
}

async fn run_video(
    api: &DiscordApi,
    quest: &Quest,
    settings: &Settings,
    emitter: &Emitter,
    cancel: &AtomicBool,
) -> Result<()> {
    let target = quest.target;
    if target <= 0.0 {
        bail!("Video quest has no target duration.");
    }

    if !quest.enrolled {
        emitter.log(LogLevel::Info, "Accepting quest…");
        api.enroll(&quest.id).await?;
    }

    let speed = settings.video_speed.max(1.0);
    let interval = settings.video_interval.max(1);
    let mut seconds = quest.progress.clamp(0.0, target);
    let loop_started = Instant::now();

    emit_progress(emitter, &quest.id, seconds, target);

    let mut completed = false;
    while seconds < target {
        if cancel.load(Ordering::SeqCst) {
            bail!("cancelled");
        }

        // In safe mode progress can only advance as fast as real playback,
        // plus a small head start. Otherwise we use the configured multiplier.
        let allowed = if settings.safe_pacing {
            loop_started.elapsed().as_secs_f64() + 15.0
        } else {
            f64::INFINITY
        };
        let desired = ((seconds + speed * interval as f64).min(target)).min(allowed);

        if desired <= seconds + 0.05 {
            if !sleep_cancellable(cancel, 1).await {
                bail!("cancelled");
            }
            continue;
        }

        let jitter = rand::thread_rng().gen_range(0.0..0.4);
        let timestamp = (desired + jitter).min(target);
        completed = api.update_video_progress(&quest.id, timestamp).await?;
        seconds = desired;

        emit_progress(emitter, &quest.id, seconds, target);

        if completed || seconds >= target {
            break;
        }
        let wait = interval + rand::thread_rng().gen_range(0..2);
        if !sleep_cancellable(cancel, wait).await {
            bail!("cancelled");
        }
    }

    if !completed {
        completed = api.update_video_progress(&quest.id, target).await?;
    }

    if completed {
        emit_progress(emitter, &quest.id, target, target);
        return Ok(());
    }
    if wait_for_completion(api, quest, emitter, cancel).await? {
        return Ok(());
    }
    if cancel.load(Ordering::SeqCst) {
        bail!("cancelled");
    }
    bail!("Discord has not confirmed this video quest yet. Try again in a few minutes.")
}

async fn run_game(
    api: &DiscordApi,
    user_id: &str,
    quest: &Quest,
    emitter: &Emitter,
    cancel: &AtomicBool,
    runner: &Mutex<Option<GameRunner>>,
) -> Result<()> {
    if quest.target <= 0.0 {
        bail!("Game quest has no target duration.");
    }
    if quest.app_id().is_empty() {
        bail!("Quest has no application id.");
    }

    if !quest.enrolled {
        emitter.log(LogLevel::Info, "Accepting quest…");
        api.enroll(&quest.id).await?;
    }

    let metadata = api.app_metadata(quest.app_id()).await?;
    let executable_path = format_executable_path(&metadata.exe_path);
    let fingerprint = synthesize_fingerprint(
        user_id,
        quest.app_id(),
        &metadata.exe_path,
        metadata.is_pe64,
    );
    emitter.log(
        LogLevel::Info,
        format!(
            "Simulating playtime for \"{}\" ({}).",
            metadata.name, executable_path
        ),
    );

    match GameRunner::spawn(quest.target) {
        Some(runner_process) => {
            emitter.log(
                LogLevel::Info,
                "Started the game runner process for this session.",
            );
            if let Ok(mut slot) = runner.lock() {
                *slot = Some(runner_process);
            }
        }
        None => emitter.log(
            LogLevel::Warn,
            "Could not start the game runner process; heartbeats will still be sent.",
        ),
    }

    let result = game_session(api, quest, emitter, cancel, &executable_path, &fingerprint).await;
    // Close the Discord session even when the user pressed Stop or the run
    // failed; otherwise the server keeps counting playtime afterwards.
    let _ = tokio::time::timeout(
        Duration::from_secs(8),
        api.send_game_heartbeat(
            &quest.id,
            quest.app_id(),
            true,
            Some(&executable_path),
            Some(&fingerprint),
        ),
    )
    .await;
    result
}

async fn game_session(
    api: &DiscordApi,
    quest: &Quest,
    emitter: &Emitter,
    cancel: &AtomicBool,
    executable_path: &str,
    fingerprint: &str,
) -> Result<()> {
    let target = quest.target;
    let started = Instant::now();
    // Discord can sit a beat behind the clock, so the loop keeps the session
    // alive until the server itself reports the target. The deadline only
    // guards against a server that stops crediting altogether.
    let deadline = started + Duration::from_secs((target - quest.progress).max(0.0) as u64 + 300);
    let mut progress = quest.progress;
    let mut credited: Option<f64> = None;

    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("cancelled");
        }

        let outcome = api
            .send_game_heartbeat(
                &quest.id,
                quest.app_id(),
                false,
                Some(executable_path),
                Some(fingerprint),
            )
            .await?;

        if let Some(value) = outcome.progress {
            credited = Some(value.max(credited.unwrap_or(0.0)));
        }
        // Discord's own number wins; the clock is only a fallback for
        // responses that carry no progress at all.
        let estimated = (quest.progress + started.elapsed().as_secs_f64()).min(target);
        progress = credited.unwrap_or(estimated).clamp(progress, target);
        emit_progress(emitter, &quest.id, progress, target);

        if outcome.completed {
            return Ok(());
        }

        let reached = match credited {
            Some(value) => value >= target - 0.5,
            None => estimated >= target,
        };
        if reached {
            // Ask Discord to finalise, then read the answer back. If it has not
            // credited the newest heartbeat yet, keep playing one more beat
            // instead of ending 1% short.
            let terminal = api
                .send_game_heartbeat(
                    &quest.id,
                    quest.app_id(),
                    true,
                    Some(executable_path),
                    Some(fingerprint),
                )
                .await;
            if terminal.map(|outcome| outcome.completed).unwrap_or(false) {
                return Ok(());
            }
            if wait_for_completion(api, quest, emitter, cancel).await? {
                return Ok(());
            }
            if cancel.load(Ordering::SeqCst) {
                bail!("cancelled");
            }
            emitter.log(
                LogLevel::Warn,
                "Discord has not credited the newest heartbeat yet; keeping the game session open.",
            );
        }

        if Instant::now() >= deadline {
            bail!(
                "Discord credited {progress:.0} of {target:.0} seconds but never marked the quest complete. Try again later."
            );
        }
        if !sleep_cancellable(cancel, heartbeat_wait()).await {
            bail!("cancelled");
        }
    }
}

async fn run_activity(
    api: &DiscordApi,
    quest: &Quest,
    emitter: &Emitter,
    cancel: &AtomicBool,
) -> Result<()> {
    if quest.target <= 0.0 {
        bail!("Activity quest has no target duration.");
    }
    if quest.app_id().is_empty() {
        bail!("Quest has no application id.");
    }

    if !quest.enrolled {
        emitter.log(LogLevel::Info, "Accepting quest…");
        api.enroll(&quest.id).await?;
    }

    let target = quest.target;
    let started = Instant::now();
    let deadline = started + Duration::from_secs((target - quest.progress).max(0.0) as u64 + 300);
    let mut progress = quest.progress;
    let mut credited: Option<f64> = None;

    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("cancelled");
        }

        let outcome = api
            .send_activity_heartbeat(&quest.id, quest.app_id(), false)
            .await?;
        if let Some(value) = outcome.progress {
            credited = Some(value.max(credited.unwrap_or(0.0)));
        }
        let estimated = (quest.progress + started.elapsed().as_secs_f64()).min(target);
        progress = credited.unwrap_or(estimated).clamp(progress, target);
        emit_progress(emitter, &quest.id, progress, target);

        if outcome.completed {
            break;
        }
        let reached = match credited {
            Some(value) => value >= target - 0.5,
            None => estimated >= target,
        };
        if reached {
            let terminal = api
                .send_activity_heartbeat(&quest.id, quest.app_id(), true)
                .await;
            if terminal.map(|outcome| outcome.completed).unwrap_or(false) {
                break;
            }
            if wait_for_completion(api, quest, emitter, cancel).await? {
                break;
            }
            if cancel.load(Ordering::SeqCst) {
                bail!("cancelled");
            }
            emitter.log(
                LogLevel::Warn,
                "Discord has not credited the newest heartbeat yet; keeping the activity session open.",
            );
        }

        if Instant::now() >= deadline {
            bail!(
                "Discord credited {progress:.0} of {target:.0} seconds but never marked the quest complete. Try again later."
            );
        }
        if !sleep_cancellable(cancel, heartbeat_wait()).await {
            bail!("cancelled");
        }
    }

    let _ = tokio::time::timeout(
        Duration::from_secs(8),
        api.send_activity_heartbeat(&quest.id, quest.app_id(), true),
    )
    .await;
    Ok(())
}

/// Discord's desktop client prefixes every stream key with `stream_` and sends
/// it with each heartbeat. The server treats the rest as opaque, so a fresh
/// random key per session is enough.
fn stream_key() -> String {
    use rand::distributions::Alphanumeric;
    let body: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    format!("stream_{body}")
}

/// The client sends stream heartbeats every 30 seconds.
fn stream_wait() -> u64 {
    30 + rand::thread_rng().gen_range(0..4)
}

async fn run_stream(
    api: &DiscordApi,
    quest: &Quest,
    emitter: &Emitter,
    cancel: &AtomicBool,
) -> Result<()> {
    if quest.target <= 0.0 {
        bail!("Stream quest has no target duration.");
    }

    if !quest.enrolled {
        emitter.log(LogLevel::Info, "Accepting quest…");
        api.enroll(&quest.id).await?;
    }

    let key = stream_key();
    emitter.log(
        LogLevel::Info,
        "Reporting an active desktop stream to Discord.",
    );

    let target = quest.target;
    let started = Instant::now();
    let deadline = started + Duration::from_secs((target - quest.progress).max(0.0) as u64 + 300);
    let mut progress = quest.progress;
    let mut credited: Option<f64> = None;
    let mut checked_credit = false;

    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("cancelled");
        }

        let outcome = api.send_stream_heartbeat(&quest.id, &key).await?;
        // Until Discord reports stream seconds we fall back to elapsed time,
        // mirroring the desktop client's own pacing.
        if let Some(value) = outcome.progress {
            credited = Some(value.max(credited.unwrap_or(0.0)));
        }
        let estimated = (quest.progress + started.elapsed().as_secs_f64()).min(target);
        progress = credited.unwrap_or(estimated).clamp(progress, target);
        emit_progress(emitter, &quest.id, progress, target);

        if outcome.completed {
            return Ok(());
        }

        // After a few heartbeats, confirm the server is actually crediting
        // them instead of running for the full duration for nothing.
        if !checked_credit && started.elapsed() >= Duration::from_secs(95) {
            checked_credit = true;
            if let Ok(quests) = api.get_quests().await {
                if let Some(fresh) = quests.iter().find(|item| item.id == quest.id) {
                    if fresh.progress <= quest.progress + 1.0 && !fresh.completed && !fresh.claimed
                    {
                        bail!(
                            "Discord is not crediting these stream heartbeats. Stream quests may need a real stream from the desktop client."
                        );
                    }
                }
            }
        }

        let reached = match credited {
            Some(value) => value >= target - 0.5,
            None => estimated >= target,
        };
        if reached {
            break;
        }
        if Instant::now() >= deadline {
            bail!(
                "Discord credited {progress:.0} of {target:.0} seconds but never marked the quest complete. Try again later."
            );
        }
        if !sleep_cancellable(cancel, stream_wait()).await {
            bail!("cancelled");
        }
    }

    if wait_for_completion(api, quest, emitter, cancel).await? {
        return Ok(());
    }
    if cancel.load(Ordering::SeqCst) {
        bail!("cancelled");
    }
    bail!("Discord has not confirmed this stream quest yet. Try again in a few minutes.")
}

/// Asks Discord for the quest state until it reports the quest as completed,
/// forwarding the server's own numbers so the window matches Discord exactly.
async fn wait_for_completion(
    api: &DiscordApi,
    quest: &Quest,
    emitter: &Emitter,
    cancel: &AtomicBool,
) -> Result<bool> {
    for attempt in 0..8 {
        if cancel.load(Ordering::SeqCst) {
            return Ok(false);
        }
        if attempt > 0 && !sleep_cancellable(cancel, 2).await {
            return Ok(false);
        }
        if let Ok(quests) = api.get_quests().await {
            if let Some(fresh) = quests.iter().find(|item| item.id == quest.id) {
                emit_progress(emitter, &quest.id, fresh.progress, fresh.target.max(1.0));
                if fresh.completed {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

async fn sleep_cancellable(cancel: &AtomicBool, seconds: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    !cancel.load(Ordering::SeqCst)
}

impl Quest {
    pub fn app_id(&self) -> &str {
        &self.application_id
    }
}

fn sort_quests(quests: &mut [Quest]) {
    quests.sort_by_key(|quest| {
        let state = if quest.claimed {
            3
        } else if quest.completed {
            2
        } else if quest.is_expired() {
            1
        } else {
            0
        };
        (state, quest.expires_at)
    });
}

fn friendly_error(error: &anyhow::Error) -> String {
    let text = format!("{error:#}");
    if text.contains("HTTP 401") {
        "Invalid or expired token.".into()
    } else if text.contains("HTTP 403") {
        format!("Discord rejected the request (403). The token may be restricted. {text}")
    } else if text.contains("HTTP 429") {
        "Discord is rate limiting. Try again in a few minutes.".into()
    } else if text.contains("error sending request") || text.contains("dns error") {
        "Could not connect to Discord. Check your network or VPN.".into()
    } else {
        text
    }
}

/// Small shared client for the avatar CDN. Tokens are not needed there.
fn avatar_client() -> Option<&'static reqwest::Client> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(12))
                .build()
                .ok()
        })
        .as_ref()
}

async fn download_avatar(url: &str) -> Option<([usize; 2], Vec<u8>)> {
    let client = avatar_client()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    if response
        .content_length()
        .map(|length| length > 2 * 1024 * 1024)
        .unwrap_or(false)
    {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    decode_png(&bytes)
}

/// Discord serves avatar PNGs; decode them into the RGBA buffer egui wants.
fn decode_png(bytes: &[u8]) -> Option<([usize; 2], Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // Paletted and low-bit-depth avatars are expanded to 8-bit RGB(A).
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    buffer.truncate(info.buffer_size());

    let pixels = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity(buffer.len() / 3 * 4);
            for chunk in buffer.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            rgba
        }
        png::ColorType::Grayscale => buffer
            .iter()
            .flat_map(|value| [*value, *value, *value, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|chunk| [chunk[0], chunk[0], chunk[0], chunk[1]])
            .collect(),
        png::ColorType::Indexed => return None,
    };

    Some(([info.width as usize, info.height as usize], pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_keys_look_like_the_clients() {
        let key = stream_key();
        assert_eq!(key.len(), 39);
        assert!(key.starts_with("stream_"));
        assert!(key[7..].chars().all(|ch| ch.is_ascii_alphanumeric()));
        assert_ne!(key, stream_key());
    }

    #[test]
    fn stop_cancels_the_active_task_and_drops_the_runner() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let (_command_tx, command_rx) = unbounded_channel();
        let (event_tx, _event_rx) = channel();
        let mut worker = Worker::new(
            command_rx,
            event_tx,
            egui::Context::default(),
            Settings::default(),
        );

        let cancel = Arc::new(AtomicBool::new(false));
        worker.active = Some(Active {
            cancel: cancel.clone(),
            runner: Arc::new(Mutex::new(None)),
        });

        runtime.block_on(worker.handle(Command::Stop));

        assert!(cancel.load(Ordering::SeqCst));
        assert!(worker.active.is_some(), "the task reports back later");
    }

    #[test]
    fn stop_always_clears_a_stale_running_state() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let (_command_tx, command_rx) = unbounded_channel();
        let (event_tx, event_rx) = channel();
        let mut worker = Worker::new(
            command_rx,
            event_tx,
            egui::Context::default(),
            Settings::default(),
        );

        runtime.block_on(worker.handle(Command::Stop));

        let events: Vec<Event> = event_rx.try_iter().collect();
        assert!(matches!(events.first(), Some(Event::Running(None))));
        assert!(matches!(events.get(1), Some(Event::Stopped)));
    }

    #[test]
    fn heartbeats_stay_inside_discords_credit_window() {
        for _ in 0..200 {
            let wait = heartbeat_wait();
            assert!((54..=60).contains(&wait), "interval {wait}s loses credit");
        }
    }

    #[test]
    fn progress_percentage_is_clamped() {
        assert_eq!(percent(450.0, 900.0), 50.0);
        assert_eq!(percent(1200.0, 900.0), 100.0);
        assert_eq!(percent(-5.0, 900.0), 0.0);
        assert_eq!(percent(10.0, 0.0), 0.0);
    }

    /// A 2x2 palette PNG with a `tRNS` chunk. Discord still serves these for
    /// some legacy avatars, so they must decode through the normalization path.
    const PALETTE_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAMAAABFaP0WAAAADFBMVEX/AAAA/wAAAP/////7AGD2AAAABHRSTlP/gAD/oaGUZgAAAA5JREFUeJxjYGBkYGIGAAARAAeeoioSAAAAAElFTkSuQmCC";

    #[test]
    fn palette_avatars_decode_to_rgba() {
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, PALETTE_PNG)
            .expect("valid base64");
        let (size, pixels) = decode_png(&bytes).expect("decode palette png");
        assert_eq!(size, [2, 2]);
        assert_eq!(pixels.len(), 16);
        assert_eq!(
            pixels.as_slice(),
            [
                255, 0, 0, 255, // red, opaque
                0, 255, 0, 128, // green, half transparent
                0, 0, 255, 0, // blue, fully transparent
                255, 255, 255, 255, // white, opaque
            ]
        );
    }
}
