//! Discord client compatibility helpers.
//!
//! `executable_fingerprint` is a 146-byte structure that Discord's desktop
//! client signs for `PLAY_ON_DESKTOP` quest heartbeats. The layout and the
//! XOR stream are re-implemented here so the REST API accepts heartbeats sent
//! by this app without a running game. The format follows the public
//! reverse-engineering notes from the discord-quest-helper project (MIT).

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

const FINGERPRINT_LEN: usize = 0x92;
const FINGERPRINT_SALT: &str = "https://discord.com/careers";

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hardware_prefix() -> &'static str {
    static PREFIX: OnceLock<String> = OnceLock::new();
    PREFIX.get_or_init(|| {
        let cores = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(8);
        format!("hw:{}:{}:{}:", host_os_tag(), std::env::consts::ARCH, cores)
    })
}

fn renderer_hardware_token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| {
        hex(&sha256(
            format!("dqh-renderer-hw-v1:{}", hardware_prefix()).as_bytes(),
        ))
    })
}

fn hardware_sha256(user_id: &str) -> [u8; 32] {
    sha256(format!("{}:{user_id}", renderer_hardware_token()).as_bytes())
}

/// Mirror Discord's `Module 973522` executable path normalisation: lowercase,
/// forward slashes, and only the last two path segments.
pub fn format_executable_path(path: &str) -> String {
    let normalized = path.trim().to_lowercase().replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() >= 2 {
        format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        parts.first().copied().unwrap_or_default().to_string()
    }
}

/// Build the 196-character Base64 fingerprint Discord expects in a
/// `PLAY_ON_DESKTOP` heartbeat.
pub fn synthesize_fingerprint(
    user_id: &str,
    app_id: &str,
    exe_path: &str,
    is_pe64: bool,
) -> String {
    let normalized_path = format_executable_path(exe_path);
    let seed = format!("{app_id}:{normalized_path}");
    let seed_hash = sha256(format!("seed:{seed}").as_bytes());

    let base_mb =
        20u64 + (u32::from_le_bytes(seed_hash[0..4].try_into().unwrap_or([0; 4])) as u64 % 120);
    let extra_bytes =
        (u32::from_le_bytes(seed_hash[4..8].try_into().unwrap_or([0; 4])) as u64 % 1024) * 512;
    let file_size = base_mb * 1024 * 1024 + extra_bytes;

    let time_date_stamp = 1_704_067_200u32.wrapping_add(
        u32::from_le_bytes(seed_hash[8..12].try_into().unwrap_or([0; 4])) % 55_000_000,
    );
    let pages = 0x3800u32
        + (u16::from_le_bytes(seed_hash[12..14].try_into().unwrap_or([0; 2])) as u32 % 0x4000);
    let size_of_image = pages.wrapping_mul(0x1000);

    let headers =
        sha256(format!("PE_HEADERS_V1:{seed}:{time_date_stamp}:{size_of_image}").as_bytes());
    let sections = sha256(format!("PE_SECTIONS_V1:{seed}:{size_of_image}").as_bytes());
    let certificate = sha256(format!("AUTHENTICODE_CERT_V1:{seed}").as_bytes());

    let mut plain = [0u8; FINGERPRINT_LEN];
    plain[0x00] = 0x01;
    plain[0x01] = if is_pe64 { 0x03 } else { 0x01 };
    plain[0x02..0x0A].copy_from_slice(&file_size.to_le_bytes());
    plain[0x0A..0x0E].copy_from_slice(&time_date_stamp.to_le_bytes());
    plain[0x0E..0x12].copy_from_slice(&size_of_image.to_le_bytes());
    plain[0x12..0x32].copy_from_slice(&headers);
    plain[0x32..0x52].copy_from_slice(&hardware_sha256(user_id));
    plain[0x52..0x72].copy_from_slice(&sections);
    plain[0x72..0x92].copy_from_slice(&certificate);

    let xor_key = sha256(format!("{user_id}{FINGERPRINT_SALT}").as_bytes());
    let mut cipher = [0u8; FINGERPRINT_LEN];
    cipher[0] = plain[0];
    for index in 1..FINGERPRINT_LEN {
        cipher[index] = plain[index] ^ xor_key[(index - 1) & 31];
    }

    BASE64.encode(cipher)
}

pub fn host_os_tag() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

fn sanitize_app_name(name: &str) -> String {
    name.chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            other => other,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn app_slug(name: &str) -> String {
    name.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

fn normalize_exe(name: &str) -> String {
    let file = name
        .trim()
        .trim_start_matches('>')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim();
    if cfg!(windows) {
        file.to_string()
    } else {
        file.strip_suffix(".exe")
            .or_else(|| file.strip_suffix(".EXE"))
            .unwrap_or(file)
            .to_string()
    }
}

/// Resolve a plausible executable path and PE bitness from `/applications/public`
/// metadata `(os, name)` pairs.
pub fn resolve_executable(
    app_name: &str,
    executables: &[(String, String)],
) -> Option<(String, bool)> {
    let app = sanitize_app_name(app_name);
    let default_exe = if cfg!(windows) {
        format!("{app}.exe")
    } else {
        app.clone()
    };

    let priorities: &[&str] = if cfg!(windows) {
        &["win32"]
    } else if cfg!(target_os = "macos") {
        &["darwin", "win32"]
    } else {
        &["linux", "win32"]
    };

    let selected = priorities.iter().find_map(|os| {
        executables
            .iter()
            .find(|(entry_os, entry_name)| entry_os == *os && !entry_name.trim().is_empty())
    });

    let (raw_name, is_pe64) = match selected {
        Some((os, name)) => {
            let looks_like_pe = os == "win32" || name.trim().to_ascii_lowercase().ends_with(".exe");
            (name.as_str(), looks_like_pe)
        }
        None => (default_exe.as_str(), cfg!(windows)),
    };

    let exe = normalize_exe(raw_name);
    let path = if cfg!(windows) {
        format!(r"C:\Program Files\{app}\{exe}")
    } else if cfg!(target_os = "macos") {
        format!("/Applications/{app}.app/Contents/MacOS/{exe}")
    } else {
        format!("/opt/{}/{exe}", app_slug(&app))
    };

    if path.trim().is_empty() {
        None
    } else {
        Some((path, is_pe64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_196_base64_chars() {
        let fingerprint = synthesize_fingerprint(
            "123456789012345678",
            "987654321",
            r"C:\Program Files\Game\game.exe",
            true,
        );
        assert_eq!(fingerprint.len(), 196);
        let raw = BASE64.decode(&fingerprint).unwrap();
        assert_eq!(raw.len(), 146);
        assert_eq!(raw[0], 0x01);
    }

    #[test]
    fn executable_path_keeps_two_segments() {
        assert_eq!(
            format_executable_path(r"C:\Program Files\Some Game\Game.exe"),
            "some game/game.exe"
        );
    }

    #[test]
    fn missing_metadata_still_builds_a_path() {
        let (path, pe) = resolve_executable("My Game", &[]).unwrap();
        assert!(path.contains("My Game"));
        assert_eq!(pe, cfg!(windows));
    }
}
