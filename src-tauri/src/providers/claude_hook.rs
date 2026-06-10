use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{Duration, TimeZone, Utc};
use serde_json::{json, Value};

use crate::core::model::{RateLimitReading, RateLimitWindow, SnapshotSource, UsageWindow};

pub const HOOK_SCRIPT: &str = include_str!("../../resources/claude_rate_limits_hook.sh");
pub const HOOK_SCRIPT_NAME: &str = "claude_rate_limits_hook.sh";
pub const CACHE_FILE_NAME: &str = "claude_rate_limits.json";
pub const HOOK_CACHE_MAX_AGE_MINUTES: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookInstallStatus {
    Installed,
    AlreadyInstalled,
}

fn data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("agent-usage"))
}

pub fn hook_script_path() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join(HOOK_SCRIPT_NAME))
}

pub fn cache_file_path() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join(CACHE_FILE_NAME))
}

pub fn default_claude_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude/settings.json"))
}

/// Write the bundled hook script to `script_path` and register it as a Stop
/// hook in the Claude Code settings file, preserving everything else there.
pub fn install_claude_hook(
    settings_path: &Path,
    script_path: &Path,
    cache_dir: &Path,
) -> anyhow::Result<HookInstallStatus> {
    if let Some(parent) = script_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(script_path, HOOK_SCRIPT)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(script_path, std::fs::Permissions::from_mode(0o755))?;
    }

    let command = format!(
        "AGENT_USAGE_CACHE_DIR='{}' '{}'",
        cache_dir.display(),
        script_path.display()
    );

    let mut settings: Value = match std::fs::read_to_string(settings_path) {
        Ok(content) => serde_json::from_str(&content)
            .with_context(|| format!("parse {}", settings_path.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(err) => return Err(err.into()),
    };
    if !settings.is_object() {
        anyhow::bail!("{} is not a JSON object", settings_path.display());
    }

    let stop_hooks = settings
        .as_object_mut()
        .expect("checked is_object above")
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("settings.hooks is not an object")?
        .entry("Stop")
        .or_insert_with(|| json!([]));
    let stop_entries = stop_hooks
        .as_array_mut()
        .context("settings.hooks.Stop is not an array")?;

    let mut status = HookInstallStatus::Installed;
    let mut replaced = false;
    for entry in stop_entries.iter_mut() {
        let Some(hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        for hook in hooks.iter_mut() {
            let existing = hook.get("command").and_then(Value::as_str).unwrap_or("");
            if existing.contains(HOOK_SCRIPT_NAME) {
                if existing == command {
                    status = HookInstallStatus::AlreadyInstalled;
                } else {
                    hook["command"] = json!(command);
                }
                replaced = true;
            }
        }
    }
    if !replaced {
        stop_entries.push(json!({
            "hooks": [{"type": "command", "command": command}]
        }));
    }
    if status == HookInstallStatus::AlreadyInstalled {
        return Ok(status);
    }

    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = settings_path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&settings)?)?;
    std::fs::rename(&tmp, settings_path)?;
    Ok(status)
}

pub fn is_claude_hook_installed(settings_path: &Path) -> anyhow::Result<bool> {
    let content = match std::fs::read_to_string(settings_path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    Ok(content.contains(HOOK_SCRIPT_NAME))
}

/// Read the cache file the hook script writes:
/// `{"written_at": <unix secs>, "hook_payload": {..., "rate_limits": {...}}}`.
pub fn read_hook_cache(path: &Path, max_age: Duration) -> anyhow::Result<Option<RateLimitReading>> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let value: Value = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let Some(written_at) = value
        .get("written_at")
        .and_then(Value::as_i64)
        .and_then(|secs| Utc.timestamp_opt(secs, 0).single())
    else {
        return Ok(None);
    };
    if Utc::now() - written_at > max_age {
        return Ok(None);
    }
    // Older Claude Code versions do not put rate_limits on hook stdin.
    let Some(rate_limits) = value.pointer("/hook_payload/rate_limits") else {
        return Ok(None);
    };

    let mut windows = Vec::new();
    for (key, window) in [
        ("five_hour", UsageWindow::FiveHour),
        ("seven_day", UsageWindow::Weekly),
    ] {
        let Some(entry) = rate_limits.get(key).filter(|v| !v.is_null()) else {
            continue;
        };
        windows.push(RateLimitWindow {
            window,
            used_percent: entry.get("used_percentage").and_then(Value::as_f64),
            resets_at: entry
                .get("resets_at")
                .and_then(Value::as_i64)
                .and_then(|secs| Utc.timestamp_opt(secs, 0).single()),
        });
    }
    if windows.is_empty() {
        return Ok(None);
    }
    Ok(Some(RateLimitReading {
        windows,
        observed_at: written_at,
        source: SnapshotSource::HookCache,
    }))
}
