# QuestPilot

A compact desktop app (Rust + egui) that completes **Discord Quests** automatically — same mechanism as
[Masterain98/discord-quest-helper](https://github.com/Masterain98/discord-quest-helper), but without
Tauri/WebView: the backend and UI live in a single `.exe` (fonts embedded).

> ⚠️ **Warning**: Automating quests violates Discord's Terms of Service. Your account may lose quest access
> or be suspended. Use at your own risk. Never share your token with anyone.

| Login | Quests |
| --- | --- |
| ![Login](docs/screenshots/login.png) | ![Quests](docs/screenshots/quests.png) |

| Settings | Logs |
| --- | --- |
| ![Settings](docs/screenshots/settings.png) | ![Logs](docs/screenshots/logs.png) |

## Install

Packaged builds (`.zip`, `.msi`, `.deb`, `.AppImage`, `.dmg`) are attached to each
[GitHub release](https://github.com/Zqw1k/Auto-Discord-Quests/releases).

### Windows

Download `questpilot-*-windows-x86_64.zip`, extract it and run `questpilot.exe`.
There is also an optional `questpilot-*-windows-x86_64.msi` installer that installs the app
for all users and adds a Start Menu shortcut.
The binaries are unsigned, so SmartScreen may warn — choose *More info → Run anyway*.

### Linux

- **AppImage** (any distro): `chmod +x questpilot-*-linux-x86_64.AppImage && ./questpilot-*-linux-x86_64.AppImage`
- **Debian / Ubuntu**: `sudo apt install ./questpilot_*_amd64.deb`
- **Portable**: extract `questpilot-*-linux-x86_64.tar.gz` and run `./questpilot/questpilot`

Runtime libraries: X11 or Wayland, `libxkbcommon` and OpenGL (`libgl1`). On a minimal
distribution install e.g. `libx11-6 libxkbcommon0 libgl1 libwayland-client0`.

The AppImage needs FUSE 2 on some distributions (`libfuse2` on Debian/Ubuntu). If it
cannot mount, run it with `--appimage-extract-and-run`.

Linux packages are built on Ubuntu 22.04 and require glibc 2.35 or newer.

### macOS

Download `QuestPilot-*-macos-universal.dmg`, open it and drag **QuestPilot** to
**Applications**. The build is universal (Apple Silicon + Intel, macOS 11+).

The app is not signed or notarized, so the first launch needs *right-click → Open*, or
run once:

```bash
xattr -dr com.apple.quarantine /Applications/QuestPilot.app
```

## Features

| Quest type | How it works | Status |
| --- | --- | --- |
| `WATCH_VIDEO` | `POST /quests/{id}/video-progress` at timestamp steps with slight jitter | ✅ |
| `PLAY_ON_DESKTOP` | Fetches app metadata → builds a valid `executable_path` + `executable_fingerprint` → spawns a real idle runner process → heartbeat every ~60s | ✅ |
| `PLAY_ACTIVITY` | Activity heartbeat, reads progress from Discord's response | ✅ |
| `STREAM_ON_DESKTOP` | Discord requires a real stream with viewers in the voice channel | ❌ |
| `ACHIEVEMENT_IN_*` | Must be unlocked inside the actual game/activity | ❌ |

- Sign in by pasting your token; the token **stays in RAM**, is never written to disk and is never sent
  anywhere except `discord.com`.
- **Token detection (Windows)**: click *Detect on this device* to read the token the Discord app already
  stored, decrypted through DPAPI — only when you ask for it. Nothing is read at startup.
- **Saved accounts**: every successful sign-in is remembered in memory, so you can switch accounts with
  one click from the sign-in screen (sign out keeps the list; closing the app clears it).
- **Do all**: queues open quests and runs them one by one; you can stop at any time.
- Auto-claim rewards on completion (optional), refresh the list when you return to the window.
- Filters by state (All / Open / Done) and by reward type (Nitro / Orbs / Decoration / Other), plus search,
  step-by-step logs and toast notifications.
- Expired quests are hidden automatically.
- Dark UI, Be Vietnam Pro font, hand-drawn vector icons, no external image dependencies.

## Build

Requires stable Rust. The same commands work on Linux and macOS. Two options on Windows:

**Option 1 — MSVC (standard, requires Visual Studio Build Tools)**

Install *Build Tools for Visual Studio* with the **Desktop development with C++** workload, then:

```powershell
cargo build --release
```

**Option 2 — GNU (lightweight, no Visual Studio)**

```powershell
winget install BrechtSanders.WinLibs.POSIX.UCRT
rustup toolchain install stable-x86_64-pc-windows-gnu
```

Add `...\WinLibs...\mingw64\bin` to `PATH`, then:

```powershell
cargo +stable-x86_64-pc-windows-gnu build --release
```

Binary: `target/release/questpilot.exe`.

### Demo mode

Build debug and run with sample data (no token, no network):

```powershell
$env:QUESTPILOT_DEMO = "1"          # enable sample data
$env:QUESTPILOT_DEMO_VIEW = "logs"  # quests | empty | settings | logs | help
cargo run
```

## Releasing

Push a version tag and the release workflow builds all packages and attaches them to a
GitHub release. File names follow `version` in `Cargo.toml`, so bump it first:

```bash
git tag v1.2.0
git push origin v1.2.0
```

## Getting your token

**Option 0 — Detect on this device (Windows)**

On the sign-in screen click *Detect on this device*. QuestPilot reads the token the Discord app already
stored in its profile, decrypts it with your Windows DPAPI key and lists it as a saved account. It only
runs when you click, and nothing is written back.

**Option 1 — Discord desktop**

1. Open Discord desktop and press `Ctrl + Shift + I`.
2. Switch to the **Console** tab, paste the snippet below, then press Enter:

```js
(() => { let t; webpackChunkdiscord_app.push([[Symbol()], {}, o => { for (const e of Object.values(o.c ?? {})) { try { const x = e.exports; if (!x || x === window) continue; for (const c of [x, ...Object.values(x)]) { try { const v = c?.getToken?.(); if (typeof v === "string" && v.length > 50 && !/\s/.test(v) && v.split(".").length > 1) { t = v; return; } } catch {} } } catch {} } }]); webpackChunkdiscord_app.pop(); return t })()
```

3. Copy the string it prints and paste it into QuestPilot.
4. If it prints `undefined` or an object, press `Ctrl + R` in Discord and run the snippet again.

**Option 2 — Network tab**

DevTools → Network → select a request to `discord.com/api` → check the `Authorization` header.

## Project layout

```
src/
  main.rs              creates the egui window
  app.rs               UI: login, quest list, settings, logs, toasts
  theme.rs             design tokens, Be Vietnam Pro font, vector icons, gradients
  models.rs            parses Discord quest JSON into compact models
  config.rs            settings stored in the OS config dir (QuestPilot/settings.json)
  worker.rs            tokio worker: login, refresh, quest execution, queue, events
  discord/
    api.rs             REST client (quests, enroll, video-progress, heartbeat, claim)
    fingerprint.rs     rebuilds the 146-byte / 196-char base64 executable fingerprint
    token.rs           detects tokens stored by the local Discord app (Windows, DPAPI)
assets/fonts/          Be Vietnam Pro (OFL)
assets/icon.png        app icon used by packages
packaging/             Linux .desktop entry and macOS Info.plist
wix/                   Windows MSI definition (cargo-wix)
scripts/shot.ps1       captures the app window (used for README screenshots)
docs/screenshots/      UI screenshots
.github/workflows/     CI and release packaging (.deb, .AppImage, .dmg, .zip)
```

## No quests showing?

1. Open the **Logs** tab — the `Loaded … quests (… available, … raw entries)` line shows what Discord actually returned.
2. If the list is empty and you see `… excluded quests`, your account/region is not eligible for those quests.
   Discord limits quests by country and age; a VPN won't help because eligibility is tied to the account.
3. If you see `HTTP 401`, the token is expired or invalid — sign out and get a new token.
4. On network errors, check connectivity to `discord.com` (a VPN/proxy may block it).
5. The **Reload** button on the empty state re-calls the API immediately.

While testing the UI, you can preview the empty state with
`$env:QUESTPILOT_DEMO_VIEW = "empty"`.

## Technical notes

- **Video**: each request sends an increasing `timestamp` (seconds); Discord returns `completed_at` when done.
  Speed and delay are configured in **Settings** (1×–30×, 2–30 seconds). **Safe mode** caps progress to real time.
- **Game**: `executable_path` is trimmed to its last two segments (matching Discord's normalization); the
  fingerprint is a 146-byte structure (PE header + hardware hash + section + signature) XORed with SHA-256
  of `user_id` + salt, then base64-encoded — the same format the desktop client sends in its heartbeat.
  A second, idle copy of the app (`questpilot --game-runner <seconds>`) is started for the duration of the
  quest and killed when it ends, so a real process exists while playtime is reported.
- **Activity**: ~60-second heartbeat, progress read from `progress.PLAY_ACTIVITY.value`.
- Heartbeat and video-progress delays carry random jitter so the request cadence is not perfectly regular.
- Every request includes `x-super-properties`, `referer: https://discord.com/quest-home` and the Discord
  client UA; 429 responses are retried once.
- The client identity is pinned to the Windows desktop build so the same quest flow works on every OS.

## License

MIT. The fingerprint reverse-engineering part is based on public documentation from the
`discord-quest-helper` project (MIT).
