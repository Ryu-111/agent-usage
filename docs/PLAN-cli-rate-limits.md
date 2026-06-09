# PLAN: CLI からレートリミットを定期取得するアーキテクチャへの変更

## Context(なぜ変えるのか)

現状の agent-usage のデータソースは以下の通り:

- **Claude Code**: 非公式 OAuth エンドポイント `GET https://api.anthropic.com/api/oauth/usage` が主
  (429 が頻発・undocumented で破壊変更リスクあり)、`~/.claude/projects/**/*.jsonl` の
  トークン集計によるローカル推定が補助。
- **Codex**: `~/.codex/sessions/**/*.jsonl` のトークン量からのローカル**推定のみ**。
  実際の利用率 %・リセット時刻は取得できていない。

これを「**CLI 自身が公開する公式なレートリミット情報を一次ソースとして定期取得する**」
アーキテクチャに変更する。スケジューラによる定期ポーリングの骨格(`core/scheduler.rs` →
`collect_snapshot()` → SQLite 永続化 → `usage://snapshot` イベント emit)は維持し、
プロバイダ層のデータソースと優先順位を差し替える。

### 調査で確定したデータソース

**Codex CLI**
- `codex app-server`(JSON-RPC over stdio)に公式メソッド `account/rateLimits/read` がある。
  応答: `{ rateLimits: { primary: { usedPercent, windowDurationMins, resetsAt }, secondary: {...} } }`
  (camelCase。primary ≈ 5h/300min、secondary ≈ 週/10080min)。
- セッションロールアウト `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` の
  `token_count` イベントにも `payload.rate_limits`(`info` の**兄弟**フィールド)として
  同等の情報が記録される。`codex exec` は `rate_limits: null` を書くため null チェック必須。
  旧版は `resets_in_seconds`(相対)、現行は `resets_at`(unix 秒)— 両対応が必要。

**Claude Code CLI**
- v2.1.80+ で hook / statusline の stdin JSON に `rate_limits` が公式に公開される:
  ```json
  {"rate_limits": {"five_hour": {"used_percentage": 42.3, "resets_at": 1774036800},
                   "seven_day": {"used_percentage": 85.7, "resets_at": 1774580400}}}
  ```
- 能動的にポーリングできる公式 CLI コマンドは存在しないため、hook がキャッシュファイルへ
  書き出し、アプリがそれを読む方式とする。OAuth エンドポイントはフォールバックに格下げ。

### 取得優先順位(確定)

| エージェント | 1st | 2nd | 3rd |
|---|---|---|---|
| Codex | `codex app-server` RPC (`OfficialCli`) | セッション JSONL の rate_limits、24h 以内 (`SessionLog`) | ローカルトークン推定 (`LocalEstimate`) |
| Claude Code | hook キャッシュ、30 分以内 (`HookCache`) | OAuth usage API (`Official`) | ローカルトークン推定 (`LocalEstimate`) |

いずれの一次/二次ソースも利用率 %・リセット時刻のみを持つため、従来どおりローカル JSONL の
トークン集計を `used_tokens` / `burn_rate_tokens_per_min` の補完としてマージする。

---

## 実装ステップ

### 1. モデル変更 — `src-tauri/src/core/model.rs`

- `SnapshotSource` に variant を追加(既存値は DB 後方互換のため維持):
  ```rust
  pub enum SnapshotSource {
      Official,      // 既存: OAuth エンドポイント(Claude のフォールバックとして継続)
      OfficialCli,   // 新規: codex app-server JSON-RPC
      SessionLog,    // 新規: codex セッション JSONL の rate_limits
      HookCache,     // 新規: claude hook が書いたキャッシュファイル
      LocalEstimate,
      Unavailable,
  }
  ```
- `UsageSnapshot` に `observed_at: Option<DateTime<Utc>>` を追加
  (ソースデータの生成時刻: hook の書込時刻 / JSONL 行の timestamp / live RPC・HTTP は now)。
