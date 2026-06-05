use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ClaudeCredentials {
    pub access_token: String,
    pub expires_at: Option<DateTime<Utc>>,
}

impl ClaudeCredentials {
    pub fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| expires_at <= Utc::now())
    }
}

pub fn read_claude_credentials() -> anyhow::Result<Option<ClaudeCredentials>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    read_claude_credentials_from(home.join(".claude/.credentials.json"))
}

pub fn read_claude_credentials_from(path: PathBuf) -> anyhow::Result<Option<ClaudeCredentials>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)?;
    Ok(parse_claude_credentials(&raw))
}

/// Parse a Claude Code credentials JSON document.
///
/// The real file written by Claude Code nests the OAuth payload under a
/// `claudeAiOauth` object and stores `expiresAt` as Unix epoch milliseconds, e.g.
/// `{"claudeAiOauth":{"accessToken":"...","expiresAt":1750000000000}}`.
/// Older/flat documents that put the fields at the root with an RFC3339
/// `expires_at` string are still accepted for backward compatibility.
pub fn parse_claude_credentials(raw: &str) -> Option<ClaudeCredentials> {
    let root: Value = serde_json::from_str(raw).ok()?;
    let obj = root.get("claudeAiOauth").unwrap_or(&root);

    let access_token = obj
        .get("accessToken")
        .or_else(|| obj.get("access_token"))
        .and_then(Value::as_str)?
        .to_string();

    let expires_at = obj
        .get("expiresAt")
        .or_else(|| obj.get("expires_at"))
        .and_then(parse_expiry);

    Some(ClaudeCredentials {
        access_token,
        expires_at,
    })
}

fn parse_expiry(value: &Value) -> Option<DateTime<Utc>> {
    match value {
        // Claude Code's real format: Unix epoch milliseconds.
        Value::Number(number) => Utc.timestamp_millis_opt(number.as_i64()?).single(),
        // Legacy/config format: RFC3339 string.
        Value::String(text) => DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|value| value.with_timezone(&Utc)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_claude_credentials;
    use chrono::{TimeZone, Utc};

    #[test]
    fn parses_real_nested_format_with_millis_expiry() {
        let expires_ms = 1_750_000_000_000_i64;
        let raw = format!(
            r#"{{"claudeAiOauth":{{"accessToken":"abc123","refreshToken":"r","expiresAt":{expires_ms},"scopes":["user:inference"]}}}}"#
        );

        let creds = parse_claude_credentials(&raw).expect("should parse");
        assert_eq!(creds.access_token, "abc123");
        assert_eq!(
            creds.expires_at,
            Some(Utc.timestamp_millis_opt(expires_ms).single().unwrap())
        );
    }

    #[test]
    fn parses_legacy_flat_format_with_rfc3339_expiry() {
        let raw = r#"{"access_token":"flat-token","expires_at":"2026-06-04T00:00:00Z"}"#;

        let creds = parse_claude_credentials(raw).expect("should parse");
        assert_eq!(creds.access_token, "flat-token");
        assert!(creds.expires_at.is_some());
    }

    #[test]
    fn detects_expired_token() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"old","expiresAt":1000000000000}}"#;
        let creds = parse_claude_credentials(raw).expect("should parse");
        assert!(creds.is_expired());
    }

    #[test]
    fn missing_access_token_is_none() {
        let raw = r#"{"claudeAiOauth":{"expiresAt":1750000000000}}"#;
        assert!(parse_claude_credentials(raw).is_none());
    }
}
