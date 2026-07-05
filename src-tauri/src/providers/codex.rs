use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::{Duration, Utc};
use tokio::sync::Mutex;

use crate::core::model::{Agent, UsageSnapshot};
use crate::providers::claude::local_windows;
use crate::providers::codex_cli::AppServerClient;
use crate::providers::codex_session::read_latest_rate_limits;
use crate::providers::jsonl::read_recent_token_events;
use crate::providers::rate_limits::merge_rate_limits_with_local;

static APP_SERVER_STATE: OnceLock<Mutex<AppServerState>> = OnceLock::new();

pub struct CodexProvider {
    sessions_pattern: Option<String>,
    sessions_root: Option<PathBuf>,
    app_server: Option<AppServerClient>,
}

impl Default for CodexProvider {
    fn default() -> Self {
        Self {
            sessions_pattern: default_sessions_pattern(),
            sessions_root: default_sessions_root(),
            app_server: Some(AppServerClient::default_codex()),
        }
    }
}

impl CodexProvider {
    pub fn with_sessions_pattern(pattern: String) -> Self {
        Self {
            sessions_pattern: Some(pattern),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub fn with_sessions_root(mut self, sessions_root: PathBuf) -> Self {
        self.sessions_root = Some(sessions_root);
        self
    }

    #[cfg(test)]
    pub fn with_app_server(mut self, app_server: AppServerClient) -> Self {
        self.app_server = Some(app_server);
        self
    }

    #[cfg(test)]
    pub fn without_app_server(mut self) -> Self {
        self.app_server = None;
        self
    }

    pub async fn snapshot(&self) -> anyhow::Result<Vec<UsageSnapshot>> {
        if let Some(root) = &self.sessions_root {
            if let Some(reading) = read_latest_rate_limits(root)? {
                let rate_limits = reading.into_snapshots(Agent::Codex);
                let local = self.local_snapshot()?;
                return Ok(merge_rate_limits_with_local(rate_limits, local));
            }
        }

        if let Some(app_server) = &self.app_server {
            let state_lock = APP_SERVER_STATE.get_or_init(|| Mutex::new(AppServerState::default()));
            let should_try = {
                let state = state_lock.lock().await;
                state.should_try()
            };
            if should_try {
                match app_server.fetch_rate_limits().await {
                    Ok(reading) => {
                        state_lock.lock().await.register_success();
                        let rate_limits = reading.into_snapshots(Agent::Codex);
                        let local = self.local_snapshot()?;
                        return Ok(merge_rate_limits_with_local(rate_limits, local));
                    }
                    Err(_) => state_lock.lock().await.register_failure(),
                }
            }
        }

        self.local_snapshot()
    }

    fn local_snapshot(&self) -> anyhow::Result<Vec<UsageSnapshot>> {
        let events = match &self.sessions_pattern {
            Some(pattern) => read_recent_token_events(pattern, Duration::days(8))?,
            None => Vec::new(),
        };
        Ok(local_windows(Agent::Codex, &events))
    }
}

#[derive(Debug)]
struct AppServerState {
    next_attempt_at: chrono::DateTime<Utc>,
}

impl Default for AppServerState {
    fn default() -> Self {
        Self {
            next_attempt_at: Utc::now(),
        }
    }
}

impl AppServerState {
    fn should_try(&self) -> bool {
        Utc::now() >= self.next_attempt_at
    }

    fn register_success(&mut self) {
        self.next_attempt_at = Utc::now() + Duration::minutes(15);
    }

    fn register_failure(&mut self) {
        self.next_attempt_at = Utc::now() + Duration::minutes(10);
    }
}

fn default_sessions_pattern() -> Option<String> {
    dirs::home_dir().map(|home| format!("{}/.codex/sessions/**/*.jsonl", home.display()))
}

fn default_sessions_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".codex/sessions"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::Utc;

    use crate::core::model::SnapshotSource;

    use super::{AppServerState, CodexProvider};

    #[tokio::test]
    async fn uses_session_log_rate_limits_when_app_server_is_disabled() {
        let root =
            std::env::temp_dir().join(format!("agent-usage-codex-session-{}", std::process::id()));
        let day = root.join("2026/06/10");
        fs::create_dir_all(&day).unwrap();
        let line = format!(
            r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"total_tokens":200}}}},"rate_limits":{{"primary":{{"used_percent":22.0,"window_minutes":300,"resets_at":1774036800}},"secondary":{{"used_percent":55.0,"window_minutes":10080,"resets_at":1774580400}}}}}}}}"#,
            chrono::Utc::now().to_rfc3339()
        );
        fs::write(day.join("rollout-test.jsonl"), line).unwrap();

        let provider =
            CodexProvider::with_sessions_pattern(format!("{}/**/*.jsonl", root.display()))
                .with_sessions_root(root.clone())
                .without_app_server();
        let snapshots = provider.snapshot().await.unwrap();

        assert_eq!(snapshots[0].source, SnapshotSource::SessionLog);
        assert_eq!(snapshots[0].utilization_pct, Some(22.0));
        assert_eq!(snapshots[0].used_tokens, Some(200));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn app_server_success_sets_low_frequency_cooldown() {
        let mut state = AppServerState::default();

        state.register_success();

        assert!(!state.should_try());
        assert!(state.next_attempt_at - Utc::now() > chrono::Duration::minutes(14));
    }
}
