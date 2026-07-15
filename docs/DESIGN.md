# DESIGN: agent-usage v2 — CodexBar の設計を取り入れたアーキテクチャ見直し

対象読者: 実装担当(Codex)。このドキュメント単体で実装に着手できることを目標に書いている。
関連ドキュメント:

- `docs/PLAN.md` — 初期プラン(v1)。現行実装のベース。
- `docs/PLAN-cli-rate-limits.md` — CLI からレートリミットを取得するデータソース調査。
  **本書はこのプランのデータソース調査結果(JSON 形状・コマンド・パス)を正とした上で、
  アーキテクチャをフォールバック明示型の「戦略チェーン」に再構成する。**
  同プランの §4〜§8 の実装詳細(app-server クライアント、セッション JSONL パーサ、
  hook スクリプト)はそのまま流用し、配線方法だけ本書に従う。

---

## 1. 背景 — なぜ設計を見直すのか

[steipete/CodexBar](https://github.com/steipete/CodexBar) は同じ問題(AI コーディング CLI の
レートリミット可視化)を扱う成熟した macOS アプリで、57+ プロバイダを単一の抽象で扱っている。
そのドキュメント(`docs/architecture.md`, `docs/provider.md`, `docs/claude.md`, `docs/codex.md`)
をレビューした結果、agent-usage の現行設計には以下のギャップがあることが分かった。

### 現行実装の問題点

1. **フォールバックがプロバイダ内にハードコードされ、暗黙的**。
   `claude.rs` の `snapshot()` は「official 失敗 → local」を `match` で直書きしており、
   ソースを増やす(hook キャッシュ、app-server RPC)たびに各プロバイダの分岐が複雑化する。
2. **失敗が観測できない**。official が 429 で落ちても `Ok(None)` に潰され、
   UI からは「なぜ localEstimate なのか」が分からない。デバッグ手段が `eprintln!` しかない。
3. **フェッチ失敗時に値が消える/フリッカする**。前回成功値のメモリ内キャッシュがなく、
   一時的なエラーで UI が「Unavailable」に落ちる。
4. **タイムアウト境界がない**。`reqwest` にタイムアウト未設定。今後の子プロセス
   (app-server)呼び出しでは無限待ちのリスクがある。
5. **常駐 UX が HUD 頼み**。CodexBar の中核 UX である「メニューバーで常に見える」に相当する
   トレイ常駐がない(HUD は場所を取り、他ウィンドウと干渉する)。
6. **設定機構がない**。ポーリング間隔・ソースの有効/無効がすべてコンパイル時定数。

### CodexBar から採用する設計原則

| # | CodexBar の原則 | agent-usage への適用 |
|---|---|---|
| P1 | `ProviderFetchStrategy` プロトコル(id / kind / isAvailable / fetch / shouldFallback)による戦略チェーン | Rust trait `FetchStrategy` + 順序付きチェーン実行器(§4) |
| P2 | `ProviderFetchOutcome` = attempts + errors をデバッグ UI / CLI `--verbose` に出す | `FetchAttempt` ログを snapshot に同梱し、ダッシュボードに表示(§5, §10) |
| P3 | "prefer cached data over flapping; show clear errors when stale" | last-good キャッシュ + 鮮度(`observed_at`)表示。失敗時は古い値を「stale」マーク付きで出し続ける(§6) |
| P4 | "timeout-bounded; no unbounded waits on network/PTY" | 全戦略に per-strategy タイムアウト必須(§4) |
| P5 | `ProviderDescriptor` による能力宣言と網羅的レジストリ | 小規模版: `ProviderDescriptor` 構造体 + 静的レジストリ。将来の第3プロバイダ(Gemini CLI 等)追加を1ファイルで完結させる(§4.4) |
| P6 | 更新間隔・ソース選択がユーザー設定可能 | `settings.json`(アプリデータディレクトリ)+ 設定 UI 最小版(§7) |
| P7 | メニューバー常駐 + 使用率アイコン | Tauri tray icon にタイトルテキストで利用率を表示。HUD はオプションに格下げ(§10) |
| P8 | 失敗した実行環境要因の隔離(quarantine 検出で 30 分スキップ等) | 戦略単位のクールダウン: 起動失敗した戦略は一定時間スキップ(§4.3) |

