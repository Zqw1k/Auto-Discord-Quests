use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct User {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub discriminator: String,
}

impl User {
    pub fn from_json(value: &Value) -> Self {
        Self {
            id: value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            username: value
                .get("username")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            global_name: value
                .get("global_name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_string),
            discriminator: value
                .get("discriminator")
                .and_then(Value::as_str)
                .unwrap_or("0")
                .to_string(),
        }
    }

    pub fn display_name(&self) -> &str {
        self.global_name.as_deref().unwrap_or(&self.username)
    }

    pub fn initial(&self) -> String {
        self.display_name()
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string()
    }

    pub fn handle(&self) -> String {
        if self.discriminator == "0" {
            format!("@{}", self.username)
        } else {
            format!("{}#{}", self.username, self.discriminator)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestKind {
    Video,
    Game,
    Activity,
    Stream,
    Achievement,
    Unsupported,
}

impl QuestKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Game => "Play game",
            Self::Activity => "Activity",
            Self::Stream => "Stream",
            Self::Achievement => "Achievement",
            Self::Unsupported => "Other",
        }
    }

    /// The task families this app can drive on its own.
    pub fn supported(self) -> bool {
        matches!(
            self,
            Self::Video | Self::Game | Self::Activity | Self::Stream
        )
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Video => "Auto-advance video progress",
            Self::Game => "Simulate game playtime",
            Self::Activity => "Cloud activity heartbeat",
            Self::Stream => "Report a desktop stream to Discord",
            Self::Achievement => "Unlock it inside the game/activity",
            Self::Unsupported => "Not supported yet",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Quest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub app_name: String,
    pub application_id: String,
    pub reward: String,
    pub kind: QuestKind,
    pub target: f64,
    pub progress: f64,
    pub enrolled: bool,
    pub completed: bool,
    pub claimed: bool,
    pub expires_at: Option<DateTime<Utc>>,
}

impl Quest {
    pub fn ratio(&self) -> f64 {
        if self.completed || self.claimed {
            return 1.0;
        }
        if self.target <= 0.0 {
            return 0.0;
        }
        (self.progress / self.target).clamp(0.0, 1.0)
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at
            .map(|expires| expires < Utc::now())
            .unwrap_or(false)
    }

    pub fn from_json(value: &Value) -> Option<Self> {
        let id = value.get("id")?.as_str()?.to_string();
        let config = value.get("config").unwrap_or(&Value::Null);
        let messages = config.get("messages").unwrap_or(&Value::Null);
        let application = config.get("application").unwrap_or(&Value::Null);
        let user_status = value.get("user_status").unwrap_or(&Value::Null);

        let name = messages
            .get("quest_name")
            .and_then(Value::as_str)
            .unwrap_or("Discord Quest")
            .to_string();

        let app_name = application
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| messages.get("game_title").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();

        let description = messages
            .get("task_description")
            .and_then(Value::as_str)
            .or_else(|| messages.get("task_title").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();

        let tasks = config
            .get("task_config_v2")
            .or_else(|| config.get("task_config"))
            .and_then(|config| config.get("tasks"))
            .and_then(Value::as_object);

        let mut parsed_tasks: Vec<TaskInfo> = tasks
            .map(|tasks| {
                tasks
                    .iter()
                    .map(|(key, task)| TaskInfo {
                        key: key.clone(),
                        task_type: task
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or(key)
                            .to_string(),
                        target: task
                            .get("target")
                            .and_then(Value::as_f64)
                            .filter(|target| *target > 0.0),
                    })
                    .collect()
            })
            .unwrap_or_default();

        if parsed_tasks.is_empty() {
            let legacy_target = config
                .get("stream_duration_requirement_minutes")
                .and_then(Value::as_f64)
                .map(|minutes| minutes * 60.0);
            parsed_tasks.push(TaskInfo {
                key: "UNKNOWN".into(),
                task_type: "UNKNOWN".into(),
                target: legacy_target,
            });
        }

        let chosen = choose_task(&parsed_tasks);

        let kind = kind_for(&chosen.task_type);
        let target = chosen
            .target
            .or_else(|| {
                config
                    .get("stream_duration_requirement_minutes")
                    .and_then(Value::as_f64)
                    .map(|minutes| minutes * 60.0)
            })
            .unwrap_or(0.0);

        let progress = user_status
            .get("progress")
            .and_then(Value::as_object)
            .and_then(|progress| {
                progress
                    .get(&chosen.key)
                    .and_then(|entry| entry.get("value"))
                    .and_then(Value::as_f64)
                    .or_else(|| {
                        progress
                            .values()
                            .find_map(|entry| entry.get("value").and_then(Value::as_f64))
                    })
            })
            .or_else(|| {
                user_status
                    .get("stream_progress_seconds")
                    .and_then(Value::as_f64)
            })
            .unwrap_or(0.0);

        let completed_at = user_status
            .get("completed_at")
            .or_else(|| {
                user_status
                    .get("progress")
                    .and_then(|progress| progress.get(&chosen.key))
                    .and_then(|entry| entry.get("completed_at"))
            })
            .filter(|value| !value.is_null());

        let reward = config
            .get("rewards_config")
            .and_then(|config| config.get("rewards"))
            .and_then(Value::as_array)
            .and_then(|rewards| rewards.first())
            .map(|reward| {
                reward
                    .get("messages")
                    .and_then(|messages| messages.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        reward.get("type").and_then(Value::as_u64).map(|kind| {
                            match kind {
                                3 => "Avatar Decoration",
                                4 => "Orbs",
                                5 => "Nitro",
                                _ => "Reward",
                            }
                            .to_string()
                        })
                    })
                    .unwrap_or_else(|| "Reward".to_string())
            })
            .unwrap_or_else(|| "Reward".to_string());

        let mut quest = Self {
            id,
            name,
            description,
            app_name,
            application_id: application
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            reward,
            kind,
            target,
            progress,
            enrolled: user_status
                .get("enrolled_at")
                .map(|value| !value.is_null())
                .unwrap_or(false),
            completed: completed_at.is_some(),
            claimed: user_status
                .get("claimed_at")
                .map(|value| !value.is_null())
                .unwrap_or(false),
            expires_at: config
                .get("expires_at")
                .and_then(Value::as_str)
                .and_then(parse_date),
        };

        if !quest.completed && quest.target > 0.0 && quest.progress >= quest.target {
            quest.completed = true;
        }

        Some(quest)
    }
}

#[derive(Clone, Debug)]
struct TaskInfo {
    key: String,
    task_type: String,
    target: Option<f64>,
}

fn choose_task(tasks: &[TaskInfo]) -> &TaskInfo {
    let priority = |task: &&TaskInfo| -> u8 {
        let kind = kind_for(&task.task_type);
        match kind {
            QuestKind::Video => 0,
            QuestKind::Game => 1,
            QuestKind::Activity => 2,
            QuestKind::Stream => 3,
            QuestKind::Achievement => 4,
            QuestKind::Unsupported => 5,
        }
    };
    tasks
        .iter()
        .filter(|task| task.target.is_some())
        .min_by_key(priority)
        .or_else(|| tasks.first())
        .expect("at least one task is always synthesised")
}

fn kind_for(task_type: &str) -> QuestKind {
    let value = task_type.to_ascii_uppercase();
    if value.contains("VIDEO") {
        QuestKind::Video
    } else if value == "PLAY_ON_DESKTOP" {
        QuestKind::Game
    } else if value == "PLAY_ACTIVITY" {
        QuestKind::Activity
    } else if value.contains("STREAM") {
        QuestKind::Stream
    } else if value.contains("ACHIEVEMENT") {
        QuestKind::Achievement
    } else {
        QuestKind::Unsupported
    }
}

fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

pub fn format_seconds(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let secs = total % 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m {secs:02}s")
    } else {
        format!("{secs}s")
    }
}

pub fn format_remaining(expires: Option<DateTime<Utc>>) -> String {
    let Some(expires) = expires else {
        return "No expiry".into();
    };
    let delta = expires - Utc::now();
    if delta.num_seconds() <= 0 {
        return "Expired".into();
    }
    let days = delta.num_days();
    if days >= 1 {
        format!("{days}d left")
    } else {
        let hours = delta.num_hours();
        if hours >= 1 {
            format!("{hours}h left")
        } else {
            format!("{}m left", delta.num_minutes().max(1))
        }
    }
}
