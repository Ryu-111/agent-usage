use std::path::PathBuf;

use crate::core::model::{merge_with_local, rate_limit_snapshots, Agent, UsageSnapshot};
use crate::providers::claude::local_windows;
use crate::providers::codex_app_server::AppServerClient;
use crate::providers::codex_session_limits::read_latest_rate_limits;
use crate::providers::jsonl::read_token_events;

#[derive(Debug, Clone)]
pub struct CodexProvider {
    sessions_pattern: Option<String>,
    sessions_root: Option<PathBuf>,
    app_server: Option<AppServerClient>,
}

impl Default for CodexProvider {
    fn default() -> Self {
        let home = dirs::home_dir();
        Self {
            sessions_pattern: home
                .as_ref()
                .map(|home| format!("{}/.codex/sessions/**/*.jsonl", home.display())),
            sessions_root: home.map(|home| home.join(".codex/sessions")),
            app_server: AppServerClient::default_codex(),
        }
    }
}

impl CodexProvider {
    pub fn with_sessions_pattern(pattern: String) -> Self {
        Self {
            sessions_pattern: Some(pattern),
            sessions_root: None,
            app_server: None,
        }
    }

    pub fn with_sessions_root(mut self, root: PathBuf) -> Self {
        self.sessions_root = Some(root);
        self
    }

    pub fn with_app_server(mut self, client: AppServerClient) -> Self {
        self.app_server = Some(client);
        self
    }

    pub fn without_app_server(mut self) -> Self {
        self.app_server = None;
        self
    }

    pub async fn snapshot(&self) -> anyhow::Result<Vec<UsageSnapshot>> {
        let local = self.local_snapshot()?;

        if let Some(client) = &self.app_server {
            match client.fetch_rate_limits().await {
                Ok(reading) => {
                    return Ok(merge_with_local(
                        rate_limit_snapshots(Agent::Codex, &reading),
                        local,
                    ))
                }
                Err(err) => eprintln!("codex app-server unavailable: {err:#}"),
            }
        }

        if let Some(root) = &self.sessions_root {
            if let Ok(Some(reading)) = read_latest_rate_limits(root) {
                return Ok(merge_with_local(
                    rate_limit_snapshots(Agent::Codex, &reading),
                    local,
                ));
            }
        }

        Ok(local)
    }

    fn local_snapshot(&self) -> anyhow::Result<Vec<UsageSnapshot>> {
        let events = match &self.sessions_pattern {
            Some(pattern) => read_token_events(pattern)?,
            None => Vec::new(),
        };
        Ok(local_windows(Agent::Codex, &events))
    }
}
