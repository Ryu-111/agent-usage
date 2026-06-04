use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ClaudeCredentials {
    #[serde(alias = "accessToken", alias = "access_token")]
    pub access_token: String,
    #[serde(default, alias = "expiresAt", alias = "expires_at")]
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
    let creds: ClaudeCredentials = serde_json::from_str(&raw)?;
    Ok(Some(creds))
}
