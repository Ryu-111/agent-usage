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
        self.conn
            .lock()
            .expect("history mutex poisoned")
            .execute_batch(
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
                source TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_usage_snapshots_time
                ON usage_snapshots(captured_at, agent, window);",
            )?;
        Ok(())
    }

    pub fn insert_app_snapshot(&self, snapshot: &AppSnapshot) -> anyhow::Result<()> {
        let conn = self.conn.lock().expect("history mutex poisoned");
        for agent in &snapshot.agents {
            for window in &agent.windows {
                conn.execute(
                    "INSERT INTO usage_snapshots
                    (captured_at, agent, window, utilization_pct, used_tokens,
                     burn_rate_tokens_per_min, reset_at, limit_reached_at, source)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
        SnapshotSource::LocalEstimate => "localEstimate",
        SnapshotSource::Unavailable => "unavailable",
    }
}
