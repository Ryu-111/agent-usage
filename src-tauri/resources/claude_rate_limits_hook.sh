#!/bin/sh
# agent-usage: cache the rate_limits from Claude Code hook stdin for the HUD app.
CACHE_DIR="${AGENT_USAGE_CACHE_DIR:-$HOME/Library/Application Support/agent-usage}"
mkdir -p "$CACHE_DIR"
TMP="$CACHE_DIR/.claude_rate_limits.json.tmp.$$"
printf '{"written_at":%s,"hook_payload":' "$(date +%s)" > "$TMP"
cat >> "$TMP"
printf '}' >> "$TMP"
mv -f "$TMP" "$CACHE_DIR/claude_rate_limits.json"
exit 0
