use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use rusqlite::{params, Connection};

use crate::core::model::{Agent, AppSnapshot, SnapshotSource, UsageWindow};

#[derive(Clone)]
pub struct HistoryStore {
    conn: Arc<Mutex<Connection>>,
}

impl HistoryStore {
    pub fn open_default() -> anyhow::Result<Self> {
        let base = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("agent-usage");
        std::fs::create_dir_all(&base).with_context(|| format!("create {}", base.display()))?;
        Self::open(base.join("history.sqlite3"))
    }

    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().expect("history mutex poisoned");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                captured_at TEXT NOT NULL,
                agent TEXT NOT NULL,
                window TEXT NOT NULL,
                utilization_pct REAL,
                used_tokens INTEGER,
                burn_rate_tokens_per_min REAL,
                reset_at TEXT,
                limit_reached_at TEXT,
                source TEXT NOT NULL,
                observed_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_usage_snapshots_time
                ON usage_snapshots(captured_at, agent, window);",
        )?;

        let mut columns = conn.prepare("PRAGMA table_info(usage_snapshots)")?;
        let has_observed_at = columns
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|name| name == "observed_at");
        if !has_observed_at {
            conn.execute("ALTER TABLE usage_snapshots ADD COLUMN observed_at TEXT", [])?;
        }
        Ok(())
    }

    pub fn insert_app_snapshot(&self, snapshot: &AppSnapshot) -> anyhow::Result<()> {
        let conn = self.conn.lock().expect("history mutex poisoned");
        for agent in &snapshot.agents {
            for window in &agent.windows {
                conn.execute(
                    "INSERT INTO usage_snapshots
                    (captured_at, agent, window, utilization_pct, used_tokens,
                     burn_rate_tokens_per_min, reset_at, limit_reached_at, source, observed_at)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        snapshot.captured_at.to_rfc3339(),
                        format_agent(agent.agent),
                        format_window(window.window),
                        window.utilization_pct,
                        window.used_tokens.map(|value| value as i64),
                        window.burn_rate_tokens_per_min,
                        window.reset_at.map(|value| value.to_rfc3339()),
                        window.limit_reached_at.map(|value| value.to_rfc3339()),
                        format_source(window.source),
                        window.observed_at.map(|value| value.to_rfc3339()),
                    ],
                )?;
            }
        }
        Ok(())
    }
}

fn format_agent(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "claudeCode",
        Agent::Codex => "codex",
    }
}

fn format_window(window: UsageWindow) -> &'static str {
    match window {
        UsageWindow::FiveHour => "fiveHour",
        UsageWindow::Weekly => "weekly",
    }
}

fn format_source(source: SnapshotSource) -> &'static str {
    match source {
        SnapshotSource::Official => "official",
        SnapshotSource::OfficialCli => "officialCli",
        SnapshotSource::SessionLog => "sessionLog",
        SnapshotSource::HookCache => "hookCache",
        SnapshotSource::LocalEstimate => "localEstimate",
        SnapshotSource::Unavailable => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::core::model::{
        Agent, AgentUsage, AppSnapshot, SnapshotSource, UsageSnapshot, UsageWindow,
    };

    use super::HistoryStore;

    #[test]
    fn migrates_pre_observed_at_schema_and_inserts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.sqlite3");
        {
            let conn = rusqlite::Connection::open(&path).expect("open raw");
            conn.execute_batch(
                "CREATE TABLE usage_snapshots (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    captured_at TEXT NOT NULL,
                    agent TEXT NOT NULL,
                    window TEXT NOT NULL,
                    utilization_pct REAL,
                    used_tokens INTEGER,
                    burn_rate_tokens_per_min REAL,
                    reset_at TEXT,
                    limit_reached_at TEXT,
                    source TEXT NOT NULL
                );",
            )
            .expect("create old schema");
        }

        let store = HistoryStore::open(path).expect("open with migration");
        let snapshot = AppSnapshot {
            captured_at: Utc::now(),
            agents: vec![AgentUsage {
                agent: Agent::Codex,
                windows: vec![UsageSnapshot {
                    agent: Agent::Codex,
                    window: UsageWindow::FiveHour,
                    utilization_pct: Some(12.5),
                    used_tokens: Some(100),
                    burn_rate_tokens_per_min: Some(2.0),
                    reset_at: None,
                    limit_reached_at: None,
                    source: SnapshotSource::OfficialCli,
                    observed_at: Some(Utc::now()),
                }],
            }],
        };
        store.insert_app_snapshot(&snapshot).expect("insert");
    }
}
