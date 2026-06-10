use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Agent {
    ClaudeCode,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageWindow {
    FiveHour,
    Weekly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotSource {
    Official,
    OfficialCli,
    SessionLog,
    HookCache,
    LocalEstimate,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub agent: Agent,
    pub window: UsageWindow,
    pub utilization_pct: Option<f64>,
    pub used_tokens: Option<u64>,
    pub burn_rate_tokens_per_min: Option<f64>,
    pub reset_at: Option<DateTime<Utc>>,
    pub limit_reached_at: Option<DateTime<Utc>>,
    pub source: SnapshotSource,
    pub observed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    pub agent: Agent,
    pub windows: Vec<UsageSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub captured_at: DateTime<Utc>,
    pub agents: Vec<AgentUsage>,
}

#[derive(Debug, Clone)]
pub struct TokenEvent {
    pub timestamp: DateTime<Utc>,
    pub tokens: u64,
}

#[derive(Debug, Clone)]
pub struct RateLimitWindow {
    pub window: UsageWindow,
    pub used_percent: Option<f64>,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct RateLimitReading {
    pub windows: Vec<RateLimitWindow>,
    pub observed_at: DateTime<Utc>,
    pub source: SnapshotSource,
}

pub fn rate_limit_snapshots(agent: Agent, reading: &RateLimitReading) -> Vec<UsageSnapshot> {
    reading
        .windows
        .iter()
        .map(|window| UsageSnapshot {
            agent,
            window: window.window,
            utilization_pct: window.used_percent,
            used_tokens: None,
            burn_rate_tokens_per_min: None,
            reset_at: window.resets_at,
            limit_reached_at: None,
            source: reading.source,
            observed_at: Some(reading.observed_at),
        })
        .collect()
}

/// Overlay local token counts and burn rates onto rate-limit snapshots.
/// Windows that only exist locally are appended so they are not dropped.
pub fn merge_with_local(
    primary: Vec<UsageSnapshot>,
    local: Vec<UsageSnapshot>,
) -> Vec<UsageSnapshot> {
    let mut merged: Vec<UsageSnapshot> = primary
        .into_iter()
        .map(|mut primary_window| {
            if let Some(local_window) = local
                .iter()
                .find(|candidate| candidate.window == primary_window.window)
            {
                primary_window.used_tokens = local_window.used_tokens;
                primary_window.burn_rate_tokens_per_min = local_window.burn_rate_tokens_per_min;
            }
            primary_window
        })
        .collect();
    for local_window in local {
        if !merged
            .iter()
            .any(|candidate| candidate.window == local_window.window)
        {
            merged.push(local_window);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{
        merge_with_local, rate_limit_snapshots, Agent, RateLimitReading, RateLimitWindow,
        SnapshotSource, UsageSnapshot, UsageWindow,
    };

    fn snapshot(window: UsageWindow, source: SnapshotSource) -> UsageSnapshot {
        UsageSnapshot {
            agent: Agent::Codex,
            window,
            utilization_pct: None,
            used_tokens: None,
            burn_rate_tokens_per_min: None,
            reset_at: None,
            limit_reached_at: None,
            source,
            observed_at: None,
        }
    }

    #[test]
    fn rate_limit_snapshots_maps_windows_and_metadata() {
        let observed_at = Utc::now();
        let reading = RateLimitReading {
            windows: vec![RateLimitWindow {
                window: UsageWindow::FiveHour,
                used_percent: Some(12.5),
                resets_at: Some(observed_at),
            }],
            observed_at,
            source: SnapshotSource::OfficialCli,
        };

        let snapshots = rate_limit_snapshots(Agent::Codex, &reading);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].agent, Agent::Codex);
        assert_eq!(snapshots[0].window, UsageWindow::FiveHour);
        assert_eq!(snapshots[0].utilization_pct, Some(12.5));
        assert_eq!(snapshots[0].reset_at, Some(observed_at));
        assert_eq!(snapshots[0].source, SnapshotSource::OfficialCli);
        assert_eq!(snapshots[0].observed_at, Some(observed_at));
    }

    #[test]
    fn merge_copies_local_tokens_and_appends_missing_windows() {
        let primary = vec![snapshot(UsageWindow::FiveHour, SnapshotSource::OfficialCli)];
        let mut five_hour_local = snapshot(UsageWindow::FiveHour, SnapshotSource::LocalEstimate);
        five_hour_local.used_tokens = Some(1000);
        five_hour_local.burn_rate_tokens_per_min = Some(5.0);
        let mut weekly_local = snapshot(UsageWindow::Weekly, SnapshotSource::LocalEstimate);
        weekly_local.used_tokens = Some(9000);

        let merged = merge_with_local(primary, vec![five_hour_local, weekly_local]);

        assert_eq!(merged.len(), 2);
        let five_hour = merged
            .iter()
            .find(|window| window.window == UsageWindow::FiveHour)
            .unwrap();
        assert_eq!(five_hour.source, SnapshotSource::OfficialCli);
        assert_eq!(five_hour.used_tokens, Some(1000));
        assert_eq!(five_hour.burn_rate_tokens_per_min, Some(5.0));
        let weekly = merged
            .iter()
            .find(|window| window.window == UsageWindow::Weekly)
            .unwrap();
        assert_eq!(weekly.source, SnapshotSource::LocalEstimate);
        assert_eq!(weekly.used_tokens, Some(9000));
    }
}
