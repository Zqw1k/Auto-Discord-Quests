//! Sign in by asking the real Discord client for its token.
//!
//! The desktop client ignores `--remote-debugging-port` unless it was started
//! with that flag. So this module closes Discord, relaunches it with a local
//! debugging port, and speaks the DevTools protocol over a loopback WebSocket.
//! The token is read from the live webpack store — the same snippet the help
//! window tells you to paste by hand.
//!
//! Windows only: locating the install and terminating the running client use
//! Win32 conventions, and the other platforms are not tested.

/// Loopback port used for the DevTools endpoint.
const PORT: u16 = 9223;

/// How long Discord may take to boot before we give up.
const BOOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

pub fn supported() -> bool {
    cfg!(windows)
}

/// Closes and relaunches Discord, then returns the token of the logged-in
/// account. `progress` receives short status lines for the UI.
pub fn sign_in(progress: impl Fn(&str)) -> Result<String, String> {
    #[cfg(windows)]
    {
        windows::sign_in(&progress)
    }
    #[cfg(not(windows))]
    {
        let _ = progress;
        Err("Signing in through the Discord app is only available on Windows.".into())
    }
}

#[cfg(windows)]
mod windows {
    use super::{BOOT_TIMEOUT, PORT};
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// A Discord installation with the profile it writes to.
    struct Install {
        label: &'static str,
        exe: PathBuf,
        process: &'static str,
    }

    pub fn sign_in(progress: &impl Fn(&str)) -> Result<String, String> {
        // If the client is already exposing DevTools (started with the flag by
        // hand), use it instead of restarting anything.
        if let Some(targets) = devtools_targets() {
            let Some((_, socket)) = page_target(&targets) else {
                return Err(format!(
                    "Port {PORT} is used by another application. Close it and try again."
                ));
            };
            progress("Reading the token from Discord…");
            return evaluate(&socket);
        }

        let Some(install) = pick_install() else {
            return Err(
                "Could not find a Discord installation. Install the Discord app and log in first."
                    .into(),
            );
        };

        progress("Closing Discord…");
        let _ = Command::new("taskkill")
            .args(["/IM", install.process, "/F", "/T"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // Windows needs a moment to release the profile lock.
        std::thread::sleep(Duration::from_millis(1200));

        progress(&format!(
            "Starting {} with a local debugging port…",
            install.label
        ));
        Command::new(&install.exe)
            .arg(format!("--remote-debugging-port={PORT}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Could not start Discord: {error}"))?;

        let socket = wait_for_target(Instant::now() + BOOT_TIMEOUT, progress)?;
        progress("Reading the token from Discord…");

        // The page appears before the webpack store holds the session, so the
        // script has to be retried for a few seconds.
        let mut last = String::new();
        for attempt in 0..12 {
            match evaluate(&socket) {
                Ok(token) => return Ok(token),
                Err(error) => last = error,
            }
            if attempt < 11 {
                std::thread::sleep(Duration::from_millis(1000));
            }
        }
        Err(last)
    }

    /// Finds installed Discord builds, preferring the one with an existing
    /// profile (that is the build the user actually logged in to).
    fn pick_install() -> Option<Install> {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
        let roaming = std::env::var_os("APPDATA").map(PathBuf::from)?;

        let builds = [
            ("Discord", "Discord", "Discord.exe", "discord"),
            ("Discord PTB", "DiscordPTB", "DiscordPTB.exe", "discordptb"),
            (
                "Discord Canary",
                "DiscordCanary",
                "DiscordCanary.exe",
                "discordcanary",
            ),
        ];

        let mut with_profile = Vec::new();
        let mut without_profile = Vec::new();
        for (label, folder, process, profile_folder) in builds {
            let Some(exe) = latest_exe(&local.join(folder), process) else {
                continue;
            };
            let install = Install {
                label,
                exe,
                process,
            };
            let leveldb = roaming
                .join(profile_folder)
                .join("Local Storage")
                .join("leveldb");
            if leveldb.is_dir() {
                with_profile.push(install);
            } else {
                without_profile.push(install);
            }
        }

        with_profile.into_iter().chain(without_profile).next()
    }

    /// Newest `app-*` directory that contains the client executable.
    fn latest_exe(folder: &Path, exe_name: &str) -> Option<PathBuf> {
        let mut best: Option<(String, PathBuf)> = None;
        for entry in std::fs::read_dir(folder).ok()?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("app-") {
                continue;
            }
            let exe = entry.path().join(exe_name);
            if !exe.is_file() {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|(current, _)| name.as_str() > current.as_str())
            {
                best = Some((name, exe));
            }
        }
        best.map(|(_, exe)| exe)
    }

    /// `GET /json/list` on the DevTools HTTP endpoint.
    ///
    /// The endpoint answers with `Content-Length` but keeps the connection open,
    /// so reading until EOF would stall until the socket timeout. Read the
    /// framed body instead.
    fn devtools_targets() -> Option<serde_json::Value> {
        let address = ("127.0.0.1", PORT).to_socket_addrs().ok()?.next()?;
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(400)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .ok()?;
        write!(
            stream,
            "GET /json/list HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
        )
        .ok()?;

        let mut response = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut header_end = None;
        let mut expected = None;

        loop {
            if header_end.is_none() {
                if let Some(position) = find_header_end(&response) {
                    if !status_ok(&response[..position]) {
                        return None;
                    }
                    header_end = Some(position);
                    expected = content_length(&response[..position]);
                }
            }
            if let (Some(position), Some(length)) = (header_end, expected) {
                if response.len() >= position + length {
                    return serde_json::from_slice(&response[position..position + length]).ok();
                }
            }
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => response.extend_from_slice(&buffer[..count]),
                // Windows can report a reset after the body has arrived.
                Err(_) => break,
            }
        }

        let position = header_end?;
        serde_json::from_slice(&response[position..]).ok()
    }

