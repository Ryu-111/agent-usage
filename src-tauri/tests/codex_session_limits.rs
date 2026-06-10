use std::fs;
use std::path::Path;

use agent_usage::core::model::{SnapshotSource, UsageWindow};
use agent_usage::providers::codex_session_limits::{
    parse_rate_limits_line, read_latest_rate_limits,
};

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read_to_string(path).expect("read fixture")
}

#[test]
fn parses_new_format_with_resets_at() {
    let content = fixture("codex_rollout_rate_limits.jsonl");
    let line = content
        .lines()
        .find(|line| line.contains("\"used_percent\":12.5"))
        .unwrap();

    let reading = parse_rate_limits_line(line).expect("should parse");
    assert_eq!(reading.source, SnapshotSource::SessionLog);
    assert_eq!(reading.windows.len(), 2);

    let five_hour = reading
        .windows
        .iter()
        .find(|window| window.window == UsageWindow::FiveHour)
        .unwrap();
    assert_eq!(five_hour.used_percent, Some(12.5));
    assert_eq!(
        five_hour.resets_at.map(|value| value.timestamp()),
        Some(4070912400)
    );

    let weekly = reading
        .windows
        .iter()
        .find(|window| window.window == UsageWindow::Weekly)
        .unwrap();
    assert_eq!(weekly.used_percent, Some(40.0));
}

#[test]
fn parses_legacy_format_with_resets_in_seconds() {
    let content = fixture("codex_rollout_rate_limits.jsonl");
    let line = content
        .lines()
        .find(|line| line.contains("resets_in_seconds"))
        .unwrap();

    let reading = parse_rate_limits_line(line).expect("should parse");
    let five_hour = reading
        .windows
        .iter()
        .find(|window| window.window == UsageWindow::FiveHour)
        .unwrap();
    assert_eq!(
        five_hour.resets_at,
        Some(reading.observed_at + chrono::Duration::seconds(3600))
    );
}

#[test]
fn skips_null_rate_limits_and_non_token_count_lines() {
    let content = fixture("codex_rollout_rate_limits.jsonl");
    let null_line = content
        .lines()
        .find(|line| line.contains("\"rate_limits\":null"))
        .unwrap();
    let other_line = content
        .lines()
        .find(|line| line.contains("agent_message"))
        .unwrap();

    assert!(parse_rate_limits_line(null_line).is_none());
    assert!(parse_rate_limits_line(other_line).is_none());
}

#[test]
fn reads_latest_reading_from_newest_day_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let old_day = dir.path().join("2099/01/01");
    let new_day = dir.path().join("2099/01/02");
    fs::create_dir_all(&old_day).unwrap();
    fs::create_dir_all(&new_day).unwrap();
    fs::write(
        old_day.join("rollout-old.jsonl"),
        fixture("codex_rollout_rate_limits.jsonl"),
    )
    .unwrap();
    // Newest day only has null rate_limits, so the scan must fall back to the
    // previous day's file.
    fs::write(
        new_day.join("rollout-new.jsonl"),
        fixture("codex_rollout_null_limits.jsonl"),
    )
    .unwrap();

    let reading = read_latest_rate_limits(dir.path())
        .expect("scan ok")
        .expect("reading found");
    let five_hour = reading
        .windows
        .iter()
        .find(|window| window.window == UsageWindow::FiveHour)
        .unwrap();
    assert_eq!(five_hour.used_percent, Some(12.5));
}

#[test]
fn returns_none_when_only_null_limits_exist() {
    let dir = tempfile::tempdir().expect("tempdir");
    let day = dir.path().join("2099/01/02");
    fs::create_dir_all(&day).unwrap();
    fs::write(
        day.join("rollout-new.jsonl"),
        fixture("codex_rollout_null_limits.jsonl"),
    )
    .unwrap();

    assert!(read_latest_rate_limits(dir.path()).unwrap().is_none());
}

#[test]
fn discards_readings_older_than_a_day() {
    let dir = tempfile::tempdir().expect("tempdir");
    let day = dir.path().join("2020/01/01");
    fs::create_dir_all(&day).unwrap();
    let stale = fixture("codex_rollout_rate_limits.jsonl").replace("2099-", "2020-");
    fs::write(day.join("rollout-stale.jsonl"), stale).unwrap();

    assert!(read_latest_rate_limits(dir.path()).unwrap().is_none());
}