- 中間型と変換関数を追加:
  ```rust
  pub struct RateLimitWindow { pub window: UsageWindow, pub used_percent: Option<f64>, pub resets_at: Option<DateTime<Utc>> }
  pub struct RateLimitReading { pub windows: Vec<RateLimitWindow>, pub observed_at: DateTime<Utc>, pub source: SnapshotSource }
  pub fn rate_limit_snapshots(agent: Agent, reading: &RateLimitReading) -> Vec<UsageSnapshot>;
  ```
- `claude.rs` の `merge_official_with_local` を汎用化して移動:
  `pub fn merge_with_local(primary: Vec<UsageSnapshot>, local: Vec<UsageSnapshot>) -> Vec<UsageSnapshot>`。
  ロジックは現行どおり(local の used_tokens / burn_rate を primary へコピー)だが、
  primary に存在しない窓は local 側から **append** して落とさないようにする。

### 2. SQLite — `src-tauri/src/core/history.rs`

- `migrate()`: `PRAGMA table_info(usage_snapshots)` でカラム有無を確認し、無ければ
  `ALTER TABLE usage_snapshots ADD COLUMN observed_at TEXT` を冪等に実行。
  新規 DB 用の CREATE 文にも `observed_at TEXT` を含める。
- `format_source()` に `OfficialCli => "officialCli"`, `SessionLog => "sessionLog"`,
  `HookCache => "hookCache"` を追加(既存文字列は不変 → 旧行はそのまま有効)。
- `insert_app_snapshot()` の INSERT に `observed_at`(RFC3339)を追加。

### 3. TS 型 — `src/shared/types.ts`, `src/shared/tauri.ts`

- `SnapshotSource` union に `"officialCli" | "sessionLog" | "hookCache"` を追加。
- `UsageSnapshot.observedAt: string | null` を追加。
- 表示用 `export const sourceLabel: Record<SnapshotSource, string>`
  (例: officialCli→"CLI", hookCache→"hook", official→"API", sessionLog→"session",
  localEstimate→"est.", unavailable→"—")。
- `tauri.ts` の `demoSnapshot()` に `observedAt` を追加。

### 4. Codex app-server クライアント — 新規 `src-tauri/src/providers/codex_app_server.rs`

```rust
pub struct AppServerClient {
    /// 例: ["codex", "-s", "read-only", "-a", "untrusted", "app-server"]
    /// テストでは ["bash", "tests/fixtures/fake_codex_app_server.sh"] に差し替え
    pub command: Vec<String>,
    pub timeout: std::time::Duration,   // 既定 10s(交信全体)
}
impl AppServerClient {
    pub fn default_codex() -> Self;     // $CODEX_BIN 上書き → PATH の "codex" →
                                        // /opt/homebrew/bin, /usr/local/bin, ~/.local/bin をプローブ
                                        // (macOS GUI アプリは PATH が最小構成のため)
    pub async fn fetch_rate_limits(&self) -> anyhow::Result<RateLimitReading>;
}
```

`fetch_rate_limits` フロー(tokio::process):
1. stdin/stdout を piped、stderr を null で spawn。`kill_on_drop(true)`。
2. 交信全体を `tokio::time::timeout(self.timeout, ...)` で包む。タイムアウトは Err
   (drop で子プロセスは kill される)。
3. 送信: `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"agent-usage","title":"agent-usage","version":"0.1.0"}}}` + 改行。
4. stdout を行読みし、`id` の無い行(通知)や id 不一致の行はスキップして id:1 応答を待つ。
   `error` フィールドがあれば bail。
5. 送信: `{"jsonrpc":"2.0","id":2,"method":"account/rateLimits/read","params":{}}` + 改行。
6. id:2 応答を待ち、`result.rateLimits.primary / secondary` をパース
   (serde `rename_all = "camelCase"`: `used_percent`, `window_duration_mins`, `resets_at`)。
   `resets_at` は unix 秒(number)と RFC3339(string)の両対応ヘルパでパース。
7. 窓割当: `window_duration_mins <= 1440` → FiveHour、それ以外 → Weekly。
   欠落時は primary→FiveHour, secondary→Weekly。
8. 子プロセスは best-effort で kill(短命プロセスなので JSON-RPC shutdown は不要)。
9. `RateLimitReading { windows, observed_at: Utc::now(), source: OfficialCli }` を返す。

