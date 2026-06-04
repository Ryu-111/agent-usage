use std::path::Path;

use agent_usage::providers::jsonl::read_token_events_file;

#[test]
fn reads_claude_style_jsonl_usage() {
    let events = read_token_events_file(Path::new("tests/fixtures/claude.jsonl")).unwrap();

    assert_eq!(events.len(), 2);
    assert_eq!(events.iter().map(|event| event.tokens).sum::<u64>(), 1500);
}

#[test]
fn reads_codex_style_jsonl_usage() {
    let events = read_token_events_file(Path::new("tests/fixtures/codex.jsonl")).unwrap();

    assert_eq!(events.len(), 2);
    assert_eq!(events.iter().map(|event| event.tokens).sum::<u64>(), 2000);
}
