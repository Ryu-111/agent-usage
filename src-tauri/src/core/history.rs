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
                 observed_at TEXT,
                 source TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_usage_snapshots_time
                ON usage_snapshots(captured_at, agent, window);",
        )?;

        let columns = conn
            .prepare("PRAGMA table_info(usage_snapshots)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "observed_at") {
            conn.execute(
                "ALTER TABLE usage_snapshots ADD COLUMN observed_at TEXT",
                [],
            )
            .context("migrating history observed_at column")?;
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
                  burn_rate_tokens_per_min, reset_at, limit_reached_at, observed_at, source)
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
                        window.observed_at.map(|value| value.to_rfc3339()),
                        format_source(window.source),
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

    use crate::core::model::{AgentUsage, UsageSnapshot};

    use super::*;

    #[test]
    fn migrates_and_inserts_observed_at() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(dir.path().join("history.sqlite3")).unwrap();
        let observed_at = Utc::now();
        let snapshot = AppSnapshot {
            captured_at: observed_at,
            agents: vec![AgentUsage {
                agent: Agent::Codex,
                windows: vec![UsageSnapshot {
                    agent: Agent::Codex,
                    window: UsageWindow::FiveHour,
                    utilization_pct: Some(42.0),
                    used_tokens: Some(100),
                    burn_rate_tokens_per_min: Some(1.0),
                    reset_at: None,
                    limit_reached_at: None,
                    observed_at: Some(observed_at),
                    source: SnapshotSource::OfficialCli,
                }],
            }],
        };

        store.insert_app_snapshot(&snapshot).unwrap();

        let conn = store.conn.lock().unwrap();
        let stored: String = conn
            .query_row(
                "SELECT observed_at FROM usage_snapshots LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, observed_at.to_rfc3339());
    }
}
