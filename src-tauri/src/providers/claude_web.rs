use std::time::Duration;

use anyhow::anyhow;
use reqwest::header::COOKIE;
use serde_json::Value;

use crate::core::model::{Agent, SnapshotSource, UsageSnapshot, UsageWindow};

pub async fn fetch_usage() -> anyhow::Result<Vec<UsageSnapshot>> {
    let cookie =
        session_cookie().ok_or_else(|| anyhow!("Claude web session cookie is unavailable"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let organization = client
        .get("https://claude.ai/api/organizations")
        .header(COOKIE, &cookie)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let organization_id = organization_id(&organization)
        .ok_or_else(|| anyhow!("Claude web organization id was not found"))?;
    let usage = client
        .get(format!(
            "https://claude.ai/api/organizations/{organization_id}/usage"
        ))
        .header(COOKIE, cookie)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    parse_usage(&usage).ok_or_else(|| anyhow!("Claude web usage response was not recognized"))
}

fn session_cookie() -> Option<String> {
    if let Some(cookie) = std::env::var_os("CLAUDE_COOKIE") {
        let cookie = cookie.to_string_lossy().trim().to_owned();
        if cookie.contains("sessionKey=") {
            return Some(cookie);
        }
    }
    if let Ok(Some(cookie)) = crate::providers::claude_secrets::read_web_cookie() {
        if cookie.contains("sessionKey=") {
            return Some(cookie);
        }
    }
    let session_key = std::env::var("CLAUDE_SESSION_KEY").ok()?;
    let session_key = session_key.trim();
    (!session_key.is_empty()).then(|| format!("sessionKey={session_key}"))
}

fn organization_id(value: &Value) -> Option<String> {
    match value {
        Value::Array(items) => items.iter().find_map(organization_id),
        Value::Object(object) => {
            for key in ["uuid", "id", "organization_id"] {
                if let Some(id) = object.get(key).and_then(Value::as_str) {
                    return Some(id.to_owned());
                }
            }
            object.values().find_map(organization_id)
        }
        _ => None,
    }
}

fn parse_usage(value: &Value) -> Option<Vec<UsageSnapshot>> {
    let five_hour =
        find_window(value, &["five_hour", "fiveHour", "session"]).and_then(parse_percent);
    let weekly =
        find_window(value, &["seven_day", "sevenDay", "weekly", "week"]).and_then(parse_percent);
    let observed_at = Some(chrono::Utc::now());
    let snapshots = [
        (UsageWindow::FiveHour, five_hour),
        (UsageWindow::Weekly, weekly),
    ]
    .into_iter()
    .filter_map(|(window, utilization_pct)| {
        utilization_pct.map(|utilization_pct| UsageSnapshot {
            agent: Agent::ClaudeCode,
            window,
            utilization_pct: Some(utilization_pct),
            used_tokens: None,
            burn_rate_tokens_per_min: None,
            reset_at: None,
            limit_reached_at: None,
            observed_at,
            source: SnapshotSource::Web,
        })
    })
    .collect::<Vec<_>>();
    (!snapshots.is_empty()).then_some(snapshots)
}

fn find_window<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(value) = object.get(*key) {
                    return Some(value);
                }
            }
            object.values().find_map(|value| find_window(value, keys))
        }
        Value::Array(items) => items.iter().find_map(|value| find_window(value, keys)),
        _ => None,
    }
}

fn parse_percent(value: &Value) -> Option<f64> {
    let value = value
        .get("utilization")
        .or_else(|| value.get("used_percentage"))
        .or_else(|| value.get("used_percent"))
        .or_else(|| value.get("utilization_pct"))
        .or_else(|| value.get("percent"))
        .and_then(Value::as_f64)
        .or_else(|| value.as_f64())?;
    (0.0..=100.0).contains(&value).then_some(value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse_usage;

    #[test]
    fn maps_claude_web_usage_windows() {
        let snapshots = parse_usage(&json!({
            "five_hour": {"utilization": 12.0},
            "seven_day": {"used_percentage": 34.0}
        }))
        .unwrap();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].utilization_pct, Some(12.0));
        assert_eq!(snapshots[1].utilization_pct, Some(34.0));
    }
}
