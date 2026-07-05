use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use chrono::{DateTime, Utc};
use glob::glob;
use serde_json::Value;

use crate::core::model::TokenEvent;

pub fn read_token_events(pattern: &str) -> anyhow::Result<Vec<TokenEvent>> {
    let mut events = Vec::new();
    for entry in glob(pattern)? {
        let path = entry?;
        events.extend(read_token_events_file(&path)?);
    }
    events.sort_by_key(|event| event.timestamp);
    Ok(events)
}

pub fn read_token_events_file(path: &Path) -> anyhow::Result<Vec<TokenEvent>> {
    let file = File::open(path)?;
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let (Some(timestamp), Some(tokens)) = (extract_timestamp(&value), extract_tokens(&value))
        {
            events.push(TokenEvent { timestamp, tokens });
        }
    }
    Ok(events)
}

pub fn extract_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    let raw = value
        .pointer("/timestamp")
        .or_else(|| value.pointer("/created_at"))
        .or_else(|| value.pointer("/message/timestamp"))?
        .as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

pub fn extract_tokens(value: &Value) -> Option<u64> {
    let total_paths = [
        "/total_tokens",
        "/usage/total_tokens",
        "/message/usage/total_tokens",
        "/response/usage/total_tokens",
        "/payload/info/last_token_usage/total_tokens",
        "/payload/info/total_token_usage/total_tokens",
    ];
    if let Some(total) = total_paths
        .iter()
        .find_map(|path| value.pointer(path).and_then(Value::as_u64))
    {
        return Some(total);
    }

    let direct_paths = [
        "/usage/input_tokens",
        "/usage/output_tokens",
        "/payload/info/last_token_usage/input_tokens",
        "/payload/info/last_token_usage/output_tokens",
        "/payload/info/last_token_usage/reasoning_output_tokens",
        "/payload/info/last_token_usage/cached_input_tokens",
    ];

    let direct_sum: u64 = direct_paths
        .iter()
        .filter_map(|path| value.pointer(path).and_then(Value::as_u64))
        .sum();
    if direct_sum > 0 {
        return Some(direct_sum);
    }

    let input = value
        .pointer("/usage/input_tokens")
        .or_else(|| value.pointer("/message/usage/input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = value
        .pointer("/usage/output_tokens")
        .or_else(|| value.pointer("/message/usage/output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached = value
        .pointer("/usage/cache_creation_input_tokens")
        .or_else(|| value.pointer("/usage/cache_read_input_tokens"))
        .or_else(|| value.pointer("/payload/info/last_token_usage/cached_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = input + output + cached;
    (total > 0).then_some(total)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::extract_tokens;

    #[test]
    fn sums_nested_input_and_output_tokens() {
        let value = json!({
            "message": {
                "usage": {
                    "input_tokens": 120,
                    "output_tokens": 30
                }
            }
        });

        assert_eq!(extract_tokens(&value), Some(150));
    }

    #[test]
    fn reads_codex_payload_last_token_usage() {
        let value = json!({
            "payload": {
                "info": {
                    "last_token_usage": {
                        "input_tokens": 120,
                        "cached_input_tokens": 30,
                        "output_tokens": 40,
                        "reasoning_output_tokens": 10,
                        "total_tokens": 200
                    }
                }
            }
        });

        assert_eq!(extract_tokens(&value), Some(200));
    }
}