    /// Offset just past the blank line that ends the HTTP headers.
    fn find_header_end(bytes: &[u8]) -> Option<usize> {
        bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| position + 4)
    }

    fn status_ok(head: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(head) else {
            return false;
        };
        text.starts_with("HTTP/")
            && text
                .lines()
                .next()
                .is_some_and(|line| line.contains(" 200"))
    }

    fn content_length(head: &[u8]) -> Option<usize> {
        let text = std::str::from_utf8(head).ok()?;
        text.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })
    }

    /// First page target that belongs to Discord, as `(url, ws url)`.
    fn page_target(targets: &serde_json::Value) -> Option<(String, String)> {
        for target in targets.as_array()? {
            if target.get("type")?.as_str()? != "page" {
                continue;
            }
            let url = target.get("url")?.as_str()?;
            if !url.contains("discord.com") {
                continue;
            }
            let socket = target.get("webSocketDebuggerUrl")?.as_str()?;
            if socket.is_empty() {
                continue;
            }
            return Some((url.to_string(), socket.to_string()));
        }
        None
    }

    fn wait_for_target(deadline: Instant, progress: &impl Fn(&str)) -> Result<String, String> {
        let mut redirected = false;
        loop {
            if let Some(targets) = devtools_targets() {
                if let Some((url, socket)) = page_target(&targets) {
                    // Discord sometimes lands on the login page first. Keep
                    // waiting for the app shell instead of failing early.
                    if url.contains("/login") {
                        if !redirected {
                            progress("Waiting for your Discord session…");
                            redirected = true;
                        }
                    } else {
                        return Ok(socket);
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(
                    "Discord did not expose its DevTools endpoint in time. Try again or paste the token manually."
                        .into(),
                );
            }
            std::thread::sleep(Duration::from_millis(700));
        }
    }

    /// Reads the token out of the live client. Same logic as the snippet in
    /// the help window: walk the webpack store looking for `getToken()`.
    const TOKEN_SCRIPT: &str = r#"(() => {
  try {
    let token = null;
    const take = (value) => {
      try {
        const found = value?.getToken?.();
        if (typeof found === "string" && found.length > 50 && !/\s/.test(found) && found.split(".").length > 1) {
          token = found;
          return true;
        }
      } catch {}
      return false;
    };
    const walk = (exports) => {
      if (!exports || exports === window) return false;
      if (take(exports)) return true;
      for (const value of Object.values(exports)) {
        if (take(value)) return true;
      }
      return false;
    };
    webpackChunkdiscord_app.push([[Symbol()], {}, (module) => {
      for (const entry of Object.values(module.c ?? {})) {
        if (walk(entry?.exports)) return;
      }
    }]);
    webpackChunkdiscord_app.pop();
    return token;
  } catch (error) {
    return null;
  }
})()"#;

    /// Sends `Runtime.evaluate` over the DevTools WebSocket and parses the reply.
    fn evaluate(socket_url: &str) -> Result<String, String> {
        use tungstenite::stream::MaybeTlsStream;
        use tungstenite::{connect, Message};

        let (mut socket, _) = connect(socket_url)
            .map_err(|error| format!("Could not open the DevTools socket: {error}"))?;

        #[allow(unreachable_patterns)]
        let plain = match socket.get_ref() {
            MaybeTlsStream::Plain(stream) => Some(stream),
            _ => None,
        };
        if let Some(stream) = plain {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        }

        let request = serde_json::json!({
            "id": 1,
            "method": "Runtime.evaluate",
            "params": {
                "expression": TOKEN_SCRIPT,
                "returnByValue": true,
                "awaitPromise": false,
                "userGesture": true,
            }
        });
        socket
            .send(Message::Text(request.to_string()))
            .map_err(|error| format!("Could not send the DevTools request: {error}"))?;

        loop {
            match socket.read() {
                Ok(Message::Text(text)) => {
                    let Ok(reply) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    if reply.get("id").and_then(|id| id.as_u64()) != Some(1) {
                        continue;
                    }
                    if let Some(error) = reply.get("error") {
                        return Err(format!("Discord returned a DevTools error: {error}"));
                    }
                    if let Some(details) = reply.pointer("/result/exceptionDetails") {
                        return Err(format!("The token script failed inside Discord: {details}"));
                    }
                    let result = reply.pointer("/result/result");
                    let kind = result
                        .and_then(|value| value.get("type"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default();
                    let token = result
                        .and_then(|value| value.get("value"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default();
                    if kind == "string" && crate::discord::token::looks_like_token(token) {
                        return Ok(token.to_string());
                    }
                    return Err(
                        "Discord did not return a token. Make sure you are logged in to the Discord app, then try again."
                            .into(),
                    );
                }
                Ok(Message::Ping(payload)) => {
                    let _ = socket.send(Message::Pong(payload));
                }
                Ok(Message::Close(_)) => {
                    return Err("Discord closed the DevTools connection.".into());
                }
                Ok(_) => {}
                Err(error) => {
                    return Err(format!("Could not read from the DevTools socket: {error}"));
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn splits_headers_and_body() {
            let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
            let end = find_header_end(raw).expect("headers end");
            assert_eq!(&raw[end..], b"hello");
            assert!(status_ok(&raw[..end]));
            assert_eq!(content_length(&raw[..end]), Some(5));
        }

        #[test]
        fn rejects_error_status() {
            let raw = b"HTTP/1.1 404 Not Found\r\n\r\n";
            let end = find_header_end(raw).expect("headers end");
            assert!(!status_ok(&raw[..end]));
        }
    }
}
