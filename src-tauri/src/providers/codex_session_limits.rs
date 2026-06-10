use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::Value;

use crate::core::model::{RateLimitReading, RateLimitWindow, SnapshotSource, UsageWindow};

pub const SESSION_LIMITS_MAX_AGE_HOURS: i64 = 24;
const MAX_DAY_DIRS: usize = 7;
const MAX_FILES: usize = 30;

/// Scan `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` newest-first for the
/// most recent non-null `rate_limits` payload from a `token_count` event.
pub fn read_latest_rate_limits(sessions_root: &Path) -> anyhow::Result<Option<RateLimitReading>> {
    let mut scanned_files = 0usize;
    for day_dir in day_dirs_newest_first(sessions_root)?.into_iter().take(MAX_DAY_DIRS) {
        for file in rollout_files_newest_first(&day_dir)? {
            if scanned_files >= MAX_FILES {
                return Ok(None);
            }
            scanned_files += 1;
            let content = match std::fs::read_to_string(&file) {
                Ok(content) => content,
                Err(_) => continue,
            };
            if let Some(reading) = content.lines().rev().find_map(parse_rate_limits_line) {
                if Utc::now() - reading.observed_at
                    > Duration::hours(SESSION_LIMITS_MAX_AGE_HOURS)
                {
                    return Ok(None);
                }
                return Ok(Some(reading));
            }
        }
    }
    Ok(None)
}

pub fn parse_rate_limits_line(line: &str) -> Option<RateLimitReading> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let rate_limits = payload.get("rate_limits")?;
    if rate_limits.is_null() {
        return None;
    }
    let observed_at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|parsed| parsed.with_timezone(&Utc))?;

    let mut windows = Vec::new();
    for (key, fallback) in [
        ("primary", UsageWindow::FiveHour),
        ("secondary", UsageWindow::Weekly),
    ] {
        let Some(window_value) = rate_limits.get(key).filter(|v| !v.is_null()) else {
            continue;
        };
        windows.push(parse_window(window_value, fallback, observed_at));
    }
    if windows.is_empty() {
        return None;
    }
    Some(RateLimitReading {
        windows,
        observed_at,
        source: SnapshotSource::SessionLog,
    })
}

fn parse_window(
    value: &Value,
    fallback: UsageWindow,
    observed_at: DateTime<Utc>,
) -> RateLimitWindow {
    let window = match value.get("window_minutes").and_then(Value::as_i64) {
        Some(mins) if mins <= 1440 => UsageWindow::FiveHour,
        Some(_) => UsageWindow::Weekly,
        None => fallback,
    };
    let resets_at = value
        .get("resets_at")
        .and_then(Value::as_i64)
        .and_then(|secs| Utc.timestamp_opt(secs, 0).single())
        .or_else(|| {
            value
                .get("resets_in_seconds")
                .and_then(Value::as_i64)
                .map(|secs| observed_at + Duration::seconds(secs))
        });
    RateLimitWindow {
        window,
        used_percent: value.get("used_percent").and_then(Value::as_f64),
        resets_at,
    }
}

/// Year/month/day directories sorted newest first by their numeric names.
fn day_dirs_newest_first(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut days = Vec::new();
    for year in numeric_dirs_desc(root) {
        for month in numeric_dirs_desc(&year) {
            for day in numeric_dirs_desc(&month) {
                days.push(day);
            }
        }
    }
    Ok(days)
}

fn numeric_dirs_desc(parent: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut dirs: Vec<(u32, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter_map(|path| {
            let number = path.file_name()?.to_str()?.parse::<u32>().ok()?;
            Some((number, path))
        })
        .collect();
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    dirs.into_iter().map(|(_, path)| path).collect()
}

fn rollout_files_newest_first(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        })
        .map(|path| {
            let mtime = path
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            (mtime, path)
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(files.into_iter().map(|(_, path)| path).collect())
}
