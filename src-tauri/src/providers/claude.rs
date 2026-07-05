use std::sync::OnceLock;

use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::core::model::{Agent, SnapshotSource, TokenEvent, UsageSnapshot, UsageWindow};
use crate::providers::claude_hook::{cache_file_path, read_hook_cache, HOOK_CACHE_MAX_AGE};
use crate::providers::creds::read_claude_credentials;
use crate::providers::jsonl::read_recent_token_events;
use crate::providers::rate_limits::merge_rate_limits_with_local;

static OFFICIAL_USAGE_STATE: OnceLock<Mutex<OfficialUsageState>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    projects_pattern: Option<String>,
    desktop_patterns: Vec<String>,
    hook_cache_path: Option<std::path::PathBuf>,
    client: reqwest::Client,
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        let projects_pattern =
            dirs::home_dir().map(|home| format!("{}/.claude/projects/**/*.jsonl", home.display()));
        Self {
            projects_pattern,
            desktop_patterns: default_desktop_patterns(),
            hook_cache_path: cache_file_path(),
            client: reqwest::Client::new(),
        }
    }
}

impl ClaudeProvider {
    pub fn with_projects_pattern(pattern: String) -> Self {
        Self {
            projects_pattern: Some(pattern),
            desktop_patterns: Vec::new(),
            hook_cache_path: cache_file_path(),
            client: reqwest::Client::new(),
        }
    }

    #[cfg(test)]
    pub fn with_hook_cache_path(mut self, hook_cache_path: std::path::PathBuf) -> Self {
        self.hook_cache_path = Some(hook_cache_path);
        self
    }

    pub async fn snapshot(&self) -> anyhow::Result<Vec<UsageSnapshot>> {
        let local = self.local_snapshot()?;

        if let Some(hook) = self.hook_snapshot()? {
            return Ok(merge_rate_limits_with_local(hook, local));
        }

        if let Ok(Some(official)) = self.official_snapshot().await {
            return Ok(merge_rate_limits_with_local(official, local));
        }

        Ok(local)
    }

    fn local_snapshot(&self) -> anyhow::Result<Vec<UsageSnapshot>> {
        let mut events = Vec::new();
        if let Some(pattern) = &self.projects_pattern {
            events.extend(read_recent_token_events(pattern, Duration::days(8))?);
        }
        for pattern in &self.desktop_patterns {
            events.extend(read_recent_token_events(pattern, Duration::days(8))?);
        }
        events.sort_by_key(|event| event.timestamp);
        Ok(local_windows(Agent::ClaudeCode, &events))
    }

    fn hook_snapshot(&self) -> anyhow::Result<Option<Vec<UsageSnapshot>>> {
        let Some(path) = &self.hook_cache_path else {
            return Ok(None);
        };
        Ok(read_hook_cache(path, HOOK_CACHE_MAX_AGE)?
            .map(|reading| reading.into_snapshots(Agent::ClaudeCode)))
    }

    async fn official_snapshot(&self) -> anyhow::Result<Option<Vec<UsageSnapshot>>> {
        let Some(creds) = read_claude_credentials()? else {
            return Ok(None);
        };
        if creds.is_expired() {
            return Ok(None);
        }

        let now = Utc::now();
        let state_lock =
            OFFICIAL_USAGE_STATE.get_or_init(|| Mutex::new(OfficialUsageState::default()));
        let mut state = state_lock.lock().await;
        if state.should_skip_request(now) {
            return Ok(state.cached_snapshots());
        }
        if state.has_fresh_cache(now) {
            return Ok(state.cached_snapshots());
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
            state.register_rate_limited(now);
            return Ok(state.cached_snapshots());
        }
        let usage: ClaudeUsageResponse = response.error_for_status()?.json().await?;
        let snapshots = usage.into_snapshots();
        state.register_success(snapshots.clone(), now);
        Ok(Some(snapshots))
    }
}

fn default_desktop_patterns() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let base = home.join("Library/Application Support/Claude");
    vec![
        format!("{}/claude-code-sessions/**/local_*.json", base.display()),
        format!(
            "{}/local-agent-mode-sessions/**/local_*.json",
            base.display()
        ),
    ]
}

#[derive(Debug, Clone)]
struct CachedOfficialUsage {
    snapshots: Vec<UsageSnapshot>,
    fetched_at: DateTime<Utc>,
}

#[derive(Debug)]
struct OfficialUsageState {
    cached: Option<CachedOfficialUsage>,
    backoff: Duration,
    retry_after: Option<DateTime<Utc>>,
}

