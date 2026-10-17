use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::core::model::SnapshotSource;
use crate::providers::jsonl::extract_timestamp;
use crate::providers::rate_limits::{unix_seconds, RateLimitReading, RateLimitWindow};

pub const SESSION_LIMITS_MAX_AGE: Duration = Duration::hours(24);

pub fn read_latest_rate_limits(sessions_root: &Path) -> anyhow::Result<Option<RateLimitReading>> {
    if !sessions_root.exists() {
        return Ok(None);
    }

    let mut files = Vec::new();
    collect_jsonl_files(sessions_root, &mut files)?;
    files.sort_by_key(|file| std::cmp::Reverse(file.modified));

    for file in files.into_iter().take(30) {
        let content = fs::read_to_string(&file.path)
            .with_context(|| format!("failed to read {}", file.path.display()))?;
        for line in content.lines().rev() {
            if let Some(reading) = parse_rate_limits_line(line) {
                if reading
                    .observed_at
                    .is_some_and(|observed_at| Utc::now() - observed_at <= SESSION_LIMITS_MAX_AGE)
                {
                    return Ok(Some(reading));
                }
                return Ok(None);
            }
        }
    }

    Ok(None)
}

pub fn parse_rate_limits_line(line: &str) -> Option<RateLimitReading> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }

    let limits = value.pointer("/payload/rate_limits")?;
    if limits.is_null() {
        return None;
    }

    let observed_at = extract_timestamp(&value);
    Some(RateLimitReading {
        primary: parse_limit(limits.get("primary"), observed_at),
        secondary: parse_limit(limits.get("secondary"), observed_at),
        observed_at,
        source: SnapshotSource::SessionLog,
    })
    .filter(|reading| reading.primary.is_some() || reading.secondary.is_some())
}

fn parse_limit(
    value: Option<&Value>,
    observed_at: Option<DateTime<Utc>>,
) -> Option<RateLimitWindow> {
    let value = value?;
    Some(RateLimitWindow {
        used_percent: value
            .get("used_percent")
            .or_else(|| value.get("usedPercent"))
            .or_else(|| value.get("used_percentage"))
            .and_then(Value::as_f64),
        window_minutes: value
            .get("window_minutes")
            .or_else(|| value.get("windowMinutes"))
            .or_else(|| value.get("windowDurationMins"))
            .and_then(Value::as_i64),
        resets_at: value
            .get("resets_at")
            .or_else(|| value.get("resetsAt"))
            .and_then(Value::as_i64)
            .and_then(unix_seconds)
            .or_else(|| {
                let observed_at = observed_at?;
                value
                    .get("resets_in_seconds")
                    .or_else(|| value.get("resetsInSeconds"))
                    .and_then(Value::as_i64)
                    .map(|seconds| observed_at + Duration::seconds(seconds))
            }),
    })
}

#[derive(Debug)]
struct SessionFile {
    path: PathBuf,
    modified: SystemTime,
}

fn collect_jsonl_files(root: &Path, files: &mut Vec<SessionFile>) -> anyhow::Result<()> {
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            files.push(SessionFile {
                modified: entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH),
                path,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::core::model::SnapshotSource;

    use super::parse_rate_limits_line;

    #[test]
    fn parses_token_count_rate_limits_line() {
        let parsed = parse_rate_limits_line(
            r#"{"timestamp":"2026-06-10T09:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":{"primary":{"used_percent":12.5,"window_minutes":300,"resets_in_seconds":60},"secondary":{"used_percent":40.0,"window_minutes":10080,"resets_at":1774580400}}}}"#,
        )
        .unwrap();

        assert_eq!(parsed.source, SnapshotSource::SessionLog);
        assert_eq!(parsed.primary.as_ref().unwrap().used_percent, Some(12.5));
        assert_eq!(
            parsed.primary.unwrap().resets_at,
            Some(Utc.with_ymd_and_hms(2026, 6, 10, 9, 1, 0).unwrap())
        );
    }

    #[test]
    fn ignores_exec_lines_without_rate_limits() {
        assert!(parse_rate_limits_line(
            r#"{"timestamp":"2026-06-10T09:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":null}}"#
        )
        .is_none());
    }
}
