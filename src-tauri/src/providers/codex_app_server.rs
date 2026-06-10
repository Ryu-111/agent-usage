use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::core::model::{RateLimitReading, RateLimitWindow, SnapshotSource, UsageWindow};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Short-lived JSON-RPC client for `codex app-server`. A fresh process is
/// spawned per call; the command vector is swappable so tests can point it
/// at a fake script.
#[derive(Debug, Clone)]
pub struct AppServerClient {
    pub command: Vec<String>,
    pub timeout: Duration,
}

impl AppServerClient {
    pub fn new(command: Vec<String>) -> Self {
        Self {
            command,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn default_codex() -> Option<Self> {
        let binary = std::env::var("CODEX_BIN")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(find_codex_binary)?;
        Some(Self::new(vec![
            binary,
            "-s".into(),
            "read-only".into(),
            "-a".into(),
            "untrusted".into(),
            "app-server".into(),
        ]))
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
            .context("empty app-server command")?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn {program}"))?;

        let mut stdin = child.stdin.take().context("app-server stdin missing")?;
        let stdout = child.stdout.take().context("app-server stdout missing")?;
        let mut lines = BufReader::new(stdout).lines();

        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "agent-usage",
                    "title": "agent-usage",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        });
        stdin
            .write_all(format!("{initialize}\n").as_bytes())
            .await?;
        wait_for_response(&mut lines, 1).await?;

        let read_limits = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "account/rateLimits/read",
            "params": {}
        });
        stdin
            .write_all(format!("{read_limits}\n").as_bytes())
            .await?;
        let result = wait_for_response(&mut lines, 2).await?;

        let _ = child.start_kill();

        let parsed: RateLimitsResult =
            serde_json::from_value(result).context("parse rateLimits result")?;
        let mut windows = Vec::new();
        if let Some(window) = parsed.rate_limits.primary {
            windows.push(window.into_window(UsageWindow::FiveHour));
        }
        if let Some(window) = parsed.rate_limits.secondary {
            windows.push(window.into_window(UsageWindow::Weekly));
        }
        if windows.is_empty() {
            bail!("app-server returned no rate limit windows");
        }
        Ok(RateLimitReading {
            windows,
            observed_at: Utc::now(),
            source: SnapshotSource::OfficialCli,
        })
    }
}

async fn wait_for_response(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    expected_id: i64,
) -> anyhow::Result<Value> {
    while let Some(line) = lines.next_line().await? {
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if value.get("id").and_then(Value::as_i64) != Some(expected_id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            bail!("app-server error for id {expected_id}: {error}");
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
    bail!("app-server closed stdout before responding to id {expected_id}")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitsResult {
    rate_limits: RateLimitsPayload,
}

#[derive(Debug, Deserialize)]
struct RateLimitsPayload {
    primary: Option<RpcWindow>,
    secondary: Option<RpcWindow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcWindow {
    used_percent: Option<f64>,
    window_duration_mins: Option<i64>,
    resets_at: Option<Value>,
}

impl RpcWindow {
    fn into_window(self, fallback: UsageWindow) -> RateLimitWindow {
        let window = match self.window_duration_mins {
            Some(mins) if mins <= 1440 => UsageWindow::FiveHour,
            Some(_) => UsageWindow::Weekly,
            None => fallback,
        };
        RateLimitWindow {
            window,
            used_percent: self.used_percent,
            resets_at: self.resets_at.as_ref().and_then(parse_resets_at),
        }
    }
}

/// `resetsAt` has been observed as both unix seconds and RFC3339 strings.
pub fn parse_resets_at(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(secs) = value.as_i64() {
        return Utc.timestamp_opt(secs, 0).single();
    }
    value
        .as_str()
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn find_codex_binary() -> Option<String> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("codex")));
    }
    // GUI apps on macOS get a minimal PATH, so probe common install dirs too.
    candidates.push("/opt/homebrew/bin/codex".into());
    candidates.push("/usr/local/bin/codex".into());
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/bin/codex"));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| path.display().to_string())
}
