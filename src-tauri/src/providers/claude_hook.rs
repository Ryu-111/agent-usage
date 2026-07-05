use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{Duration, Utc};
use serde_json::{json, Value};

use crate::core::model::SnapshotSource;
use crate::providers::rate_limits::{unix_seconds, RateLimitReading, RateLimitWindow};

pub const HOOK_SCRIPT: &str = include_str!("../../resources/claude_rate_limits_hook.sh");
pub const HOOK_CACHE_MAX_AGE: Duration = Duration::minutes(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookInstallStatus {
    Installed,
    AlreadyInstalled,
}

pub fn hook_script_path() -> Option<PathBuf> {
    data_dir().map(|path| path.join("claude_rate_limits_hook.sh"))
}

pub fn cache_file_path() -> Option<PathBuf> {
    data_dir().map(|path| path.join("claude_rate_limits.json"))
}

pub fn default_claude_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude/settings.json"))
}

pub fn install_claude_hook(
    settings_path: &Path,
    script_path: &Path,
    cache_dir: &Path,
) -> anyhow::Result<HookInstallStatus> {
    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(script_path, HOOK_SCRIPT)?;
    make_executable(script_path)?;

    let mut settings = match fs::read_to_string(settings_path) {
        Ok(content) => serde_json::from_str::<Value>(&content)
            .with_context(|| format!("failed to parse {}", settings_path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error.into()),
    };

    let command = format!(
        "AGENT_USAGE_CACHE_DIR='{}' '{}'",
        shell_quote_path(cache_dir),
        shell_quote_path(script_path)
    );
    if replace_existing_hook_command(&mut settings, script_path, &command) {
        write_json_atomic(settings_path, &settings)?;
        return Ok(HookInstallStatus::AlreadyInstalled);
    }

    ensure_object(&mut settings);
    let hooks = ensure_child_object(&mut settings, "hooks");
    let stop = ensure_child_array(hooks, "Stop");
    stop.push(json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": 5
        }]
    }));

    write_json_atomic(settings_path, &settings)?;
    Ok(HookInstallStatus::Installed)
}

pub fn is_claude_hook_installed(settings_path: &Path) -> anyhow::Result<bool> {
    let content = match fs::read_to_string(settings_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let settings = serde_json::from_str::<Value>(&content)?;
    Ok(settings
        .pointer("/hooks/Stop")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.pointer("/hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hooks| {
                        hooks.iter().any(|hook| {
                            hook.get("command")
                                .and_then(Value::as_str)
                                .is_some_and(|command| {
                                    command.contains("claude_rate_limits_hook.sh")
                                })
                        })
                    })
            })
        }))
}

pub fn read_hook_cache(path: &Path, max_age: Duration) -> anyhow::Result<Option<RateLimitReading>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let value = serde_json::from_str::<Value>(&content)?;
    let written_at = value
        .get("written_at")
        .and_then(Value::as_i64)
        .and_then(unix_seconds);
    let Some(written_at) = written_at else {
        return Ok(None);
    };
    if Utc::now() - written_at > max_age {
        return Ok(None);
    }

    let limits = value
        .pointer("/hook_payload/rate_limits")
        .or_else(|| value.pointer("/hook_payload/rateLimits"));
    let Some(limits) = limits else {
        return Ok(None);
    };

    Ok(Some(RateLimitReading {
        primary: parse_limit(
            limits
                .get("five_hour")
                .or_else(|| limits.get("fiveHour"))
                .or_else(|| limits.get("primary")),
        ),
        secondary: parse_limit(
            limits
                .get("seven_day")
                .or_else(|| limits.get("sevenDay"))
                .or_else(|| limits.get("weekly"))
                .or_else(|| limits.get("secondary")),
        ),
        observed_at: Some(written_at),
        source: SnapshotSource::HookCache,
    })
    .filter(|reading| reading.primary.is_some() || reading.secondary.is_some()))
}

fn data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|path| path.join("agent-usage"))
}

