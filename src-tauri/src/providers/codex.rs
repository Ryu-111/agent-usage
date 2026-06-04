use crate::core::model::{Agent, UsageSnapshot};
use crate::providers::claude::local_windows;
use crate::providers::jsonl::read_token_events;

#[derive(Debug, Clone, Default)]
pub struct CodexProvider {
    sessions_pattern: Option<String>,
}

impl CodexProvider {
    pub fn with_sessions_pattern(pattern: String) -> Self {
        Self {
            sessions_pattern: Some(pattern),
        }
    }

    pub async fn snapshot(&self) -> anyhow::Result<Vec<UsageSnapshot>> {
        let pattern = self.sessions_pattern.clone().or_else(|| {
            dirs::home_dir().map(|home| format!("{}/.codex/sessions/**/*.jsonl", home.display()))
        });
        let events = match pattern {
            Some(pattern) => read_token_events(&pattern)?,
            None => Vec::new(),
        };
        Ok(local_windows(Agent::Codex, &events))
    }
}
