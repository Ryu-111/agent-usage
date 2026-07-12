use std::path::{Path, PathBuf};

use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use rusqlite::{Connection, OptionalExtension};

pub fn find_claude_session_cookie() -> Option<String> {
    let home = dirs::home_dir()?;
    let chromium_paths = [
        home.join("Library/Application Support/Claude/Cookies"),
        home.join("Library/Application Support/Google/Chrome/*/Cookies"),
        home.join("Library/Application Support/BraveSoftware/Brave-Browser/*/Cookies"),
        home.join("Library/Application Support/Microsoft Edge/*/Cookies"),
    ];
    for pattern in chromium_paths {
        let pattern = pattern.display().to_string();
        for path in glob::glob(&pattern).ok()?.flatten() {
            if let Some(cookie) = read_chromium_cookie(&path) {
                return Some(cookie);
            }
        }
    }

    let firefox_pattern =
        home.join("Library/Application Support/Firefox/Profiles/*/cookies.sqlite");
    for path in glob::glob(&firefox_pattern.display().to_string())
        .ok()?
        .flatten()
    {
        if let Some(cookie) = read_plain_cookie(&path) {
            return Some(cookie);
        }
    }
    None
}

fn read_chromium_cookie(path: &Path) -> Option<String> {
    let copy = copy_for_read(path)?;
    let result = read_cookie_row(&copy, |encrypted, value| {
        if !encrypted.is_empty() {
            decrypt_chromium_cookie(&encrypted)
                .or_else(|| (!value.is_empty()).then(|| value.into_bytes()))
        } else {
            (!value.is_empty()).then(|| value.into_bytes())
        }
    });
    let _ = std::fs::remove_file(copy);
    result
}

fn read_plain_cookie(path: &Path) -> Option<String> {
    let copy = copy_for_read(path)?;
    let result = read_cookie_row(&copy, |_encrypted, value| {
        (!value.is_empty()).then(|| value.into_bytes())
    });
    let _ = std::fs::remove_file(copy);
    result
}

fn read_cookie_row<F>(path: &Path, decode: F) -> Option<String>
where
    F: Fn(Vec<u8>, String) -> Option<Vec<u8>>,
{
    let connection = Connection::open(path).ok()?;
    let mut statement = connection
        .prepare(
            "SELECT encrypted_value, value FROM cookies \
             WHERE host_key LIKE '%claude.ai' AND name = 'sessionKey' \
             ORDER BY last_access_utc DESC LIMIT 1",
        )
        .ok()?;
    let row = statement
        .query_row([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
        })
        .optional()
        .ok()??;
    let value = decode(row.0, row.1)?;
    let value = String::from_utf8(value).ok()?;
    (!value.is_empty()).then(|| format!("sessionKey={value}"))
}

fn copy_for_read(path: &Path) -> Option<PathBuf> {
    if !path.is_file() {
        return None;
    }
    let copy = std::env::temp_dir().join(format!(
        "agent-usage-cookie-{}-{}",
        std::process::id(),
        path.file_name()?.to_string_lossy()
    ));
    std::fs::copy(path, &copy).ok()?;
    Some(copy)
}

fn decrypt_chromium_cookie(encrypted: &[u8]) -> Option<Vec<u8>> {
    if encrypted.len() < 4 || !(encrypted.starts_with(b"v10") || encrypted.starts_with(b"v11")) {
        return None;
    }
    let password = chrome_safe_storage_password()?;
    let mut key = [0_u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password.as_bytes(), b"saltysalt", 1003, &mut key);
    let iv = [b' '; 16];
    let mut buffer = encrypted[3..].to_vec();
    let plaintext = cbc::Decryptor::<aes::Aes128>::new(&key.into(), &iv.into())
        .decrypt_padded_mut::<cbc::cipher::block_padding::Pkcs7>(&mut buffer)
        .ok()?;
    Some(plaintext.to_vec())
}

#[cfg(target_os = "macos")]
fn chrome_safe_storage_password() -> Option<String> {
    use security_framework::os::macos::keychain::SecKeychain;
    use security_framework::os::macos::passwords::find_generic_password;

    let _lock = SecKeychain::disable_user_interaction().ok()?;
    for account in ["Chrome", "Claude", "Electron"] {
        if let Ok((password, _item)) = find_generic_password(None, "Chrome Safe Storage", account) {
            if let Ok(password) = String::from_utf8(password.as_ref().to_vec()) {
                return Some(password);
            }
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn chrome_safe_storage_password() -> Option<String> {
    None
}
