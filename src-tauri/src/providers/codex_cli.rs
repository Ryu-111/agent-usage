use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use serde_json::json;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::core::model::SnapshotSource;
use crate::providers::rate_limits::{unix_seconds, RateLimitReading, RateLimitWindow};

#[derive(Debug, Clone)]
pub struct AppServerClient {
    pub command: Vec<String>,
    pub timeout: Duration,
}

impl AppServerClient {
    pub fn default_codex() -> Self {
        Self {
            command: default_codex_command(),
            timeout: Duration::from_secs(10),
        }
    }

    pub async fn fetch_rate_limits(&self) -> anyhow::Result<RateLimitReading> {
        tokio::time::timeout(self.timeout, self.fetch_rate_limits_inner())
            .await
            .context("codex app-server timed out")?
    }

    async fn fetch_rate_limits_inner(&self) -> anyhow::Result<RateLimitReading> {
        let (program, args) = self
            .command
            .split_first()
            .ok_or_else(|| anyhow!("codex app-server command is empty"))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn {}", program))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to open codex app-server stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to open codex app-server stdout"))?;
        let mut reader = BufReader::new(stdout);

        write_request(
            &mut stdin,
            1,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "agent-usage",
                    "title": "agent-usage",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
        .await?;

        let initialize = read_response(&mut reader, 1).await?;
        if let Some(reading) = parse_rate_limits_response(&initialize) {
            return Ok(reading);
        }

        write_request(
            &mut stdin,
            2,
            "account/rateLimits/read",
            serde_json::Value::Object(Default::default()),
        )
        .await?;
        let response = read_response(&mut reader, 2).await?;
        parse_rate_limits_response(&response)
            .ok_or_else(|| anyhow!("codex app-server response did not contain rateLimits"))
    }
}

fn default_codex_command() -> Vec<String> {
    let codex = std::env::var_os("CODEX_BIN")
        .map(PathBuf::from)
        .or_else(|| find_in_path("codex"))
        .or_else(|| existing_path("/opt/homebrew/bin/codex"))
        .or_else(|| existing_path("/usr/local/bin/codex"))
        .or_else(|| dirs::home_dir().and_then(|home| existing_path(home.join(".local/bin/codex"))))
        .unwrap_or_else(|| PathBuf::from("codex"));

    vec![
        codex.display().to_string(),
        "-s".to_string(),
        "read-only".to_string(),
        "-a".to_string(),
        "untrusted".to_string(),
        "app-server".to_string(),
    ]
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|path| path.join(name))
            .find(|path| path.is_file())
    })
}

fn existing_path(path: impl Into<PathBuf>) -> Option<PathBuf> {
    let path = path.into();
    path.is_file().then_some(path)
}

async fn write_request(
    stdin: &mut tokio::process::ChildStdin,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> anyhow::Result<()> {
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    stdin.write_all(request.to_string().as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    Ok(())
}

async fn read_response<R>(reader: &mut R, expected_id: u64) -> anyhow::Result<serde_json::Value>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).await?;
        if read == 0 {
            return Err(anyhow!("codex app-server exited before response"));
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
            if value.get("id").and_then(serde_json::Value::as_u64) != Some(expected_id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(anyhow!("codex app-server JSON-RPC error: {}", error));
            }
            return Ok(value);
        }
    }
}

pub fn parse_rate_limits_response(value: &serde_json::Value) -> Option<RateLimitReading> {
    let limits = value
        .pointer("/result/rateLimits")
        .or_else(|| value.pointer("/result"))
        .or_else(|| value.pointer("/rateLimits"))?;

    Some(RateLimitReading {
        primary: parse_limit(
            limits
                .get("primary")
                .or_else(|| limits.get("five_hour"))
                .or_else(|| limits.get("fiveHour")),
        ),
        secondary: parse_limit(
            limits
                .get("secondary")
                .or_else(|| limits.get("seven_day"))
                .or_else(|| limits.get("weekly")),
        ),
        observed_at: Some(Utc::now()),
        source: SnapshotSource::OfficialCli,
    })
    .filter(|reading| reading.primary.is_some() || reading.secondary.is_some())
}

fn parse_limit(value: Option<&serde_json::Value>) -> Option<RateLimitWindow> {
    let value = value?;
    Some(RateLimitWindow {
        used_percent: value
            .get("usedPercent")
            .or_else(|| value.get("used_percent"))
            .or_else(|| value.get("used_percentage"))
            .and_then(serde_json::Value::as_f64),
        window_minutes: value
            .get("windowDurationMins")
            .or_else(|| value.get("window_minutes"))
            .or_else(|| value.get("windowMinutes"))
            .and_then(serde_json::Value::as_i64),
        resets_at: parse_reset_at(value),
    })
}

fn parse_reset_at(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    value
        .get("resetsAt")
        .or_else(|| value.get("resets_at"))
        .and_then(|value| {
            value
                .as_i64()
                .and_then(unix_seconds)
                .or_else(|| value.as_str().and_then(parse_datetime))
        })
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::core::model::SnapshotSource;

    use super::parse_rate_limits_response;

    #[test]
    fn parses_app_server_rate_limits_response() {
        let parsed = parse_rate_limits_response(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "rateLimits": {
                    "primary": {
                        "usedPercent": 42.5,
                        "windowDurationMins": 300,
                        "resetsAt": 1_774_036_800
                    },
                    "secondary": {
                        "usedPercent": 85.0,
                        "windowDurationMins": 10080,
                        "resetsAt": 1_774_580_400
                    }
                }
            }
        }))
        .unwrap();

        assert_eq!(parsed.source, SnapshotSource::OfficialCli);
        assert_eq!(parsed.primary.unwrap().used_percent, Some(42.5));
        assert_eq!(parsed.secondary.unwrap().window_minutes, Some(10080));
    }
}