エラー処理: spawn 失敗(バイナリ無し)・タイムアウト・JSON-RPC error(未ログイン等)・
JSON 不正はすべて `Err` → 呼び出し側が JSONL フォールバックへ。失敗は `eprintln!` で 1 回ログ。

**方式判断**: 常駐子プロセス(`account/rateLimits/updated` 通知の購読)ではなく、
**スケジューラのティック毎に短命 spawn** する。プロセス監視・再起動・macOS sleep/wake 後の
パイプ復旧が不要で、120〜300 秒のポーリング周期には spawn コスト(~1s)は無視できる。
command を差し替え可能にしてあるため、将来常駐型へ移行する余地は残る。

### 5. Codex セッション JSONL フォールバック — 新規 `src-tauri/src/providers/codex_session_limits.rs`

```rust
pub fn read_latest_rate_limits(sessions_root: &Path) -> anyhow::Result<Option<RateLimitReading>>;
pub fn parse_rate_limits_line(line: &str) -> Option<RateLimitReading>;  // 単体テスト用に公開
```

- 走査: 年/月/日ディレクトリを新しい順(数値ソート)→ 日内は mtime 降順の `rollout-*.jsonl`
  → 各ファイルを**末尾から逆順**に見て最初の非 null `rate_limits` を返す。
  上限 ~7 日分 / ~30 ファイルで打ち切り、見つからなければ `Ok(None)`。
- パース対象行:
  ```json
  {"timestamp":"...","type":"event_msg","payload":{"type":"token_count","info":{...},
   "rate_limits":{"primary":{"used_percent":12.5,"window_minutes":300,"resets_at":1750000000},
                  "secondary":{"used_percent":40.0,"window_minutes":10080,"resets_at":1750600000}}}}
  ```
  - `payload.type == "token_count"` かつ `payload.rate_limits` が非 null であること
    (`rate_limits` は `info` の兄弟。`codex exec` は null を書く)。
  - 窓 struct(snake_case): `used_percent: Option<f64>`, `window_minutes: Option<i64>`,
    `resets_at: Option<i64>`(unix 秒), `resets_in_seconds: Option<i64>`(旧形式)。
  - resets 解決: `resets_at` 優先。無ければ `行の timestamp + resets_in_seconds`。
  - 窓割当は app-server と同じルール。`observed_at` = 行 timestamp、source = `SessionLog`。
- 鮮度: `observed_at` が 24h より古い読み値は破棄(定数 `SESSION_LIMITS_MAX_AGE`)。

### 6. CodexProvider 書き換え — `src-tauri/src/providers/codex.rs`

```rust
pub struct CodexProvider {
    sessions_pattern: Option<String>,     // トークンイベント用(既存)
    sessions_root: Option<PathBuf>,       // rate_limits 走査用(~/.codex/sessions)
    app_server: Option<AppServerClient>,  // None で RPC 無効(フォールバック経路のテスト用)
}
```

`snapshot()`: app-server RPC を試行 → 失敗時はセッション JSONL(≤24h)→ どちらかが取れたら
`merge_with_local(rate_limit_snapshots(...), local)`、両方ダメなら従来のローカル推定のみ。
テスト用ビルダ: `with_sessions_pattern` / `with_sessions_root` / `with_app_server` /
`without_app_server`。あわせて `claude.rs` の `local_window()` にも
`observed_at: Some(Utc::now())` を入れてフィールドを一貫させる。

### 7. Claude hook キャッシュ — 新規 `src-tauri/src/providers/claude_hook.rs` + `src-tauri/resources/claude_rate_limits_hook.sh`

**hook スクリプト**(`include_str!` で同梱。Tauri リソースバンドルは使わない):
```sh
#!/bin/sh
# agent-usage: Claude Code hook stdin の rate_limits をキャッシュファイルへ書き出す
CACHE_DIR="${AGENT_USAGE_CACHE_DIR:-$HOME/Library/Application Support/agent-usage}"
mkdir -p "$CACHE_DIR"
TMP="$CACHE_DIR/.claude_rate_limits.json.tmp.$$"
printf '{"written_at":%s,"hook_payload":' "$(date +%s)" > "$TMP"
cat >> "$TMP"
printf '}' >> "$TMP"
mv -f "$TMP" "$CACHE_DIR/claude_rate_limits.json"
exit 0
```
- 書込先は `dirs::data_local_dir()/agent-usage/claude_rate_limits.json`
  (macOS: `~/Library/Application Support/agent-usage/`、Linux 開発時: `~/.local/share/agent-usage/`)。
  インストーラが解決済み絶対パスを `AGENT_USAGE_CACHE_DIR` としてコマンド文字列に埋めるため
  スクリプト自体はポータブル。tmp+mv でアトミック書き込み。常に exit 0 で Claude Code を阻害しない。

