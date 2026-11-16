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
use std::sync::Arc;
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
    Start(Quest, Settings),
    StartQueue(Vec<Quest>, Settings),
    Stop,
    Claim(String),
    UpdateSettings(Settings),
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
}

/// A real, idle process kept alive while a play-time quest runs. Discord's
/// client only reports playtime for processes it sees, so the helper mode is
/// spawned from this same binary (`questpilot --game-runner <seconds>`).
struct GameRunner(Child);

impl GameRunner {
    fn spawn(seconds: f64) -> Option<Self> {
        let executable = std::env::current_exe().ok()?;
        let lifetime = (seconds.max(60.0) + 300.0).ceil() as u64;
        ProcessCommand::new(executable)
            .arg("--game-runner")
            .arg(lifetime.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
            .map(Self)
    }
}

impl Drop for GameRunner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Heartbeats are sent a little early or late so the cadence does not look
/// machine-generated.
fn heartbeat_wait() -> u64 {
    52 + rand::thread_rng().gen_range(0..17)
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
                    self.queue.clear();
                }
            }
            Command::Claim(quest_id) => self.claim(quest_id).await,
            Command::UpdateSettings(settings) => self.settings = settings,
        }
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
        self.active = Some(Active {
            cancel: cancel.clone(),
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
            let finish = run_quest(api, user.id, quest, settings, emitter, cancel).await;
            let _ = finish_tx.send(finish);
        });
    }

    fn cancel_active(&mut self) {
        if let Some(active) = &self.active {
            active.cancel.store(true, Ordering::SeqCst);
        }
    }

    async fn on_finish(&mut self, finish: Finish) {
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
) -> Finish {
    let quest_id = quest.id.clone();
    let result = match quest.kind {
        QuestKind::Video => run_video(&api, &quest, &settings, &emitter, &cancel).await,
        QuestKind::Game => run_game(&api, &user_id, &quest, &emitter, &cancel).await,
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

    emitter.send(Event::Progress {
        quest_id: quest.id.clone(),
        percent: (seconds / target * 100.0).clamp(0.0, 100.0),
        seconds,
        target,
    });

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

        emitter.send(Event::Progress {
            quest_id: quest.id.clone(),
            percent: (seconds / target * 100.0).clamp(0.0, 100.0),
            seconds,
            target,
        });

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

    verify_completion(api, quest, emitter, cancel, completed).await
}

async fn run_game(
    api: &DiscordApi,
    user_id: &str,
    quest: &Quest,
    emitter: &Emitter,
    cancel: &AtomicBool,
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

    let runner = GameRunner::spawn(quest.target);
    match &runner {
        Some(_) => emitter.log(
            LogLevel::Info,
            "Started the game runner process for this session.",
        ),
        None => emitter.log(
            LogLevel::Warn,
            "Could not start the game runner process; heartbeats will still be sent.",
        ),
    }

    let target = quest.target;
    let started = Instant::now();
    let mut progress = quest.progress;

    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("cancelled");
        }

        let outcome = api
            .send_game_heartbeat(
                &quest.id,
                quest.app_id(),
                false,
                Some(&executable_path),
                Some(&fingerprint),
            )
            .await?;

        let estimated = (quest.progress + started.elapsed().as_secs_f64()).min(target);
        progress = outcome
            .progress
            .unwrap_or(estimated)
            .max(progress)
            .min(target);

        emitter.send(Event::Progress {
            quest_id: quest.id.clone(),
            percent: (progress / target * 100.0).clamp(0.0, 99.0),
            seconds: progress,
            target,
        });

        if outcome.completed {
            return verify_completion(api, quest, emitter, cancel, true).await;
        }
        if progress >= target {
            break;
        }
        if !sleep_cancellable(cancel, heartbeat_wait()).await {
            bail!("cancelled");
        }
    }

    let terminal = api
        .send_game_heartbeat(
            &quest.id,
            quest.app_id(),
            true,
            Some(&executable_path),
            Some(&fingerprint),
        )
        .await?;

    verify_completion(api, quest, emitter, cancel, terminal.completed).await
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
    let mut progress = quest.progress;
    let mut completed = false;

    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("cancelled");
        }

        let outcome = api
            .send_activity_heartbeat(&quest.id, quest.app_id(), false)
            .await?;
        if let Some(value) = outcome.progress {
            progress = value.max(progress);
        }
        progress = progress.min(target);

        emitter.send(Event::Progress {
            quest_id: quest.id.clone(),
            percent: (progress / target * 100.0).clamp(0.0, 99.0),
            seconds: progress,
            target,
        });

        if outcome.completed {
            completed = true;
            break;
        }
        if progress >= target {
            break;
        }
        if !sleep_cancellable(cancel, heartbeat_wait()).await {
            bail!("cancelled");
        }
    }

    let terminal = api
        .send_activity_heartbeat(&quest.id, quest.app_id(), true)
        .await?;
    if terminal.completed {
        completed = true;
    }

    verify_completion(api, quest, emitter, cancel, completed).await
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
    let mut progress = quest.progress;
    let mut checked_credit = false;

    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("cancelled");
        }

        let outcome = api.send_stream_heartbeat(&quest.id, &key).await?;
        // Until Discord reports stream seconds we fall back to elapsed time,
        // mirroring the desktop client's own pacing.
        let estimated = (quest.progress + started.elapsed().as_secs_f64()).min(target);
        progress = outcome
            .progress
            .unwrap_or(estimated)
            .max(progress)
            .min(target);

        emitter.send(Event::Progress {
            quest_id: quest.id.clone(),
            percent: (progress / target * 100.0).clamp(0.0, 99.0),
            seconds: progress,
            target,
        });

        if outcome.completed {
            return verify_completion(api, quest, emitter, cancel, true).await;
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

        if progress >= target {
            break;
        }
        if !sleep_cancellable(cancel, stream_wait()).await {
            bail!("cancelled");
        }
    }

    verify_completion(api, quest, emitter, cancel, false).await
}

async fn verify_completion(
    api: &DiscordApi,
    quest: &Quest,
    emitter: &Emitter,
    cancel: &AtomicBool,
    already_completed: bool,
) -> Result<()> {
    if already_completed {
        emitter.send(Event::Progress {
            quest_id: quest.id.clone(),
            percent: 100.0,
            seconds: quest.target,
            target: quest.target,
        });
        return Ok(());
    }

    emitter.log(LogLevel::Info, "Waiting for Discord to confirm completion…");

    for attempt in 0..6 {
        if cancel.load(Ordering::SeqCst) {
            bail!("cancelled");
        }
        if attempt > 0 && !sleep_cancellable(cancel, 2).await {
            bail!("cancelled");
        }
        if let Ok(quests) = api.get_quests().await {
            if let Some(fresh) = quests.iter().find(|item| item.id == quest.id) {
                emitter.send(Event::Progress {
                    quest_id: quest.id.clone(),
                    percent: fresh.ratio() * 100.0,
                    seconds: fresh.progress,
                    target: fresh.target,
                });
                if fresh.completed {
                    return Ok(());
                }
            }
        }
    }

    bail!("Discord has not confirmed completion yet. Try again in a few minutes.")
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
}