### RunCatNeo から採用する設計原則(追補)

[runcat-dev/RunCatNeo](https://github.com/runcat-dev/RunCatNeo)(macOS メニューバー常駐の
システムモニタ)もレビューした。特に **Custom Metrics**(ローカル JSON を fs イベントで監視して
カード表示する機構。公式サンプルに Claude Code / Codex 連携が存在する)から以下を採用する。

| # | RunCat の原則 | agent-usage への適用 |
|---|---|---|
| P9 | 対象ファイルをポーリングせず fs イベントで監視し、変更を即時反映 | `notify` による watcher。ターン終了→数秒で表示更新(§8) |
| P10 | 文書化されたローカル JSON 契約で他ツールと疎結合連携(producer 側整形・atomic write・失敗時は failed 表示で自動復旧) | RunCat 互換 Custom Metrics JSON のエクスポート(§9) |
| P11 | メトリクス駆動アニメーション(負荷→猫の走る速度) | 任意フェーズ: burn rate でトレイアイコンをアニメ(§11 Phase 6) |

### 採用しないもの(スコープ判断)

- **ブラウザ Cookie 抽出 / Web スクレイピング**(claude.ai の Web API):
  Claude CLIの公式 `/usage` が利用できない場合の最終フォールバックとして採用する。
  Cookieは `sessionKey` のみ許可し、値はKeychainへ保存してアプリログ・settings JSONには出さない。
- **PTY で `claude` TUI を起動して `/usage` をパースする方式**: CodexBarの実装知見を採用する。
  専用作業ディレクトリ、短命プロセス、期限付き読み取り、ANSI除去、初回trust画面処理を必須とする。
- **57 プロバイダ対応・多言語・WidgetKit・Sparkle 相当**: 対象は Claude Code + Codex の 2 つ。
  ただし §4.4 のレジストリ構造で第 3 プロバイダの追加コストは低く保つ。
- **CLI コンパニオンツール**: 将来課題。`FetchStrategy` 層を UI 非依存にしておくことで道は残る。

---

## 2. 全体アーキテクチャ

```
┌────────────────────────── Rust backend (src-tauri) ──────────────────────────┐
│                                                                              │
│  core/registry.rs        core/engine.rs             core/scheduler.rs       │
│  ProviderDescriptor ──▶  ProviderEngine              interval from settings  │
│  (静的レジストリ)          ├─ strategies: Vec<Box<dyn FetchStrategy>>          │
│                           ├─ チェーン実行 + per-strategy timeout + cooldown   │
│                           ├─ merge_with_local()                              │
│                           └─ last-good cache (ProviderState)                 │
│                                      │                                       │
│  providers/ (戦略の実装)              ▼                                       │
│   claude_hook.rs   ┐        ProviderOutcome { snapshots, attempts }          │
│   claude_oauth.rs  │                 │                                       │
│   codex_app_server.rs │              ├──▶ core/history.rs (SQLite 永続化)     │
│   codex_session_limits.rs │          └──▶ emit "usage://snapshot"            │
│   local_estimate.rs ┘                                                        │
│   (jsonl.rs / creds.rs は共有ユーティリティ)                                    │
│                                                                              │
│  core/settings.rs — settings.json の読み書き + "settings://changed" emit      │
│  core/watcher.rs  — fs イベント監視(notify)→ デバウンス付き即時 refresh       │
│  core/export.rs   — RunCat 互換 Custom Metrics JSON の書き出し                │
│  tray.rs          — トレイアイコン(タイトル=最重要窓の %、メニュー)              │
└──────────────────────────────────────────────────────────────────────────────┘
                       │ events / #[tauri::command]
┌──────────────────────▼───────────────────────────────────────────────────────┐
│ Frontend (src/)                                                              │
│  dashboard/ — リング・履歴・ソースバッジ・鮮度・attempts デバッグパネル・設定     │
│  hud/      — 従来どおり(設定でオフ可能)                                       │
└──────────────────────────────────────────────────────────────────────────────┘
```

