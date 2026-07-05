use chrono::{DateTime, TimeZone, Utc};

use crate::core::model::{Agent, SnapshotSource, UsageSnapshot, UsageWindow};

#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitReading {
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
    pub observed_at: Option<DateTime<Utc>>,
    pub source: SnapshotSource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitWindow {
    pub used_percent: Option<f64>,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<DateTime<Utc>>,
}

impl RateLimitReading {
    pub fn into_snapshots(self, agent: Agent) -> Vec<UsageSnapshot> {
        [
            (
                infer_window(self.primary.as_ref(), UsageWindow::FiveHour),
                self.primary,
            ),
            (
                infer_window(self.secondary.as_ref(), UsageWindow::Weekly),
                self.secondary,
            ),
        ]
        .into_iter()
        .filter_map(|(window, reading)| {
            reading.map(|reading| UsageSnapshot {
                agent,
                window,
                utilization_pct: reading.used_percent,
                used_tokens: None,
                burn_rate_tokens_per_min: None,
                reset_at: reading.resets_at,
                limit_reached_at: None,
                observed_at: self.observed_at,
                source: self.source,
            })
        })
        .collect()
    }
}

fn infer_window(reading: Option<&RateLimitWindow>, fallback: UsageWindow) -> UsageWindow {
    match reading.and_then(|reading| reading.window_minutes) {
        Some(minutes) if minutes <= 1440 => UsageWindow::FiveHour,
        Some(_) => UsageWindow::Weekly,
        None => fallback,
    }
}

pub fn unix_seconds(value: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(value, 0).single()
}

pub fn merge_rate_limits_with_local(
    rate_limits: Vec<UsageSnapshot>,
    local: Vec<UsageSnapshot>,
) -> Vec<UsageSnapshot> {
    let mut merged: Vec<UsageSnapshot> = rate_limits
        .into_iter()
        .map(|mut rate_snapshot| {
            if let Some(local_snapshot) = local.iter().find(|snapshot| {
                snapshot.agent == rate_snapshot.agent && snapshot.window == rate_snapshot.window
            }) {
                rate_snapshot.used_tokens = local_snapshot.used_tokens;
                rate_snapshot.burn_rate_tokens_per_min = local_snapshot.burn_rate_tokens_per_min;
            }
            rate_snapshot
        })
        .collect();

    for local_snapshot in local {
        if !merged.iter().any(|snapshot| {
            snapshot.agent == local_snapshot.agent && snapshot.window == local_snapshot.window
        }) {
            merged.push(local_snapshot);
        }
    }

    merged
}