fn parse_limit(value: Option<&Value>) -> Option<RateLimitWindow> {
    let value = value?;
    Some(RateLimitWindow {
        used_percent: value
            .get("used_percentage")
            .or_else(|| value.get("used_percent"))
            .or_else(|| value.get("usedPercent"))
            .and_then(Value::as_f64),
        window_minutes: value
            .get("window_minutes")
            .or_else(|| value.get("windowMinutes"))
            .and_then(Value::as_i64),
        resets_at: value
            .get("resets_at")
            .or_else(|| value.get("resetsAt"))
            .and_then(Value::as_i64)
            .and_then(unix_seconds),
    })
}

fn replace_existing_hook_command(settings: &mut Value, script_path: &Path, command: &str) -> bool {
    let script_name = script_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("claude_rate_limits_hook.sh");
    let Some(stop) = settings
        .pointer_mut("/hooks/Stop")
        .and_then(Value::as_array_mut)
    else {
        return false;
    };

    let mut replaced = false;
    for item in stop {
        let Some(hooks) = item.pointer_mut("/hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        for hook in hooks {
            let Some(existing) = hook.get("command").and_then(Value::as_str) else {
                continue;
            };
            if existing.contains(script_name) {
                if let Some(object) = hook.as_object_mut() {
                    object.insert("command".to_string(), Value::String(command.to_string()));
                    object.insert("timeout".to_string(), Value::Number(5.into()));
                    replaced = true;
                }
            }
        }
    }
    replaced
}

fn write_json_atomic(path: &Path, value: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(value)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn ensure_object(value: &mut Value) {
    if !value.is_object() {
        *value = json!({});
    }
}

fn ensure_child_object<'a>(value: &'a mut Value, key: &str) -> &'a mut Value {
    let object = value.as_object_mut().expect("value was made an object");
    let entry = object.entry(key).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry
}

fn ensure_child_array<'a>(value: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    let object = value.as_object_mut().expect("value was made an object");
    let entry = object.entry(key).or_insert_with(|| json!([]));
    if !entry.is_array() {
        *entry = json!([]);
    }
    entry.as_array_mut().expect("entry was made an array")
}

fn shell_quote_path(path: &Path) -> String {
    path.display().to_string().replace('\'', r#"'\''"#)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::core::model::SnapshotSource;

    use super::{
        install_claude_hook, is_claude_hook_installed, read_hook_cache, HookInstallStatus,
        HOOK_CACHE_MAX_AGE,
    };

    #[test]
    fn installs_hook_without_removing_existing_hooks() {
        let root =
            std::env::temp_dir().join(format!("agent-usage-claude-hook-{}", std::process::id()));
        let settings = root.join("settings.json");
        let script = root.join("claude_rate_limits_hook.sh");
        let cache = root.join("cache");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &settings,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo existing"}]}]},"other":true}"#,
        )
        .unwrap();

        assert_eq!(
            install_claude_hook(&settings, &script, &cache).unwrap(),
            HookInstallStatus::Installed
        );
        assert!(is_claude_hook_installed(&settings).unwrap());
        assert_eq!(
            install_claude_hook(&settings, &script, &cache).unwrap(),
            HookInstallStatus::AlreadyInstalled
        );

        let updated = fs::read_to_string(&settings).unwrap();
        assert!(updated.contains("echo existing"));
        assert!(updated.contains("claude_rate_limits_hook.sh"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reads_fresh_hook_cache() {
        let root =
            std::env::temp_dir().join(format!("agent-usage-claude-cache-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let cache = root.join("claude_rate_limits.json");
        fs::write(
            &cache,
            format!(
                r#"{{"written_at":{},"hook_payload":{{"rate_limits":{{"five_hour":{{"used_percentage":42.0,"resets_at":1774036800}},"seven_day":{{"used_percentage":64.0,"resets_at":1774580400}}}}}}}}"#,
                chrono::Utc::now().timestamp()
            ),
        )
        .unwrap();

        let reading = read_hook_cache(&cache, HOOK_CACHE_MAX_AGE)
            .unwrap()
            .unwrap();
        assert_eq!(reading.source, SnapshotSource::HookCache);
        assert_eq!(reading.primary.unwrap().used_percent, Some(42.0));

        let _ = fs::remove_dir_all(root);
    }
}
