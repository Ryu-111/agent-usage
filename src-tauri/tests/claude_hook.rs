use std::fs;
use std::path::Path;

use agent_usage::core::model::{SnapshotSource, UsageWindow};
use agent_usage::providers::claude_hook::{
    install_claude_hook, is_claude_hook_installed, read_hook_cache, HookInstallStatus,
};
use chrono::Duration;

fn fixture_path(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn reads_fresh_hook_cache() {
    // written_at in the fixture is far in the future, so it is always fresh.
    let reading = read_hook_cache(&fixture_path("claude_hook_cache.json"), Duration::minutes(30))
        .expect("read ok")
        .expect("reading found");

    assert_eq!(reading.source, SnapshotSource::HookCache);
    let five_hour = reading
        .windows
        .iter()
        .find(|window| window.window == UsageWindow::FiveHour)
        .unwrap();
    assert_eq!(five_hour.used_percent, Some(42.3));
    assert_eq!(
        five_hour.resets_at.map(|value| value.timestamp()),
        Some(4070912400)
    );
    let weekly = reading
        .windows
        .iter()
        .find(|window| window.window == UsageWindow::Weekly)
        .unwrap();
    assert_eq!(weekly.used_percent, Some(85.7));
}

#[test]
fn rejects_stale_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let stale = fs::read_to_string(fixture_path("claude_hook_cache.json"))
        .unwrap()
        .replace("4070908800", "1500000000");
    fs::write(&path, stale).unwrap();

    assert!(read_hook_cache(&path, Duration::minutes(30))
        .unwrap()
        .is_none());
}

#[test]
fn returns_none_when_rate_limits_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    fs::write(&path, r#"{"written_at":4070908800,"hook_payload":{"session_id":"x"}}"#).unwrap();

    assert!(read_hook_cache(&path, Duration::minutes(30))
        .unwrap()
        .is_none());
}

#[test]
fn returns_none_when_cache_file_missing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        read_hook_cache(&dir.path().join("missing.json"), Duration::minutes(30))
            .unwrap()
            .is_none()
    );
}

#[test]
fn installs_hook_into_fresh_settings_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    let script = dir.path().join("claude_rate_limits_hook.sh");
    let cache_dir = dir.path().join("cache");

    let status = install_claude_hook(&settings, &script, &cache_dir).expect("install");
    assert_eq!(status, HookInstallStatus::Installed);
    assert!(script.is_file());
    assert!(is_claude_hook_installed(&settings).unwrap());

    let status = install_claude_hook(&settings, &script, &cache_dir).expect("reinstall");
    assert_eq!(status, HookInstallStatus::AlreadyInstalled);

    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    let stop = value.pointer("/hooks/Stop").unwrap().as_array().unwrap();
    assert_eq!(stop.len(), 1, "re-install must not duplicate the entry");
}

#[test]
fn preserves_existing_settings_keys_and_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    fs::write(
        &settings,
        r#"{
            "model": "opus",
            "hooks": {
                "Stop": [{"hooks": [{"type": "command", "command": "echo done"}]}],
                "PostToolUse": [{"hooks": [{"type": "command", "command": "lint"}]}]
            }
        }"#,
    )
    .unwrap();
    let script = dir.path().join("claude_rate_limits_hook.sh");
    let cache_dir = dir.path().join("cache");

    install_claude_hook(&settings, &script, &cache_dir).expect("install");

    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(value.pointer("/model").unwrap(), "opus");
    assert_eq!(
        value
            .pointer("/hooks/PostToolUse/0/hooks/0/command")
            .unwrap(),
        "lint"
    );
    let stop = value.pointer("/hooks/Stop").unwrap().as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert_eq!(
        stop[0].pointer("/hooks/0/command").unwrap(),
        "echo done",
        "existing Stop hook must survive"
    );
}