変わらないもの: Tauri v2、Rust backend + Vanilla TS/Vite、SQLite 履歴、
イベント `usage://snapshot`、HUD/Dashboard の 2 ウィンドウ構成、フィクスチャ駆動テスト。

---

## 3. データソースと優先順位(確定仕様)

`docs/PLAN-cli-rate-limits.md` の調査結果を戦略チェーンとして表現する。
**チェーンは設定ファイルに順序付きで宣言され、上から順に試行する。**

### Claude Code

| 順 | 戦略 id | kind | 内容 | 鮮度上限 |
|---|---|---|---|---|
| 1 | `claude.hook-cache` | LocalFile | Claude Code の Stop hook が書くキャッシュ JSON(`rate_limits.five_hour/seven_day`)を読む | 30 分 |
| 2 | `claude.oauth-api` | Http | `GET https://api.anthropic.com/api/oauth/usage`(現行実装を戦略化。429 対策・低頻度は維持) | — |
| 3 | `claude.cli-usage` | Subprocess | Claude CLIをPTY起動し`/usage`のCurrent session / Current weekを解析 | 20秒 |
| 4 | `claude.web-usage` | Http | `claude.ai/api/organizations/{id}/usage`をsessionKey Cookieで取得 | 15秒 |
| 常時併走 | `claude.local-estimate` | LocalFile | `~/.claude/projects`とClaude Desktop埋め込み`.claude/projects`のtoken集計 | — |

### Codex

