use chrono::{DateTime, Duration, DurationRound, Utc};
use serde::Serialize;

use crate::core::model::{Agent, TokenEvent};

/// Hourly token totals per agent, used by the dashboard history chart.
/// Buckets are UTC hours so the UI can regroup them into local calendar days.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistory {
    pub bucket_start: DateTime<Utc>,
    pub bucket_minutes: u32,
    pub series: Vec<UsageHistorySeries>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistorySeries {
    pub agent: Agent,
    pub tokens: Vec<u64>,
}

pub fn hourly_history(
    now: DateTime<Utc>,
    hours: usize,
    series: Vec<(Agent, Vec<TokenEvent>)>,
) -> UsageHistory {
    let current_hour = now.duration_trunc(Duration::hours(1)).unwrap_or(now);
    let bucket_start = current_hour - Duration::hours(hours as i64 - 1);
    let series = series
        .into_iter()
        .map(|(agent, events)| {
            let mut tokens = vec![0_u64; hours];
            for event in events {
                if event.timestamp < bucket_start || event.timestamp > now {
                    continue;
                }
                let index = (event.timestamp - bucket_start).num_hours() as usize;
                if let Some(slot) = tokens.get_mut(index) {
                    *slot += event.tokens;
                }
            }
            UsageHistorySeries { agent, tokens }
        })
        .collect();
    UsageHistory {
        bucket_start,
        bucket_minutes: 60,
        series,
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn event(at: DateTime<Utc>, tokens: u64) -> TokenEvent {
        TokenEvent {
            timestamp: at,
            tokens,
        }
    }

    #[test]
    fn buckets_events_by_utc_hour_and_drops_out_of_range() {
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 12, 30, 0).unwrap();
        let history = hourly_history(
            now,
            3,
            vec![(
                Agent::Codex,
                vec![
                    event(Utc.with_ymd_and_hms(2026, 10, 11, 9, 59, 0).unwrap(), 1),
                    event(Utc.with_ymd_and_hms(2026, 10, 11, 10, 0, 0).unwrap(), 10),
                    event(Utc.with_ymd_and_hms(2026, 10, 11, 10, 45, 0).unwrap(), 5),
                    event(Utc.with_ymd_and_hms(2026, 10, 11, 12, 10, 0).unwrap(), 7),
                    event(Utc.with_ymd_and_hms(2026, 10, 11, 12, 40, 0).unwrap(), 100),
                ],
            )],
        );
        assert_eq!(
            history.bucket_start,
            Utc.with_ymd_and_hms(2026, 10, 11, 10, 0, 0).unwrap()
        );
        assert_eq!(history.series[0].tokens, vec![15, 0, 7]);
    }
}
