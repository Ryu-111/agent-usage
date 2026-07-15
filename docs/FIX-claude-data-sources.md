# FIX: Claude Code 側のデータが取得できない問題の診断と修正指示

対象読者: 実装担当(Codex)。対象コミット: `c78a898`(main)。

## 症状

ダッシュボード/HUD で Claude Code 側の値が出ない(Unavailable のまま)。
Claude Desktop 連携(`d978437` で追加)も効いていない模様。

## 現行の Claude チェーンと、それぞれが動かない理由

`ClaudeProvider::snapshot()` は hook キャッシュ → OAuth API → ローカル推定 の順。

### 原因1(最有力・確定): macOS では OAuth 資格情報が Keychain にあり、`.credentials.json` は存在しない

`creds.rs` は `~/.claude/.credentials.json` しか読まない。しかし **macOS の Claude Code は
OAuth 資格情報を macOS Keychain(サービス名 `Claude Code-credentials`)に保存し、
このファイルを作らない**(ファイル方式は Linux / 一部旧版のみ)。
そのため macOS 実機では `official_snapshot()` が常に `Ok(None)` になり、OAuth 経路は永遠に動かない。

**修正**: `read_claude_credentials()` にフォールバックを追加する。

```rust
// creds.rs: ファイルが無い/パース不能なら Keychain を試す(macOS のみ)
#[cfg(target_os = "macos")]
fn read_keychain_credentials() -> anyhow::Result<Option<ClaudeCredentials>> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()?;
    if !output.status.success() {
        return Ok(None); // 未ログイン or エントリ無し
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    Ok(parse_claude_credentials(raw.trim())) // 中身は .credentials.json と同形式の JSON
}
```

- 優先順: `.credentials.json` → Keychain。既存の `parse_claude_credentials` がそのまま使える
  (Keychain の中身も `{"claudeAiOauth":{"accessToken":...,"expiresAt":<ms>}}`)。
- 注意: `security` 呼び出しは初回に Keychain アクセス許可ダイアログが出ることがある。
  「常に許可」を選べば以後出ない旨を README に記載すること。
- テスト: `security` コマンドは Command を差し替え可能な形にして、
  fake スクリプト(成功/失敗/空出力)でユニットテストする。

### 原因2(確定バグ): Desktop セッションファイルを JSONL 行パーサで読んでいる

`claude.rs` の `default_desktop_patterns()` は `local_*.json`(**単一 JSON ファイル**)にマッチするが、
読み手は `read_token_events_file()` = **1行ずつ `serde_json::from_str`** する JSONL パーサ。
ファイルが整形(複数行)JSON の場合、全行がパース失敗して黙って 0 イベントになる。

**修正**: `read_token_events_file()` に「行パースが全滅した場合はファイル全体を 1 つの JSON
としてパースし、配列ならトップレベル要素ごと、オブジェクトならメッセージ配列らしきキー
(`messages` / `events` / `entries`)を走査して `extract_timestamp`/`extract_tokens` を適用する」
フォールバックを追加。実ファイルの形状確認(下記診断)より前に一般形で書いてよいが、
**フィクスチャは実機から採った実ファイル(匿名化済み)を使うこと**。

### 原因3(確定バグ): `buddy-tokens.json` の日付比較が UTC 基準

`read_desktop_tokens_today()` が `date != Utc::now().date_naive()` で当日判定している。
JST(UTC+9)では **毎朝 9:00 まで UTC 日付が前日**のため、Desktop 側がローカル日付で
書いていれば午前中は常に不一致 → トークンが無視される。

**修正**: `chrono::Local::now().date_naive()` と比較する(両様書き込みに備えるなら
UTC/Local どちらかに一致すれば採用でもよい)。

### 原因4(要実機確認): Desktop 連携パスの実在が未検証

`d978437` が追加した以下のパスは公式ドキュメントに存在せず、実在の裏取りが必要:

- `~/Library/Application Support/Claude/claude-code-sessions/**/local_*.json`
- `~/Library/Application Support/Claude/local-agent-mode-sessions/**/local_*.json`
- `~/Library/Application Support/Claude/buddy-tokens.json`

存在しない場合、Desktop 由来の使用量はどのソースにも乗らない。実機の診断結果(下記)を見て、
実在するパス・実際の JSON 形状に合わせてパターンとパーサを修正すること。
**推測でパスを増やさない。実機で確認できたものだけ実装する。**

### 原因5(前提未成立の可能性): hook 経路の成立条件

hook キャッシュが効くには全部揃う必要がある:

1. ダッシュボードの hook インストールボタンを押して `~/.claude/settings.json` に Stop hook が入っている
2. **ターミナルの Claude Code** でターンを 1 回完了している(hook は Stop 時のみ発火)
3. Claude Code が hook stdin に `rate_limits` を含むバージョン(2.1.80+)である
4. キャッシュが 30 分以内

Claude Desktop のエージェントモードが `~/.claude/settings.json` の Stop hook を
実行するかは**未確認**。Desktop 主体の利用では hook 経路は当てにしない
(→ 原因1の Keychain 修正で OAuth 経路を復活させるのが本命)。

## 実機診断(ユーザーの Mac で実行して結果を貼る)

```sh
# 1) 資格情報: ファイルは無く Keychain にあるはず
ls -la ~/.claude/.credentials.json 2>&1
security find-generic-password -s "Claude Code-credentials" -w 2>&1 | cut -c1-60

# 2) CLI のローカルログ(localEstimate の材料)
ls -lt ~/.claude/projects/*/ 2>/dev/null | head -5

# 3) Desktop 側に何が実在するか
ls "$HOME/Library/Application Support/Claude/" 2>&1
ls "$HOME/Library/Application Support/Claude/buddy-tokens.json" 2>&1
find "$HOME/Library/Application Support/Claude" -name "*.json" -newer "$HOME/Library" -maxdepth 3 2>/dev/null | head -20

# 4) hook の状態
grep -o 'claude_rate_limits_hook[^"]*' ~/.claude/settings.json 2>&1
ls -la "$HOME/Library/Application Support/agent-usage/" 2>&1
cat "$HOME/Library/Application Support/agent-usage/claude_rate_limits.json" 2>/dev/null | cut -c1-200

# 5) Claude Code のバージョン(hook の rate_limits は 2.1.80+)
claude --version 2>&1
```

## 修正の優先順位

| 優先 | 項目 | 効果 |
|---|---|---|
| 1 | Keychain フォールバック(原因1) | macOS で OAuth 経路が復活し、公式 % が出る。**これだけで症状はほぼ解消するはず** |
| 2 | UTC 日付バグ(原因3) | 既存 Desktop トークン集計の正しさ |
| 3 | JSON/JSONL パーサフォールバック(原因2) | Desktop セッションのローカル推定 |
| 4 | 診断結果を見てパス修正(原因4) | Desktop 連携の実効化 |

## 回帰防止

- `creds.rs`: Keychain フォールバックのユニットテスト(fake `security`)。
- `jsonl.rs`: 整形 JSON(複数行)フィクスチャで 0 イベントにならないテスト。
- `claude.rs`: `read_desktop_tokens_today` をタイムゾーン注入可能にし、
  「JST 08:00(=UTC 前日 23:00)にローカル日付ファイルが有効」を検証するテスト。
- 診断を容易にするため、`snapshot()` の各段が None になった理由を `eprintln!` ではなく
  snapshot に載せる件は `docs/DESIGN.md` §4-5(FetchAttempt)で対応予定。
  今回の修正で先行して簡易版(直近の失敗理由文字列を Tauri コマンドで返す)を入れてもよい。