impl Default for OfficialUsageState {
    fn default() -> Self {
        Self {
            cached: None,
            backoff: initial_backoff(),
            retry_after: None,
        }
    }
}

impl OfficialUsageState {
    fn cached_snapshots(&self) -> Option<Vec<UsageSnapshot>> {
        self.cached.as_ref().map(|cached| cached.snapshots.clone())
    }

    fn has_fresh_cache(&self, now: DateTime<Utc>) -> bool {
        self.cached
            .as_ref()
            .is_some_and(|cached| cached.fetched_at + cache_ttl() > now)
    }

    fn should_skip_request(&self, now: DateTime<Utc>) -> bool {
        self.retry_after
            .is_some_and(|retry_after| retry_after > now)
    }

    fn register_success(&mut self, snapshots: Vec<UsageSnapshot>, now: DateTime<Utc>) {
        self.cached = Some(CachedOfficialUsage {
            snapshots,
            fetched_at: now,
        });
        self.backoff = initial_backoff();
        self.retry_after = None;
    }

    fn register_rate_limited(&mut self, now: DateTime<Utc>) {
        self.retry_after = Some(now + self.backoff);
        self.backoff = std::cmp::min(self.backoff * 2, max_backoff());
    }
}

fn cache_ttl() -> Duration {
    Duration::minutes(5)
}

fn initial_backoff() -> Duration {
    Duration::minutes(5)
}

fn max_backoff() -> Duration {
    Duration::hours(1)
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
                observed_at: Some(Utc::now()),
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
                observed_at: Some(Utc::now()),
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
    let has_usage = used_tokens > 0;
    let minutes = duration.num_minutes().max(1) as f64;
    let reset_at = counted_events
        .iter()
        .map(|event| event.timestamp + duration)
        .min();

    UsageSnapshot {
        agent,
        window,
        utilization_pct: None,
        used_tokens: has_usage.then_some(used_tokens),
        burn_rate_tokens_per_min: has_usage.then_some(used_tokens as f64 / minutes),
        reset_at,
        limit_reached_at: None,
        observed_at: Some(now),
        source: if has_usage {
            SnapshotSource::LocalEstimate
        } else {
            SnapshotSource::Unavailable
        },
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use crate::core::model::{Agent, SnapshotSource, TokenEvent, UsageSnapshot, UsageWindow};

    use super::{local_window, local_windows, ClaudeUsageResponse, OfficialUsageState};

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

    #[test]
    fn official_usage_state_reuses_fresh_cache() {
        let now = chrono::Utc::now();
        let mut state = OfficialUsageState::default();
        state.register_success(vec![official_window()], now);

        assert!(state.has_fresh_cache(now + Duration::minutes(4)));
        assert_eq!(state.cached_snapshots().unwrap().len(), 1);
    }

    #[test]
    fn claude_usage_response_maps_unified_utilization() {
        let snapshots = ClaudeUsageResponse {
            five_hour_utilization: Some(25.0),
            weekly_utilization: Some(40.0),
            five_hour_reset: None,
            weekly_reset: None,
        }
        .into_snapshots();

        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].window, UsageWindow::FiveHour);
        assert_eq!(snapshots[0].utilization_pct, Some(25.0));
        assert_eq!(snapshots[0].source, SnapshotSource::Official);
        assert_eq!(snapshots[1].window, UsageWindow::Weekly);
        assert_eq!(snapshots[1].utilization_pct, Some(40.0));
    }

    #[test]
    fn official_usage_state_backs_off_after_rate_limit() {
        let now = chrono::Utc::now();
        let mut state = OfficialUsageState::default();

        state.register_rate_limited(now);

        assert!(state.should_skip_request(now + Duration::minutes(4)));
        assert!(!state.should_skip_request(now + Duration::minutes(5)));
    }

    #[test]
    fn local_window_without_events_is_unavailable_without_usage_values() {
        let snapshot = local_window(
            Agent::ClaudeCode,
            UsageWindow::FiveHour,
            &[],
            chrono::Utc::now(),
            Duration::hours(5),
        );

        assert_eq!(snapshot.source, SnapshotSource::Unavailable);
        assert_eq!(snapshot.used_tokens, None);
        assert_eq!(snapshot.burn_rate_tokens_per_min, None);
    }

    fn official_window() -> UsageSnapshot {
        UsageSnapshot {
            agent: Agent::ClaudeCode,
            window: UsageWindow::FiveHour,
            utilization_pct: Some(12.0),
            used_tokens: None,
            burn_rate_tokens_per_min: None,
            reset_at: None,
            limit_reached_at: None,
            observed_at: Some(chrono::Utc::now()),
            source: SnapshotSource::Official,
        }
    }
}