**インストーラ / リーダー**:
```rust
pub const HOOK_SCRIPT: &str = include_str!("../../resources/claude_rate_limits_hook.sh");
pub fn hook_script_path() -> Option<PathBuf>;  // <data_local_dir>/agent-usage/claude_rate_limits_hook.sh
pub fn cache_file_path() -> Option<PathBuf>;
pub fn install_claude_hook(settings_path: &Path, script_path: &Path, cache_dir: &Path)
    -> anyhow::Result<HookInstallStatus>;       // Installed | AlreadyInstalled
pub fn is_claude_hook_installed(settings_path: &Path) -> anyhow::Result<bool>;
pub fn read_hook_cache(path: &Path, max_age: chrono::Duration)
    -> anyhow::Result<Option<RateLimitReading>>;
```
- `~/.claude/settings.json` へのマージは serde_json::Value で行い、未知キー・他の hook を保持。
  追加する構造:
  `{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"AGENT_USAGE_CACHE_DIR='<cache_dir>' '<script_path>'"}]}]}}`
  - **Stop hook** を使う(ターン終了毎に 1 回。PostToolUse は無駄に高頻度)。
  - 冪等性: command に `claude_rate_limits_hook.sh` を含む既存エントリがあれば置換、無ければ追加。
  - settings.json はアトミック書き込み(tmp + rename)。ファイルが無ければ hooks のみで新規作成。
- **ユーザー同意必須**: 起動時の自動インストールはしない(他人の `~/.claude/settings.json` を
  黙って書き換えない。OAuth フォールバックがあるので未インストールでも動く)。
- `read_hook_cache`: `{"written_at": <unix秒>, "hook_payload": {..., "rate_limits": {...}}}` を
  パース。ファイル無し / `rate_limits` 欠落(旧版 Claude Code)/ `written_at` が
  `HOOK_CACHE_MAX_AGE = 30min` より古い場合は `Ok(None)`。
  five_hour→FiveHour, seven_day→Weekly、`used_percentage`→utilization、
  `resets_at`(unix 秒)→reset_at、observed_at = written_at、source = `HookCache`。

**Tauri コマンド + UI**(`lib.rs` / `src/dashboard/`):
- `#[tauri::command] install_claude_hook_cmd() -> Result<String, String>`("installed"|"alreadyInstalled")
- `#[tauri::command] claude_hook_status() -> Result<bool, String>`
- ダッシュボードに小さな設定行を追加: ロード時に `claude_hook_status` を確認し、未インストールなら
  「Enable Claude Code hook(faster, no API calls)」ボタンを表示 → 実行後に再確認して確認文言を表示。

### 8. ClaudeProvider 優先順位 — `src-tauri/src/providers/claude.rs`

`hook_cache_path: Option<PathBuf>` フィールドを追加(既定 `claude_hook::cache_file_path()`、
テスト用 `with_hook_cache_path`)。`snapshot()`:
1. hook キャッシュ(30 分以内)→ `merge_with_local` して返す
2. ダメなら既存 `official_snapshot()`(OAuth、source=Official、observed_at=now)
3. それもダメならローカル推定のみ

### 9. スケジューラ — `src-tauri/src/lib.rs`

- 間隔を 300s → **120s** に短縮(429 リスクのある経路が一次ソースでなくなったため)。
- env `AGENT_USAGE_INTERVAL_SECS` で上書き可能に(デバッグ用)。

### 10. UI(最小スコープ)

- `src/dashboard/dashboard.ts`: source 表示を `sourceLabel[usage.source]` に変更。
  `observedAt` が 2 分超過なら「· 12m old」のような鮮度表示(`formatAge` ヘルパ追加)。
