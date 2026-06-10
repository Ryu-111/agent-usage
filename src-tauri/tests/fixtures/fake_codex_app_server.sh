#!/bin/sh
# Fake `codex app-server` for tests. FAKE_MODE: ok (default) | error | hang.
MODE="${FAKE_MODE:-ok}"

while IFS= read -r line; do
  case "$line" in
    *'"id":1'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"some/notification","params":{}}'
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"userAgent":"fake"}}'
      ;;
    *'"id":2'*)
      case "$MODE" in
        error)
          printf '%s\n' '{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"not logged in"}}'
          ;;
        hang)
          sleep 60
          ;;
        *)
          printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"rateLimits":{"primary":{"usedPercent":12.5,"windowDurationMins":300,"resetsAt":4070912400},"secondary":{"usedPercent":40.0,"windowDurationMins":10080,"resetsAt":4071517200}}}}'
          ;;
      esac
      ;;
  esac
done
