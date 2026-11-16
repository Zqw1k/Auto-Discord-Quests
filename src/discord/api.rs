use crate::discord::fingerprint;
use crate::models::{Quest, User};
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::{Client, Method};
use serde_json::{json, Value};
use std::fmt;
use std::time::Duration;

const API_BASE: &str = "https://discord.com/api/v9";
const QUEST_REFERER: &str = "https://discord.com/quest-home";

// Discord gates several endpoints on a reasonably fresh desktop client build.
// Keep these in sync with the current stable Discord release.
const CLIENT_VERSION: &str = "1.0.9266";
const CHROME_VERSION: &str = "152.0.7977.167";
const ELECTRON_VERSION: &str = "44.9.3";
const CLIENT_BUILD_NUMBER: u64 = 657850;
const NATIVE_BUILD_NUMBER: u64 = 97026;

fn user_agent() -> String {
    format!(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
         discord/{CLIENT_VERSION} Chrome/{CHROME_VERSION} Electron/{ELECTRON_VERSION} \
         Safari/537.36"
    )
}

fn super_properties() -> String {
    json!({
        "os": "Windows",
        "browser": "Discord Client",
        "device": "",
        "system_locale": "en-US",
        "client_version": CLIENT_VERSION,
        "release_channel": "stable",
        "os_version": "10.0.19045",
        "os_arch": "x64",
        "app_arch": "x64",
        "has_client_mods": false,
        "browser_user_agent": user_agent(),
        "browser_version": ELECTRON_VERSION,
        "client_build_number": CLIENT_BUILD_NUMBER,
        "native_build_number": NATIVE_BUILD_NUMBER,
        "client_event_source": Value::Null,
    })
    .to_string()
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub body: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.status == 0 {
            write!(formatter, "{}", self.body)
        } else {
            write!(formatter, "HTTP {}: {}", self.status, self.body)
        }
    }
}

impl std::error::Error for ApiError {}

