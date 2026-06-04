# agent-usage

Claude Code と Codex の使用量を常駐 HUD とダッシュボードで可視化する macOS 向け Tauri v2 アプリです。

## What It Shows

- Claude Code / Codex それぞれの 5 時間ローリング窓と週次窓
- ローカル JSONL から推定した token 使用量と burn rate
- Claude Code OAuth usage API が使える環境では公式 utilization を優先表示
- 小型の常時最前面 HUD と、詳細確認用の dashboard window

## Data Sources

- Claude Code official: `~/.claude/.credentials.json` の OAuth token で `https://api.anthropic.com/api/oauth/usage` を低頻度に取得
- Claude Code local: `~/.claude/projects/**/*.jsonl`
- Codex local: `~/.codex/sessions/**/*.jsonl`

Claude の公式 API は 429 になりやすいため、初期実装ではローカル推定を常にフォールバックとして使います。

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

## Current Scope

このリポジトリではコードとフィクスチャ駆動のテストまでを扱います。最終的な `.app` / `.dmg` 生成と透明 HUD の目視確認は macOS 実機で行ってください。
