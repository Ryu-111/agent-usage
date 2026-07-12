use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Agent {
    ClaudeCode,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    Web,
    LocalEstimate,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub agent: Agent,
    pub window: UsageWindow,
    pub utilization_pct: Option<f64>,
    pub used_tokens: Option<u64>,
    pub burn_rate_tokens_per_min: Option<f64>,
    pub reset_at: Option<DateTime<Utc>>,
    pub limit_reached_at: Option<DateTime<Utc>>,
    pub observed_at: Option<DateTime<Utc>>,
    pub source: SnapshotSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
