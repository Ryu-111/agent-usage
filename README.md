# agent-usage

Claude Code と Codex の使用量を常駐 HUD とダッシュボードで可視化する macOS 向け Tauri v2 アプリです。

## What It Shows

- Claude Code / Codex それぞれの 5 時間ローリング窓と週次窓
- ローカル JSONL から推定した token 使用量と burn rate
- Claude Code OAuth usage API が使える環境では公式 utilization を優先表示
- 小型の常時最前面 HUD と、詳細確認用の dashboard window

## Data Sources

- Claude Code hook: Stop hook cacheが利用可能な場合に最優先で取得
- Claude Code OAuth: credentials file / macOS Keychain の OAuth tokenで `https://api.anthropic.com/api/oauth/usage` を取得
- Claude Code CLI: Claude CLIのPTY経由 `/usage` を公式使用率ソースとして取得
- Claude Code Web: sessionKey Cookieで `claude.ai` usage APIを取得（任意、Keychain保存）
- Claude Code local: `~/.claude/projects/**/*.jsonl` とClaude Desktop埋め込み `.claude/projects/**/*.jsonl`
- Codex local: `~/.codex/sessions/**/*.jsonl`

Claudeの公式ソースは `hook → OAuth → CLI → Web` の順でフォールバックし、ローカル推定は常にtoken数補完として併走します。

## Development

```sh
npm install
npm run tauri:dev
```

Rust 側の単体テスト:

```sh
cd src-tauri
cargo test
```

## macOS Build

```sh
npm run tauri:build
```

個人利用の ad-hoc 署名では、初回起動時に Finder で右クリックして「開く」が必要になる場合があります。HUD は通常のデスクトップアプリと同じように Dock / タスクバーへ最小化できる構成です。

## CI / Release

- `CI`（`.github/workflows/ci.yml`）: PR と `main` への push で、フロントエンドの型チェックとビルド、Rust の fmt / clippy / test（Linux・macOS）、gitleaks による機密スキャンを実行します。
- `Release`（`.github/workflows/release.yml`）: `v0.2.0` のようなタグを push すると、macOS universal の `.dmg` をビルドしてドラフトの GitHub Release に添付します。Developer ID 署名はしていない ad-hoc 署名です。
- Dependabot が GitHub Actions・npm・Cargo の依存を毎週まとめて更新します。

## Current Scope

このリポジトリではコードとフィクスチャ駆動のテストまでを扱います。最終的な `.app` / `.dmg` 生成と透明 HUD の目視確認は macOS 実機で行ってください。
