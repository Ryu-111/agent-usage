# agent-usage — Claude Code / Codex レートリミット可視化 macOS スタンドアロンアプリ

## Context（なぜ作るのか）
Claude Code・Codex には「5時間ローリング窓」と「週次キャップ」の二層レートリミットがあるが、
現状 `/usage`・`/status` を**手動で叩いた時の一瞬のスナップショット**でしか確認できない。
リミット到達で作業が突然止まるのを避けるため、両エージェントの使用率を**常時グラフで可視化**し、
リセットまでの時間・消費ペース・上限到達予測を一目で把握できる macOS スタンドアロンアプリを作る。

リポジトリは現状ほぼ空（`README.md` と Rust 用 `.gitignore` のみ）のグリーンフィールド。ゼロから構築する。

## 確定した方針
- **UIデザイン**: フローティングHUD（常駐・小型・半透明・ドラッグ可）＋ フルダッシュボード窓（リング/履歴/予測）の2画面構成
- **対象エージェント**: Claude Code + Codex 両方
- **フレームワーク**: **Tauri v2**（リッチなダッシュボード＋グラフ＋常駐HUDを1アプリの複数ウィンドウで両立できるため）

## データソース戦略（ハイブリッド）
正確性と可用性を両立するため、公式値を主・ローカル解析を補助とする。

### Claude Code
- **公式値（主）**: `GET https://api.anthropic.com/api/oauth/usage`
  - 認証: `~/.claude/.credentials.json` の OAuth access token を `Authorization: Bearer <token>`
  - 必須ヘッダ: `anthropic-beta: oauth-2025-04-20`, `Content-Type: application/json`,
    そして **`User-Agent: claude-code/<version>`**（無いと厳しい 429 バケットに落ちる）
  - レスポンスの `anthropic-ratelimit-unified-5h-utilization` / `-7d-utilization` と reset 時刻を取得
  - ⚠️ **このエンドポイントは叩きすぎると 429（Retry-After 無し）**。→ ポーリングは低頻度（既定5分）＋ディスクキャッシュ＋指数バックオフ
- **ローカル推定（補助/オフライン/バーンレート）**: `~/.claude/projects/**/*.jsonl` を解析。
  メッセージ毎の token 使用量＋timestamp から 5h ローリング和・7d 和・消費ペース(tok/min)・到達予測を算出。

### Codex
- `~/.codex/sessions/**/*.jsonl` を解析（2026/4 以降トークンベース）。5h/週の token 使用量・バーンレートを算出。
- 公式 `/status` 相当のエンドポイントがあれば best-effort で利用（未確定なので初版はローカル解析を主とする）。

## アーキテクチャ
Tauri v2 アプリ。Rust バックエンド（コア＋データ取得）＋ Web フロントエンド（2ウィンドウ）。

### ウィンドウ構成
1. **HUD (`hud`)**: frameless / transparent / always-on-top / skipTaskbar / draggable。
   CC・Codex の 5h/週バーをコンパクト表示。クリックで Dashboard を開く。
2. **Dashboard (`main`)**: フル窓。リング(5h/週 × 2エージェント)・バーンレートのスパークライン・
   7日間履歴の棒グラフ・上限到達予測・各リセット時刻。

### Rust バックエンド（`src-tauri/src/`）
- `core/model.rs`: ドメイン型
  `UsageSnapshot { agent, window(FiveHour|Weekly), utilization_pct, used_tokens, reset_at, source(Official|LocalEstimate) }`
- `core/scheduler.rs`: tokio による定期リフレッシュ。更新を Tauri イベントで全ウィンドウに emit。
- `core/history.rs`: 7日履歴を SQLite (`rusqlite`) に永続化（アプリ未起動時間も含めグラフ化するため）。
- `providers/creds.rs`: `~/.claude/.credentials.json` 読み取り（トークン期限切れ検出含む）。
- `providers/claude.rs`: OAuth usage API ポーリング（429 バックオフ＋キャッシュ）＋ jsonl 解析。
- `providers/codex.rs`: `~/.codex/sessions` jsonl 解析。
- `main.rs`: Tauri セットアップ、ウィンドウ生成、`#[tauri::command]`、（任意）トレイアイコン。
- 主要クレート: `tauri`(v2), `tokio`, `reqwest`(rustls), `serde`/`serde_json`, `rusqlite`(bundled), `chrono`, `glob`, `dirs`, `anyhow`/`thiserror`。

### フロントエンド（`src/`）
- `hud/`(index.html / hud.ts / hud.css): 半透明・最小スタイル、2行バー。
- `dashboard/`(index.html / dashboard.ts / dashboard.css): リング(SVG)、棒グラフ＋スパークライン
  （軽量に uPlot もしくは手書き SVG）。
- Vanilla TS + Vite（Tauri 既定）。Rust から `emit` されるイベントを購読してリアルタイム更新。

### macOS スタンドアロン化（`tauri.conf.json` / `Info.plist`）
- `app.macOSPrivateApi` 有効化（透明ウィンドウ用）。
- macOS `activationPolicy: "accessory"`（= `LSUIElement`）で Dock/Cmd+Tab に出さない常駐ユーティリティ化。
- HUD ウィンドウ: `decorations:false`, `transparent:true`, `alwaysOnTop:true`, `skipTaskbar:true`。
- バンドラで `.app` + `.dmg` を生成。個人利用は ad-hoc 署名（または初回 右クリック→開く）で起動可、と README に明記。

## 実装ステップ
1. **（最初に Push するもの）** 本プランを `docs/PLAN.md` としてリポジトリに追加し commit & push。
2. Tauri v2 雛形作成（`src-tauri/` + `src/` + `package.json` + `tauri.conf.json`）。
3. コアモデル＋ SQLite 履歴ストア。
4. Claude provider: jsonl 解析（先に確実に動く方）→ OAuth usage API（429 対策込み）。
5. Codex provider: sessions jsonl 解析。
6. scheduler でポーリング＆イベント emit。
7. Dashboard UI（リング/グラフ）。
8. HUD UI（半透明・ドラッグ・クリックで展開）。
9. macOS パッケージング設定（accessory / 透明 / always-on-top / .dmg）。
10. README（ビルド手順・データソース・署名の注意）。

## 検証方法
- **コア単体**: `cargo test`。サンプル jsonl のフィクスチャで CC/Codex の集計（5h/週・バーンレート）を検証。
- **OAuth API**: モックレスポンスで unified utilization のパースと 429 バックオフを単体テスト。
- **アプリ起動**: `npm run tauri dev` を**ユーザーの Mac で**実行し、HUD ＋ Dashboard に実データが
  出ること、HUD が常時最前面・ドラッグ可・クリックで展開することを目視確認。
- **配布物**: `npm run tauri build` → `.dmg` を開き、Dock に出ない・常駐 HUD が動くことを確認。

## 重要な制約
- **このリモート実行環境は Linux**。Tauri の macOS バンドル（`.app`/`.dmg`）生成と最終動作確認は
  **macOS 上でのみ可能**。当環境ではコード作成・`cargo` でのコンパイル/単体テストまでを行い、
  実機ビルド・GUI 動作確認はユーザーの Mac で実施する想定。
- 実 `~/.claude` / `~/.codex` データはユーザーの Mac にあり当環境からは見えないため、解析ロジックは
  フィクスチャ駆動で実装・テストする。