#[derive(Clone, Debug, Default)]
pub struct HeartbeatOutcome {
    pub completed: bool,
    pub progress: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct AppMetadata {
    pub name: String,
    pub exe_path: String,
    pub is_pe64: bool,
}

/// Everything the quest endpoint tells us, including the diagnostic counts the
/// UI needs to explain an empty list.
#[derive(Debug, Default)]
pub struct QuestFeed {
    pub quests: Vec<Quest>,
    /// Entries in the raw `quests` array.
    pub raw: usize,
    /// Entries that could not be parsed (normally 0).
    pub dropped: usize,
    /// Quests Discord says this account cannot participate in.
    pub excluded: usize,
    /// Set when the account is temporarily blocked from enrolling.
    pub blocked_until: Option<String>,
}

pub struct DiscordApi {
    client: Client,
}

impl DiscordApi {
    pub fn new(token: &str) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(token).context("Token contains invalid characters")?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&user_agent()).context("Could not build User-Agent")?,
        );
        headers.insert(
            "x-super-properties",
            HeaderValue::from_str(&BASE64.encode(super_properties()))
                .context("Could not build x-super-properties")?,
        );
        headers.insert("x-discord-locale", HeaderValue::from_static("en-US"));
        headers.insert(
            "accept-language",
            HeaderValue::from_static("en-US,en;q=0.9"),
        );
        headers.insert(
            "x-debug-options",
            HeaderValue::from_static("bugReporterEnabled"),
        );

        let client = Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .context("Could not build HTTP client")?;

        Ok(Self { client })
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> std::result::Result<Value, ApiError> {
        let url = format!("{API_BASE}{path}");

        for attempt in 0..2 {
            let mut request = self.client.request(method.clone(), &url);
            if path.starts_with("/quests") {
                request = request.header("referer", QUEST_REFERER);
            }
            if let Some(body) = body.clone() {
                request = request.json(&body);
            }

            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    return Err(ApiError {
                        status: 0,
                        body: format!("Could not connect to Discord: {error}"),
                    })
                }
            };

            let status = response.status();
            let bytes = match response.bytes().await {
                Ok(bytes) => bytes,
                Err(error) => {
                    return Err(ApiError {
                        status: status.as_u16(),
                        body: format!("Could not read the response: {error}"),
                    })
                }
            };

            if status.as_u16() == 429 && attempt == 0 {
                let retry_after = serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .and_then(|value| value.get("retry_after").and_then(Value::as_f64))
                    .unwrap_or(2.0)
                    .clamp(1.0, 10.0);
                tokio::time::sleep(Duration::from_secs_f64(retry_after)).await;
                continue;
            }

            if !status.is_success() {
                return Err(ApiError {
                    status: status.as_u16(),
                    body: snippet(&bytes),
                });
            }

            if bytes.is_empty() {
                return Ok(Value::Null);
            }

            return serde_json::from_slice(&bytes).map_err(|error| ApiError {
                status: status.as_u16(),
                body: format!("Response is not JSON: {error}"),
            });
        }

        Err(ApiError {
            status: 429,
            body: "Discord is rate limiting, try again later.".into(),
        })
    }

    pub async fn get_me(&self) -> Result<User> {
        let value = self
            .send(Method::GET, "/users/@me", None)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(User::from_json(&value))
    }

    pub async fn get_quests(&self) -> Result<Vec<Quest>> {
        Ok(self.get_quest_feed().await?.quests)
    }

    pub async fn get_quest_feed(&self) -> Result<QuestFeed> {
        let value = self
            .send(Method::GET, "/quests/@me", None)
            .await
            .map_err(anyhow::Error::from)?;
        parse_quest_feed(&value)
    }

    pub async fn enroll(&self, quest_id: &str) -> Result<()> {
        let path = format!("/quests/{quest_id}/enroll");
        let payload = json!({
            "location": 11,
            "is_targeted": false,
            "metadata_raw": null
        });

        match self.send(Method::POST, &path, Some(payload)).await {
            Ok(_) => Ok(()),
            Err(error) if already_done(&error) => Ok(()),
            Err(first_error) => {
                let minimal = json!({ "location": 11 });
                match self.send(Method::POST, &path, Some(minimal)).await {
                    Ok(_) => Ok(()),
                    Err(error) if already_done(&error) => Ok(()),
                    Err(error) => Err(anyhow!("Could not accept quest: {first_error} | {error}")),
                }
            }
        }
    }

    pub async fn update_video_progress(&self, quest_id: &str, timestamp: f64) -> Result<bool> {
        let path = format!("/quests/{quest_id}/video-progress");
        let payload = json!({ "timestamp": (timestamp * 1000.0).round() / 1000.0 });
        let value = self
            .send(Method::POST, &path, Some(payload))
            .await
            .map_err(anyhow::Error::from)?;
        Ok(completed_at(&value))
    }

    pub async fn send_game_heartbeat(
        &self,
        quest_id: &str,
        application_id: &str,
        terminal: bool,
        executable_path: Option<&str>,
        executable_fingerprint: Option<&str>,
    ) -> Result<HeartbeatOutcome> {
        let path = format!("/quests/{quest_id}/heartbeat");
        let mut payload = json!({
            "application_id": application_id,
            "terminal": terminal
        });
        if let Some(path) = executable_path {
            payload["executable_path"] = Value::String(path.to_string());
        }
        if let Some(fingerprint) = executable_fingerprint {
            payload["executable_fingerprint"] = Value::String(fingerprint.to_string());
        }

        let value = self
            .send(Method::POST, &path, Some(payload))
            .await
            .map_err(anyhow::Error::from)?;
        Ok(parse_heartbeat(&value))
    }

    pub async fn send_stream_heartbeat(
        &self,
        quest_id: &str,
        stream_key: &str,
    ) -> Result<HeartbeatOutcome> {
        let path = format!("/quests/{quest_id}/heartbeat");
        let payload = json!({ "stream_key": stream_key });
        let value = self
            .send(Method::POST, &path, Some(payload))
            .await
            .map_err(anyhow::Error::from)?;
        Ok(parse_heartbeat(&value))
    }

    pub async fn send_activity_heartbeat(
        &self,
        quest_id: &str,
        application_id: &str,
        terminal: bool,
    ) -> Result<HeartbeatOutcome> {
        let path = format!("/quests/{quest_id}/heartbeat");
        let payload = json!({
            "application_id": application_id,
            "terminal": terminal
        });
        let value = self
            .send(Method::POST, &path, Some(payload))
            .await
            .map_err(anyhow::Error::from)?;
        Ok(parse_heartbeat(&value))
    }

    pub async fn claim_reward(&self, quest_id: &str) -> Result<Value> {
        let path = format!("/quests/{quest_id}/claim-reward");
        match self.send(Method::POST, &path, Some(json!({}))).await {
            Ok(value) => Ok(value),
            Err(error) if error.body.to_ascii_lowercase().contains("platform") => self
                .send(Method::POST, &path, Some(json!({ "platform": "desktop" })))
                .await
                .map_err(anyhow::Error::from),
            Err(error) => Err(anyhow::Error::from(error)),
        }
    }

    /// Fetch the application name and a plausible host-OS executable. This is
    /// what makes a synthesized game fingerprint believable to Discord.
    pub async fn app_metadata(&self, application_id: &str) -> Result<AppMetadata> {
        let path = format!("/applications/public?application_ids={application_id}");
        let value = self
            .send(Method::GET, &path, None)
            .await
            .map_err(anyhow::Error::from)?;
        let app = value
            .as_array()
            .and_then(|apps| apps.first())
            .ok_or_else(|| anyhow!("No metadata found for application {application_id}"))?;

        let name = app
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Discord Game")
            .to_string();

        let executables: Vec<(String, String)> = app
            .get("executables")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        Some((
                            entry.get("os")?.as_str()?.to_string(),
                            entry.get("name")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let (exe_path, is_pe64) = fingerprint::resolve_executable(&name, &executables)
            .ok_or_else(|| anyhow!("Application has no suitable executable"))?;

        Ok(AppMetadata {
            name,
            exe_path,
            is_pe64,
        })
    }
}

fn parse_quest_feed(value: &Value) -> Result<QuestFeed> {
    let array = value
        .as_array()
        .or_else(|| value.get("quests").and_then(Value::as_array))
        .ok_or_else(|| {
            anyhow!(
                "Discord did not return a quest list (got {})",
                describe(value)
            )
        })?;

    let raw = array.len();
    let mut dropped = 0;
    let quests = array
        .iter()
        .filter_map(|item| {
            let parsed = Quest::from_json(item);
            if parsed.is_none() {
                dropped += 1;
            }
            parsed
        })
        .collect();

    Ok(QuestFeed {
        quests,
        raw,
        dropped,
        excluded: value
            .get("excluded_quests")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        blocked_until: value
            .get("quest_enrollment_blocked_until")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn describe(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let keys = map
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            format!("object with keys [{keys}]")
        }
        Value::Array(items) => format!("array with {} items", items.len()),
        Value::Null => "null".into(),
        Value::Bool(_) => "boolean".into(),
        Value::Number(_) => "number".into(),
        Value::String(text) => format!("string of {} chars", text.chars().count()),
    }
}

fn completed_at(value: &Value) -> bool {
    value
        .get("completed_at")
        .map(|completed| !completed.is_null())
        .unwrap_or(false)
}

fn parse_heartbeat(value: &Value) -> HeartbeatOutcome {
    let progress = value
        .get("progress")
        .and_then(Value::as_object)
        .and_then(|progress| {
            progress
                .values()
                .find_map(|entry| entry.get("value").and_then(Value::as_f64))
        })
        .or_else(|| value.get("stream_progress_seconds").and_then(Value::as_f64));

    let nested_completed = value
        .get("progress")
        .and_then(Value::as_object)
        .map(|progress| {
            progress.values().any(|entry| {
                entry
                    .get("completed_at")
                    .map(|completed| !completed.is_null())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    HeartbeatOutcome {
        completed: completed_at(value) || nested_completed,
        progress,
    }
}

fn already_done(error: &ApiError) -> bool {
    let body = error.body.to_ascii_lowercase();
    body.contains("already")
        || body.contains("enrolled")
        || error.status == 204
        || error.status == 409
}

fn snippet(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() > 360 {
        format!("{}…", clean.chars().take(360).collect::<String>())
    } else if clean.is_empty() {
        "<empty response>".into()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quest(id: &str) -> Value {
        json!({
            "id": id,
            "config": {
                "messages": { "quest_name": "Sample quest", "game_title": "Game" },
                "application": { "id": "42", "name": "Game" },
                "task_config_v2": { "tasks": {
                    "WATCH_VIDEO": { "type": "WATCH_VIDEO", "target": 120 }
                } }
            },
            "user_status": null
        })
    }

    #[test]
    fn parses_envelope_and_keeps_diagnostics() {
        let value = json!({
            "quests": [quest("1"), { "unexpected": true }],
            "excluded_quests": [{ "id": "2" }, { "id": "3" }],
            "quest_enrollment_blocked_until": "2026-11-06T00:00:00+00:00"
        });

        let feed = parse_quest_feed(&value).expect("feed parses");
        assert_eq!(feed.raw, 2);
        assert_eq!(feed.quests.len(), 1);
        assert_eq!(feed.dropped, 1);
        assert_eq!(feed.excluded, 2);
        assert_eq!(
            feed.blocked_until.as_deref(),
            Some("2026-11-06T00:00:00+00:00")
        );
        assert_eq!(feed.quests[0].target, 120.0);
    }

    #[test]
    fn parses_bare_array() {
        let value = json!([quest("1")]);
        let feed = parse_quest_feed(&value).expect("array parses");
        assert_eq!(feed.raw, 1);
        assert_eq!(feed.quests.len(), 1);
        assert_eq!(feed.excluded, 0);
    }

    #[test]
    fn rejects_unexpected_shape_with_useful_error() {
        let error = parse_quest_feed(&json!({ "message": "Unauthorized" }))
            .expect_err("shape is invalid")
            .to_string();
        assert!(error.contains("message"), "{error}");
    }
}