- `src/hud/hud.ts`: 変更なし(型整合のみ)。鮮度詳細はダッシュボードのみという判断。

### 11. テスト計画

新フィクスチャ(`src-tauri/tests/fixtures/`):
- `codex_rollout_rate_limits.jsonl` — null / 旧形式(resets_in_seconds)/ 新形式(resets_at)の
  token_count 行 + 無関係なイベント行の混在
- `codex_rollout_null_limits.jsonl` — null のみ(codex exec 相当)
- `claude_hook_cache.json` — written_at + rate_limits 入りの完全なラッパ
- `fake_codex_app_server.sh` — 実行可能スクリプト。stdin の `"id":1` に initialize 応答、
  `"id":2` に canned な rateLimits 応答(camelCase)。先に通知行も 1 行出す(スキップ検証用)。
  env `FAKE_MODE`(ok / error / hang)でエラー・タイムアウト系を再現。

新テスト:
- `tests/codex_session_limits.rs` — parse(新形式/旧形式/null/非 token_count)、
  tempdir に YYYY/MM/DD/rollout-*.jsonl を作って新しい順選択・null スキップを検証
- `tests/codex_app_server.rs` — fake スクリプト経由で正常系(FiveHour/Weekly マッピングと %)、
  FAKE_MODE=error で Err、FAKE_MODE=hang + timeout 1s でタイムアウト Err。`#[tokio::test]`、`#[cfg(unix)]`
- `tests/claude_hook.rs` — read_hook_cache の ok / stale / rate_limits 欠落、
  install_claude_hook の新規作成・冪等再実行・既存の無関係な hooks/キー保持
- `merge_with_local` の「primary に無い窓を local から補完」テスト、`rate_limit_snapshots` のマッピングテスト
- history.rs — 旧スキーマで作った DB を open → ALTER マイグレーション → insert 成功のテスト

依存変更: `src-tauri/Cargo.toml` の tokio features に `process`, `io-util` を追加。新クレートは不要。

### 12. 実装順序

1. model.rs(SnapshotSource / observed_at / RateLimitReading / merge_with_local)+ history.rs
   マイグレーション + types.ts / tauri.ts — まずコンパイルを通す
2. codex_session_limits.rs + フィクスチャ + テスト(純関数で高速フィードバック)
3. codex_app_server.rs + fake スクリプト + テスト
4. codex.rs の優先順位配線
5. claude_hook.rs(スクリプト・インストーラ・リーダー)+ テスト
6. claude.rs の優先順位配線
7. lib.rs のコマンド追加・間隔変更、ダッシュボード UI(hook ボタン・鮮度表示)
8. `cargo test` / `cargo clippy --all-targets` / `npx tsc --noEmit` 全パス

## 検証方法

**Linux 開発環境(この CI 環境で可能な範囲)**:
- `cd src-tauri && cargo test` — `~/.claude` / `~/.codex` 無しで全フィクスチャテストがパス
- `cargo clippy --all-targets` クリーン
- `npx tsc --noEmit`(または `npm run build`)で TS 型整合

**ユーザーの Mac での手動検証**:
1. ターミナルで `codex -s read-only -a untrusted app-server` を起動し、initialize と
   `account/rateLimits/read` の 2 行を手で流して応答形状を確認(特に `resetsAt` の型。
   実機と差異があればパーサを修正)。
2. アプリ起動 → Codex 行の source が `officialCli` で実 % が出ること。codex バイナリを
   一時的にリネームして refresh → `sessionLog` フォールバックを確認。数値が `codex` の
   `/status` 表示と一致すること。
3. ダッシュボードの「Enable Claude Code hook」→ `~/.claude/settings.json` に Stop hook が
   追加され、Claude Code(≥2.1.80)で 1 ターン実行後に
   `~/Library/Application Support/agent-usage/claude_rate_limits.json` が出現、
   source が `hookCache` に切り替わること。キャッシュ削除で `official`(OAuth)へ
   フォールバックすること。
4. 既存 `history.sqlite3` がエラーなく開け(マイグレーション)、新規行に `observed_at` が
   入っていること。
