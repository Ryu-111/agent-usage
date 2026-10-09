use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClaudeCliUsage {
    pub five_hour_used_percent: Option<f64>,
    pub weekly_used_percent: Option<f64>,
}

pub async fn fetch_usage(
    binary: Option<PathBuf>,
    timeout: Duration,
) -> anyhow::Result<ClaudeCliUsage> {
    let binary = binary.ok_or_else(|| anyhow!("claude CLI was not found"))?;
    tokio::task::spawn_blocking(move || fetch_usage_blocking(&binary, timeout))
        .await
        .context("Claude CLI probe task failed")?
}

fn fetch_usage_blocking(binary: &PathBuf, timeout: Duration) -> anyhow::Result<ClaudeCliUsage> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 50,
            cols: 160,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("open Claude CLI PTY")?;

    let mut command = CommandBuilder::new(binary);
    command.args(["--allowed-tools", ""]);
    command.cwd(std::env::temp_dir());
    let mut child = pair
        .slave
        .spawn_command(command)
        .context("spawn Claude CLI")?;
    drop(pair.slave);

    // Run the interaction in a closure so every exit path, including PTY
    // setup and write errors, kills the spawned CLI instead of leaking it.
    let result = (|| -> anyhow::Result<ClaudeCliUsage> {
        let mut writer = pair
            .master
            .take_writer()
            .context("open Claude CLI PTY writer")?;
        let reader = pair
            .master
            .try_clone_reader()
            .context("open Claude CLI PTY reader")?;
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || read_output(reader, sender));

        let mut output = String::new();
        let started = Instant::now();
        let mut sent_usage = false;
        let mut sent_trust = false;

        while started.elapsed() < timeout {
            while let Ok(chunk) = receiver.try_recv() {
                output.push_str(&String::from_utf8_lossy(&chunk));
            }

            let normalized = normalize(&output);
            if !sent_trust
                && (normalized.contains("doyoutrustthefilesinthisfolder")
                    || normalized.contains("readytocodehere")
                    || normalized.contains("pressentertocontinue"))
            {
                writer.write_all(b"y\r")?;
                writer.flush()?;
                sent_trust = true;
            }
            if !sent_usage
                && (normalized.contains("claude")
                    || normalized.contains("currentsession")
                    || sent_trust)
            {
                writer.write_all(b"/usage\r")?;
                writer.flush()?;
                sent_usage = true;
            }
            if sent_usage && normalized.contains("currentsession") {
                if let Some(usage) = parse_usage(&output) {
                    return Ok(usage);
                }
            }

            std::thread::sleep(Duration::from_millis(60));
        }
        Err(anyhow!(
            "Claude CLI /usage probe timed out or returned an unreadable panel"
        ))
    })();

    let _ = child.kill();
    let _ = child.wait();
    result
}

fn read_output(mut reader: Box<dyn Read + Send>, sender: mpsc::Sender<Vec<u8>>) {
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(size) => {
                if sender.send(buffer[..size].to_vec()).is_err() {
                    break;
                }
            }
        }
    }
}

fn normalize(value: &str) -> String {
    strip_ansi(value)
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn strip_ansi(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut escape = false;
    for character in value.chars() {
        if escape {
            if character.is_ascii_alphabetic() {
                escape = false;
            }
            continue;
        }
        if character == '\u{1b}' {
            escape = true;
            continue;
        }
        result.push(character);
    }
    result
}

fn parse_usage(value: &str) -> Option<ClaudeCliUsage> {
    let lines: Vec<String> = strip_ansi(value).lines().map(str::to_owned).collect();
    Some(ClaudeCliUsage {
        five_hour_used_percent: percent_near_label(&lines, &["Current session", "Session"]),
        weekly_used_percent: percent_near_label(
            &lines,
            &["Current week (all models)", "Current week", "Weekly"],
        ),
    })
    .filter(|usage| usage.five_hour_used_percent.is_some())
}

fn percent_near_label(lines: &[String], labels: &[&str]) -> Option<f64> {
    let index = lines.iter().position(|line| {
        let lower = line.to_ascii_lowercase();
        labels
            .iter()
            .any(|label| lower.contains(&label.to_ascii_lowercase()))
    })?;
    let end = (index + 12).min(lines.len());
    for line in &lines[index..end] {
        if let Some((percent, is_left)) = parse_percent(line) {
            return Some(if is_left { 100.0 - percent } else { percent });
        }
    }
    None
}

fn parse_percent(line: &str) -> Option<(f64, bool)> {
    let percent_index = line.find('%')?;
    let bytes = line.as_bytes();
    let mut start = percent_index;
    while start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.') {
        start -= 1;
    }
    let value = line[start..percent_index].trim().parse::<f64>().ok()?;
    if !(0.0..=100.0).contains(&value) {
        return None;
    }
    Some((value, line.to_ascii_lowercase().contains("left")))
}

#[cfg(test)]
mod tests {
    use super::{parse_percent, parse_usage};

    #[test]
    fn parses_used_and_left_percentages() {
        let text = "Current session\n42% used\nCurrent week (all models)\n71% left";
        let usage = parse_usage(text).unwrap();
        assert_eq!(usage.five_hour_used_percent, Some(42.0));
        assert_eq!(usage.weekly_used_percent, Some(29.0));
    }

    #[test]
    fn rejects_out_of_range_percentages() {
        assert!(parse_percent("101% used").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fetches_usage_from_a_fake_claude_pty() {
        use std::os::unix::fs::PermissionsExt;

        let root =
            std::env::temp_dir().join(format!("agent-usage-claude-cli-{}", std::process::id()));
        let binary = root.join("claude");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &binary,
            "#!/bin/sh\nprintf 'Current session\\n42%% used\\nCurrent week (all models)\\n71%% left\\n'\nsleep 1\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&binary, permissions).unwrap();

        let usage = super::fetch_usage(Some(binary), std::time::Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(usage.five_hour_used_percent, Some(42.0));
        assert_eq!(usage.weekly_used_percent, Some(29.0));
        let _ = std::fs::remove_dir_all(root);
    }
}