| 順 | 戦略 id | kind | 内容 | 鮮度上限 |
|---|---|---|---|---|
| 1 | `codex.app-server` | Subprocess | `codex -s read-only -a untrusted app-server` に JSON-RPC で `initialize` → `account/rateLimits/read` | — |
| 2 | `codex.session-log` | LocalFile | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` 末尾の非 null `payload.rate_limits` | 24 時間 |
| 3 | `codex.local-estimate` | LocalFile | セッション JSONL のトークン集計(現行) | — |

補足(CodexBar `docs/codex.md` からの追加知見):

- CodexBar は同じ app-server RPC で `account/read` も呼び、アカウント識別(email / plan)を
  取得している。**v2 では初回スコープ外**だが、`RateLimitReading` に
  `identity: Option<String>` を将来追加できる余地だけ残す(フィールド追加のみで済む設計に)。
- `~/.codex/archived_sessions/*.jsonl` もローカル集計の走査対象に加える(CodexBar が対象にしている。
  アーカイブ後もその日の使用量に含まれるため)。
- **検討事項(現時点ではチェーンに入れない)**: RunCatNeo の Codex サンプルは
  `~/.codex/hooks.json` の **Stop hook** でターン毎に使用量 JSON を書かせている。
  Codex にも hook 機構が存在するため、app-server RPC が使えない環境向けの受動経路
  (Claude と同じ hook キャッシュ方式)になり得る。実機で hooks.json の仕様
  (発火タイミング・stdin ペイロード)を確認してから採否を決めること。

### マージ規則(現行踏襲 + 拡張)

1st/2nd ソースは「利用率 % + リセット時刻」しか持たないため、`local-estimate` の結果を常に併走させ、
`merge_with_local(primary, local)` で `used_tokens` / `burn_rate_tokens_per_min` を補完する。
primary に存在しない窓は local から **append** して落とさない(PLAN-cli-rate-limits §1 のとおり)。

---

## 4. コア抽象(新規)— CodexBar `ProviderFetchStrategy` の Rust 版

### 4.1 `FetchStrategy` trait — `src-tauri/src/core/strategy.rs`(新規)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StrategyKind { LocalFile, Subprocess, Http }

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("not available: {0}")]
    Unavailable(String),        // 前提を満たさない(バイナリ無し・creds 無し・ファイル無し)
    #[error("stale: {0}")]
    Stale(String),              // データはあったが鮮度上限超過
    #[error("rate limited")]
    RateLimited,                // HTTP 429 等
    #[error("timeout")]
    Timeout,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[async_trait::async_trait]
pub trait FetchStrategy: Send + Sync {
    fn id(&self) -> &'static str;              // 例 "codex.app-server"
    fn kind(&self) -> StrategyKind;
    fn source(&self) -> SnapshotSource;        // この戦略が返す snapshot の source 値
    fn timeout(&self) -> std::time::Duration;  // P4: 必須。既定値なし(各戦略が宣言)

    /// 環境チェック(バイナリ存在・creds 存在など)。fetch より安価であること。
    async fn is_available(&self) -> bool;

    /// レートリミット読み値を返す。% とリセット時刻のみ。トークン集計は local-estimate の責務。
    async fn fetch(&self) -> Result<RateLimitReading, FetchError>;
}
```

タイムアウト目安: LocalFile 2s / Subprocess 10s / Http 15s。
`should_fallback` は CodexBar ではメソッドだが、本設計では **`FetchError` の型で一律に決める**
(全 variant がフォールバック対象。チェーン打ち切りは無し)。理由: ソースが 3 つしかなく、
「後段を試さない方が良いエラー」が現状存在しないため。将来必要になったら trait にメソッドを足す。

### 4.2 チェーン実行器 — `src-tauri/src/core/engine.rs`(新規)

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchAttempt {              // P2: デバッグ可観測性
    pub strategy_id: String,
    pub kind: StrategyKind,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub outcome: AttemptOutcome,       // Success | Skipped(reason) | Failed(message)
}

pub struct ProviderEngine {
    pub descriptor: &'static ProviderDescriptor,
    strategies: Vec<Box<dyn FetchStrategy>>,   // 優先順
    local: Box<dyn FetchStrategy>,             // local-estimate(常に併走)
    cooldowns: HashMap<&'static str, DateTime<Utc>>,  // P8
}

impl ProviderEngine {
    /// チェーンを上から試行。成功した最初の reading を local とマージして返す。
    /// 全滅なら local のみ。attempts には試行した全戦略の記録が入る。
    pub async fn collect(&mut self) -> ProviderOutcome;
}

pub struct ProviderOutcome {
    pub snapshots: Vec<UsageSnapshot>,
    pub attempts: Vec<FetchAttempt>,
}
```

実行規則:

1. 各戦略について: 設定で無効なら `Skipped("disabledInSettings")`、
   クールダウン中なら `Skipped("cooldown")`、`is_available() == false` なら
   `Skipped("unavailable")` を attempts に記録して次へ。
2. `tokio::time::timeout(strategy.timeout(), strategy.fetch())` で実行。
3. 最初の `Ok(reading)` でチェーン打ち切り → `merge_with_local`。
   **打ち切り後の戦略は attempts に記録しない**(試していないものは載せない)。
4. local-estimate はチェーンとは別枠で毎回実行する(マージ材料 + 最終フォールバック)。

### 4.3 クールダウン(P8)

`Unavailable`(spawn 失敗・バイナリ無し)で失敗した Subprocess 戦略は **10 分間スキップ**
(定数 `STRATEGY_COOLDOWN`)。`RateLimited` の Http 戦略は **指数バックオフ**
(5 分 → 10 分 → 20 分、上限 60 分。成功でリセット)。LocalFile 戦略はクールダウンしない(安価なため)。
クールダウンはメモリ内のみ(再起動でリセット)。

### 4.4 プロバイダレジストリ(P5)— `src-tauri/src/core/registry.rs`(新規)

```rust
pub struct ProviderDescriptor {
    pub agent: Agent,
    pub display_name: &'static str,
    pub strategy_ids: &'static [&'static str],  // 既定のチェーン順
    pub supports_hook_install: bool,            // Claude のみ true
}

/// 網羅的レジストリ。プロバイダ追加 = ここに 1 エントリ + 戦略ファイルを追加。
pub fn descriptors() -> &'static [ProviderDescriptor];
pub fn build_engine(agent: Agent, settings: &Settings) -> ProviderEngine;
```

`lib.rs` の `collect_snapshot()` は「レジストリを走査して各 engine の `collect()` を呼ぶ」
だけになり、プロバイダ名のハードコードが消える。

---

## 5. モデル変更 — `src-tauri/src/core/model.rs`

`PLAN-cli-rate-limits.md` §1 と同一。要点のみ再掲 + 追加:

- `SnapshotSource` に `OfficialCli` / `SessionLog` / `HookCache` を追加(既存値は維持)。
- `UsageSnapshot.observed_at: Option<DateTime<Utc>>` を追加(P3 の鮮度表示の基盤)。
- 中間型 `RateLimitWindow` / `RateLimitReading`、変換 `rate_limit_snapshots()`、
  汎用 `merge_with_local()` を追加。
- **追加(本書)**: `AppSnapshot` に `attempts: Vec<FetchAttempt>` を追加(P2)。
  SQLite には保存しない(揮発的なデバッグ情報。DB スキーマを汚さない)。

SQLite(`history.rs`)は `observed_at TEXT` カラムの冪等 ALTER と
`format_source()` の 3 値追加のみ(PLAN-cli-rate-limits §2 と同一)。

---

## 6. last-good キャッシュと鮮度(P3)

`AppState` を拡張:

```rust
pub struct AppState {
    latest: Arc<RwLock<Option<AppSnapshot>>>,   // 既存
}
```

- `collect()` が **全戦略失敗**(local も空)だったプロバイダは、`latest` 内の前回値を
  そのまま残す(上書きしない)。UI は `observed_at` / `captured_at` の経過で stale を判定する。
- UI 規則: `observed_at` が 2× ポーリング間隔より古い → 値をグレー表示 + 「12m old」バッジ。
  さらに 30 分超 → 「stale」警告表示。**値は消さない**(CodexBar: "prefer cached data over
  flapping; show clear errors when stale")。

---

## 7. 設定(P6)— `src-tauri/src/core/settings.rs`(新規)

保存先: `<data_local_dir>/agent-usage/settings.json`。tmp+rename でアトミック書き込み。

```jsonc
{
  "version": 1,
  "refreshIntervalSecs": 300,        // 60–900 にクランプ。env AGENT_USAGE_INTERVAL_SECS が最優先
                                     // watcher(§8)有効時は 300 で十分。無効時は 120 推奨
  "hudEnabled": true,                // P7: HUD をオプション化
  "fsWatchEnabled": true,            // P9: fs イベント監視(§8)。env AGENT_USAGE_WATCH=0 が最優先
  "exportRuncatMetrics": false,      // P10: RunCat 互換 JSON 書き出し(§9)
  "providers": {
    "claudeCode": { "enabled": true, "disabledStrategies": [] },
    "codex":      { "enabled": true, "disabledStrategies": [] }
  }
}
```

- Tauri コマンド: `get_settings() -> Settings` / `update_settings(patch) -> Settings`。
  更新時に `settings://changed` を emit し、スケジューラは次ティックから新間隔を使う
  (`tokio::select!` で「sleep or 設定変更通知」を待つ形に scheduler.rs を変更)。
- 設定 UI はダッシュボード内の 1 セクションで足りる(別ウィンドウは作らない):
  更新間隔セレクト、HUD on/off、戦略ごとの on/off トグル、
  「Enable Claude Code hook」ボタン(PLAN-cli-rate-limits §7 の hook インストーラを呼ぶ)。

---

## 8. イベント駆動更新(P9)— `src-tauri/src/core/watcher.rs`(新規)

RunCat の Custom Metrics は対象 JSON をポーリングせず fs イベントで監視し、変更を即時反映する。
同じ方式を取り込み、「ターン終了 → 数秒以内に表示更新」を実現する。

- `notify` クレート(macOS では FSEvents バックエンド)で以下を監視する:
  - Claude hook キャッシュ: `<data_dir>/agent-usage/claude_rate_limits.json`
  - `~/.claude/projects/`(再帰)
  - `~/.codex/sessions/`(再帰)
- イベント受信 → **2 秒デバウンス**(タイマーリセット方式。連続書き込みを 1 回にまとめる)→
  通常の collect を 1 回実行して emit。
- **キャッシュとの整合**: watcher 起点の refresh では、変更が検知されたパターンの
  `RECENT_EVENTS_CACHE`(jsonl.rs、10 分 TTL)エントリを先に無効化する。
  さもないとイベント駆動にした意味がなくなる。
- **スケジューラは廃止しない**。低頻度フォールバック(watcher 有効時は既定 300s)として維持する。
  ファイル変更を伴わない状態変化(リセット時刻の経過による % 変化)と watcher 障害を拾う安全網。
- watcher の初期化失敗はログに記録するのみで、アプリはスケジューラのみで動作継続する。
- 無効化手段: settings の `fsWatchEnabled`(§7)、env `AGENT_USAGE_WATCH=0`(最優先)。

## 9. エクスポート(P10)— RunCat 互換 Custom Metrics JSON — `src-tauri/src/core/export.rs`(新規)

RunCatNeo の Custom Metrics スキーマ(`docs/CustomMetricsSchema.md`)に準拠した JSON を書き出す。
RunCat Neo ユーザーはファイルをソース登録するだけで、メニューバーの Metrics Bar に使用率が出る
(本アプリのトレイ §10.1 の補完。RunCat 専用ではなく xbar / SketchyBar 等からも読める汎用契約)。

- 設定 `exportRuncatMetrics: true` のとき、snapshot 更新毎に以下を書き出す
  (RunCat は 1 ファイル = 1 カードのため 2 ファイル):
  - `<data_dir>/agent-usage/export/claude-code.json`
  - `<data_dir>/agent-usage/export/codex.json`
  - `<data_dir>/agent-usage/export/snapshot.json`(生の `AppSnapshot`。他ツール向け)
- 出力例(スキーマ: `title` 必須 / `symbol` / `metricsBarValue` / `metrics[]` / `lastUpdatedDate` 必須):

```jsonc
{
  "title": "Claude Code",
  "symbol": "asterisk",
  "metricsBarValue": "42%",            // 最逼迫窓の %。localEstimate のみなら "~42%" 形式
  "metrics": [
    { "title": "5h",   "formattedValue": "42% · resets 14:05", "normalizedValue": 0.42 },
    { "title": "Week", "formattedValue": "71% · resets Mon",   "normalizedValue": 0.71 },
    { "title": "Burn", "formattedValue": "1.2k tok/min" }
  ],
  "lastUpdatedDate": "2026-07-15T04:50:40Z"   // observed_at を ISO 8601 で
}
```

- 契約規律(RunCat の原則をそのまま守る):
  - **整形は producer(本アプリ)側の責務**。単位・% 記号・丸めを含めた表示文字列を出す。
  - `normalizedValue` は 0–1 にクランプ。% が取れない窓(localEstimate のみ)では省略し、
    `formattedValue` にトークン量を出す。
  - **atomic write**(tmp + rename)。部分読みを発生させない。1MB 未満を保つ。
- README に RunCat 側の登録手順を記載する:
  RunCat Neo → Settings → Custom Metrics → Add JSON Source → 上記 2 ファイルを指定。

## 10. UI 変更

### 10.1 トレイ常駐(P7)— `src-tauri/src/tray.rs`(新規)

CodexBar の中核 UX の移植。Tauri v2 の `tray-icon` 機能を使う。

- **タイトルテキスト**(macOS はトレイにテキスト表示可): 最も逼迫している窓の利用率を
  `C 42% · X 71%` 形式で表示(C=Claude, X=Codex。% はチェーン 1st/2nd ソースの値のみ。
  localEstimate しかない場合は `~` を付ける: `C ~38%`)。
- 90% 以上の窓があればタイトル先頭に `⚠` を付ける。
- メニュー: 各プロバイダ × 窓の「% / リセットまでの残り時間 / ソースバッジ」、
  `Open Dashboard` / `Refresh Now` / `Quit`。
- `tauri.conf.json`: `trayIcon` 追加。`activationPolicy: accessory` は維持。
  HUD ウィンドウは `hudEnabled` 設定に従い生成/破棄。

### 10.2 ダッシュボード

- ソースバッジ: `sourceLabel`(CLI / hook / API / session / est. / —)+ 鮮度(§6 の規則)。
- **デバッグパネル(P2)**: 折りたたみ式「Last fetch attempts」。`AppSnapshot.attempts` を
  戦略ごとに `✓ codex.app-server 812ms` / `✗ claude.oauth-api rate limited` /
  `— claude.hook-cache skipped: unavailable` の形式で列挙。
  CodexBar の debug UI / `--verbose` に相当し、「なぜこのソースなのか」に常に答えられるようにする。
- 設定セクション(§7)。

### 10.3 TS 型 — `src/shared/types.ts`

```ts
export type SnapshotSource =
  | "official" | "officialCli" | "sessionLog" | "hookCache"
  | "localEstimate" | "unavailable";
export type StrategyKind = "localFile" | "subprocess" | "http";

export interface FetchAttempt {
  strategyId: string;
  kind: StrategyKind;
  startedAt: string;
  durationMs: number;
  outcome: { status: "success" } | { status: "skipped"; reason: string }
         | { status: "failed"; message: string };
}

export interface UsageSnapshot { /* 既存 + observedAt: string | null */ }
export interface AppSnapshot   { /* 既存 + attempts: FetchAttempt[] */ }
export interface Settings      { /* §7 と同形 */ }
```

---

## 11. 実装フェーズ(実装エージェントへの指示)

各フェーズは独立にコンパイル・テスト可能。**フェーズ順に PR を分けること。**

### Phase 1 — コア抽象とモデル(UI 変更なし)
1. `core/model.rs`: SnapshotSource 3 値追加、`observed_at`、`RateLimitReading` 系、
   `merge_with_local`(claude.rs から移動・汎用化)。
2. `core/strategy.rs`: `FetchStrategy` / `FetchError` / `StrategyKind`。
3. `core/engine.rs`: `ProviderEngine` / `FetchAttempt` / クールダウン。
4. `core/registry.rs`: descriptor 2 件。
5. 既存 2 ソースを戦略に移植: `providers/claude_oauth.rs`(現 claude.rs の official 部分、
   `reqwest` に timeout 設定を追加)、`providers/local_estimate.rs`(現 local_windows を
   Claude/Codex 両対応の戦略でラップ)。`claude.rs` / `codex.rs` は engine 構築に置換。
6. `history.rs` マイグレーション。`types.ts` / `tauri.ts` 追随。
- 受け入れ基準: `cargo test` / `cargo clippy --all-targets` / `npx tsc --noEmit` パス。
  挙動は現行と同等(ソースは official / localEstimate のみ)だが attempts が snapshot に載る。

### Phase 2 — Codex の公式ソース
`PLAN-cli-rate-limits.md` §4(app-server クライアント)・§5(セッション JSONL)を
そのまま実装し、戦略 `codex.app-server` / `codex.session-log` として登録。
フィクスチャ・テストも同プラン §11 のとおり。

### Phase 3 — Claude hook キャッシュ
`PLAN-cli-rate-limits.md` §7 を実装(hook スクリプト・インストーラ・リーダー)し、
戦略 `claude.hook-cache` として登録。インストールはダッシュボードのボタンから明示的に
(自動インストール禁止・ユーザー同意必須、同プランのとおり)。

### Phase 4 — 設定 + スケジューラ + watcher
`core/settings.rs`、`update_settings` コマンド、scheduler の動的間隔、
戦略の enable/disable を engine に配線。`core/watcher.rs`(§8): notify 監視・デバウンス・
キャッシュ無効化・`fsWatchEnabled` / `AGENT_USAGE_WATCH` の配線。
既定間隔は watcher 有効時 300s / 無効時 120s。

### Phase 5 — トレイ + UI 仕上げ + エクスポート
`tray.rs`、HUD のオプション化、ダッシュボードのソースバッジ/鮮度/attempts パネル/設定セクション。
`core/export.rs`(§9): RunCat 互換 JSON + `snapshot.json` の書き出し、`exportRuncatMetrics` 設定、
README への RunCat 登録手順追記。

### Phase 6(任意)— トレイアニメーション(P11)
burn rate / 利用率に応じてトレイアイコンをフレームアニメーションさせる(RunCat 本体の UX)。
必須要件: フレームレート上限(最大 5fps)・アニメ off 設定・アイドル時(burn rate 0)は静止。
省電力を優先し、実装コストが高ければ見送ってよい。

---

## 12. テスト計画(差分)

`PLAN-cli-rate-limits.md` §11 のテストに加えて:

- **engine のチェーン実行**: モック戦略(`Ok` / `Err(Unavailable)` / `Err(Timeout)` /
  sleep でタイムアウト誘発)を組み合わせ、(a) 最初の成功で打ち切る、(b) 全滅で local のみ、
  (c) attempts に Skipped/Failed が正しく載る、(d) 打ち切り後の戦略が attempts に載らない、を検証。
- **クールダウン**: Unavailable 後 10 分以内の再 collect で `Skipped("cooldown")` になること。
- **RateLimited バックオフ**: 5→10→20 分と伸び、成功でリセットされること(時刻は注入可能にする)。
- **settings**: 不正 JSON → 既定値へフォールバック、クランプ、`disabledStrategies` が
  `Skipped("disabledInSettings")` になること。
- **last-good**: 全滅ティックで `latest` の前回値が保持されること。
- **watcher**: tempdir を監視対象にし、(a) ファイル書き込みで refresh コールバックが発火する、
  (b) 2 秒以内の連続 5 回書き込みが 1 回の refresh にまとまる(デバウンス)、
  (c) 監視対象パスが存在しなくても初期化がエラーにならない、を検証。
- **export**: (a) 必須フィールド(`title` / `metrics` / `lastUpdatedDate`)が常に存在、
  (b) `normalizedValue` が 0–1 にクランプされる、(c) tmp+rename の atomic write
  (書き込み途中のファイル名が最終名と異なる)、(d) localEstimate のみの場合は
  `normalizedValue` 省略で `formattedValue` にトークン量が出る、を検証。

いずれも実 `~/.claude` / `~/.codex` に依存しない(tempdir + フィクスチャ + コマンド差し替え)。
Linux CI で `cargo test` / `cargo clippy` / `npx tsc --noEmit` が全パスすること。
トレイ・HUD の目視確認は macOS 実機(従来どおり)。

---

## 13. 決定記録(Design Decisions)

| 決定 | 理由 |
|---|---|
| 戦略チェーンを trait + Vec で明示化 | CodexBar P1。ソース追加が「ファイル 1 個 + レジストリ 1 行」になり、プロバイダ本体の分岐が消える |
| `should_fallback` を導入しない | ソース 3 つで打ち切り要件が存在しない。YAGNI。エラー型で一律フォールバック |
| attempts を SQLite に保存しない | 揮発的デバッグ情報。履歴 DB の目的(グラフ)と無関係でスキーマを汚す |
| Web Cookieを自動＋手動で許可 | CLIやOAuthが利用できない環境でも使用率を取得する。CookieはallowlistとKeychainで隔離 |
| Claude CLI PTYを採用 | CodexBarが実運用している公式CLI `/usage` を使い、hook/cacheへの依存を解消する |
| トレイをプライマリ、HUD をオプションに | CodexBar の実証済み UX。常時視認は menu bar が最も低コスト。HUD が好みのユーザー向けに設定で残す |
| 短命 spawn(常駐 app-server にしない) | PLAN-cli-rate-limits §4 の判断を踏襲。sleep/wake 復旧不要、120s 周期に spawn ~1s は無視できる |
| hook 自動インストール禁止 | 他ツールの設定ファイル(`~/.claude/settings.json`)を黙って書き換えない。CodexBar の permission transparency と同じ思想 |
| fs イベント監視を追加し、ポーリングをフォールバックに格下げ | RunCat の実証済みパターン。即時性(ターン終了→数秒)と省電力を両立。リセット時刻経過や watcher 障害はスケジューラが安全網として拾う |
| エクスポートは RunCat 互換スキーマを採用 | 独自形式を発明せず既存エコシステム(RunCat Neo の Custom Metrics)にそのまま乗れる。producer-side formatting・atomic write・failed 表示からの自動復旧という契約が明文化済みで流用できる |
| Claude の statusLine 連携は不採用 | `statusLine.command` は 1 つしか登録できず、既存ユーザー設定と競合する(RunCatNeo のサンプルはこの方式)。Stop hook で同じ `rate_limits` が取れる。hook が発火しない環境が実機診断で判明した場合のみ、既存コマンドをパススルーする wrapper 方式を再検討 |
| LUCA アーキテクチャ自体は不採用 | SwiftUI 固有の構成。本アプリは「collect → 単一 snapshot → emit → UI が購読」の単方向フローで同等の原則を既に満たしている |
