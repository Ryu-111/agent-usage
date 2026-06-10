#![cfg(unix)]

use std::time::Duration;

use agent_usage::core::model::{SnapshotSource, UsageWindow};
use agent_usage::providers::codex_app_server::AppServerClient;

fn fake_client(mode: &str) -> AppServerClient {
    let script = format!(
        "{}/tests/fixtures/fake_codex_app_server.sh",
        env!("CARGO_MANIFEST_DIR")
    );
    AppServerClient::new(vec![
        "env".into(),
        format!("FAKE_MODE={mode}"),
        "sh".into(),
        script,
    ])
}

#[tokio::test]
async fn fetches_rate_limits_from_app_server() {
    let reading = fake_client("ok")
        .fetch_rate_limits()
        .await
        .expect("fetch ok");

    assert_eq!(reading.source, SnapshotSource::OfficialCli);
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

#[tokio::test]
async fn surfaces_json_rpc_errors() {
    let err = fake_client("error")
        .fetch_rate_limits()
        .await
        .expect_err("should fail");
    assert!(err.to_string().contains("error"), "got: {err:#}");
}

#[tokio::test]
async fn times_out_when_server_hangs() {
    let mut client = fake_client("hang");
    client.timeout = Duration::from_secs(1);
    let err = client.fetch_rate_limits().await.expect_err("should fail");
    assert!(err.to_string().contains("timed out"), "got: {err:#}");
}
