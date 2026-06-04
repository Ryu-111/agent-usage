use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;
use serde::Deserialize;

use crate::core::model::{Agent, SnapshotSource, TokenEvent, UsageSnapshot, UsageWindow};
use crate::providers::creds::read_claude_credentials;
use crate::providers::jsonl::read_token_events;

#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    projects_pattern: Option<String>,
    client: reqwest::Client,
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        let projects_pattern =
            dirs::home_dir().map(|home| format!("{}/.claude/projects/**/*.jsonl", home.display()));
        Self {
            projects_pattern,
            client: reqwest::Client::new(),
        }
    }
}

impl ClaudeProvider {
    pub fn with_projects_pattern(pattern: String) -> Self {
        Self {
            projects_pattern: Some(pattern),
            client: reqwest::Client::new(),
        }
    }

    pub async fn snapshot(&self) -> anyhow::Result<Vec<UsageSnapshot>> {
        let local = self.local_snapshot()?;
        match self.official_snapshot().await {
            Ok(Some(official)) => Ok(merge_official_with_local(official, local)),
            _ => Ok(local),
        }
    }

    fn local_snapshot(&self) -> anyhow::Result<Vec<UsageSnapshot>> {
        let events = match &self.projects_pattern {
            Some(pattern) => read_token_events(pattern)?,
            None => Vec::new(),
        };
        Ok(local_windows(Agent::ClaudeCode, &events))
    }

    async fn official_snapshot(&self) -> anyhow::Result<Option<Vec<UsageSnapshot>>> {
        let Some(creds) = read_claude_credentials()? else {
            return Ok(None);
        };
        if creds.is_expired() {
            return Ok(None);
        }

        let response = self
            .client
            .get("https://api.anthropic.com/api/oauth/usage")
            .bearer_auth(creds.access_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("content-type", "application/json")
            .header("user-agent", "claude-code/1.0.0")
            .send()
            .await?;

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Ok(None);
        }
        let usage: ClaudeUsageResponse = response.error_for_status()?.json().await?;
        Ok(Some(usage.into_snapshots()))
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeUsageResponse {
    #[serde(default, rename = "anthropic-ratelimit-unified-5h-utilization")]
    five_hour_utilization: Option<f64>,
    #[serde(default, rename = "anthropic-ratelimit-unified-7d-utilization")]
    weekly_utilization: Option<f64>,
    #[serde(default, rename = "anthropic-ratelimit-unified-5h-reset")]
    five_hour_reset: Option<DateTime<Utc>>,
    #[serde(default, rename = "anthropic-ratelimit-unified-7d-reset")]
    weekly_reset: Option<DateTime<Utc>>,
}

impl ClaudeUsageResponse {
    fn into_snapshots(self) -> Vec<UsageSnapshot> {
        vec![
            UsageSnapshot {
                agent: Agent::ClaudeCode,
                window: UsageWindow::FiveHour,
                utilization_pct: self.five_hour_utilization,
                used_tokens: None,
                burn_rate_tokens_per_min: None,
                reset_at: self.five_hour_reset,
                limit_reached_at: None,
                source: SnapshotSource::Official,
            },
            UsageSnapshot {
                agent: Agent::ClaudeCode,
                window: UsageWindow::Weekly,
                utilization_pct: self.weekly_utilization,
                used_tokens: None,
                burn_rate_tokens_per_min: None,
                reset_at: self.weekly_reset,
                limit_reached_at: None,
                source: SnapshotSource::Official,
            },
        ]
    }
}

pub fn local_windows(agent: Agent, events: &[TokenEvent]) -> Vec<UsageSnapshot> {
    let now = Utc::now();
    vec![
        local_window(
            agent,
            UsageWindow::FiveHour,
            events,
            now,
            Duration::hours(5),
        ),
        local_window(agent, UsageWindow::Weekly, events, now, Duration::days(7)),
    ]
}

fn local_window(
    agent: Agent,
    window: UsageWindow,
    events: &[TokenEvent],
    now: DateTime<Utc>,
    duration: Duration,
) -> UsageSnapshot {
    let start = now - duration;
    let counted_events: Vec<&TokenEvent> = events
        .iter()
        .filter(|event| event.timestamp >= start && event.timestamp <= now)
        .collect();
    let used_tokens: u64 = counted_events.iter().map(|event| event.tokens).sum();
    let minutes = duration.num_minutes().max(1) as f64;
    let reset_at = counted_events
        .iter()
        .map(|event| event.timestamp + duration)
        .min();

    UsageSnapshot {
        agent,
        window,
        utilization_pct: None,
        used_tokens: Some(used_tokens),
        burn_rate_tokens_per_min: Some(used_tokens as f64 / minutes),
        reset_at,
        limit_reached_at: None,
        source: if used_tokens > 0 {
            SnapshotSource::LocalEstimate
        } else {
            SnapshotSource::Unavailable
        },
    }
}

fn merge_official_with_local(
    official: Vec<UsageSnapshot>,
    local: Vec<UsageSnapshot>,
) -> Vec<UsageSnapshot> {
    official
        .into_iter()
        .map(|mut official_window| {
            if let Some(local_window) = local
                .iter()
                .find(|candidate| candidate.window == official_window.window)
            {
                official_window.used_tokens = local_window.used_tokens;
                official_window.burn_rate_tokens_per_min = local_window.burn_rate_tokens_per_min;
            }
            official_window
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use crate::core::model::{Agent, TokenEvent, UsageWindow};

    use super::{local_window, local_windows};

    #[test]
    fn local_windows_only_count_events_inside_window() {
        let now = chrono::Utc::now();
        let events = vec![
            TokenEvent {
                timestamp: now - Duration::hours(1),
                tokens: 100,
            },
            TokenEvent {
                timestamp: now - Duration::hours(6),
                tokens: 900,
            },
        ];

        let windows = local_windows(Agent::ClaudeCode, &events);
        let five_hour = windows
            .iter()
            .find(|window| window.window == UsageWindow::FiveHour)
            .unwrap();

        assert_eq!(five_hour.used_tokens, Some(100));
    }

    #[test]
    fn local_window_reset_is_when_oldest_counted_event_ages_out() {
        let now = chrono::Utc::now();
        let oldest_counted = now - Duration::hours(4);
        let events = vec![
            TokenEvent {
                timestamp: oldest_counted,
                tokens: 100,
            },
            TokenEvent {
                timestamp: now - Duration::hours(1),
                tokens: 50,
            },
            TokenEvent {
                timestamp: now - Duration::hours(6),
                tokens: 900,
            },
        ];

        let snapshot = local_window(
            Agent::ClaudeCode,
            UsageWindow::FiveHour,
            &events,
            now,
            Duration::hours(5),
        );

        assert_eq!(snapshot.reset_at, Some(oldest_counted + Duration::hours(5)));
    }
}
