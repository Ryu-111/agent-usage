pub mod core;
pub mod providers;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use core::history::HistoryStore;
use core::model::{Agent, AgentUsage, AppSnapshot, SnapshotSource, UsageSnapshot, UsageWindow};
use core::scheduler::{Scheduler, SchedulerConfig};
use providers::claude::ClaudeProvider;
use providers::codex::CodexProvider;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    latest: Arc<RwLock<Option<AppSnapshot>>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeHookStatus {
    installed: bool,
    cache_exists: bool,
    cache_fresh: bool,
    cache_path: Option<String>,
    desktop_tokens_exists: bool,
    desktop_tokens_fresh: bool,
    desktop_tokens_path: Option<String>,
    desktop_tokens_date: Option<String>,
    desktop_tokens_modified_at: Option<String>,
    desktop_bridge_enabled: bool,
    web_cookie_configured: bool,
}

impl AppState {
    fn new() -> Self {
        Self {
            latest: Arc::new(RwLock::new(None)),
        }
    }
}

#[tauri::command]
async fn get_usage_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<Option<AppSnapshot>, String> {
    Ok(state.latest.read().await.clone())
}

#[tauri::command]
async fn refresh_usage<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<AppSnapshot, String> {
    let previous = state.latest.read().await.clone();
    if let Some(snapshot) = &previous {
        if chrono::Utc::now() - snapshot.captured_at < chrono::Duration::seconds(120) {
            return Ok(snapshot.clone());
        }
    }

    let snapshot = collect_snapshot(previous.as_ref()).await;
    *state.latest.write().await = Some(snapshot.clone());
    let _ = app.emit("usage://snapshot", &snapshot);
    Ok(snapshot)
}

#[tauri::command]
async fn install_claude_rate_limits_hook() -> Result<String, String> {
    let settings_path = providers::claude_hook::default_claude_settings_path()
        .ok_or_else(|| "Could not resolve Claude settings path".to_string())?;
    let script_path = providers::claude_hook::hook_script_path()
        .ok_or_else(|| "Could not resolve hook script path".to_string())?;
    let cache_dir = providers::claude_hook::cache_file_path()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
        .ok_or_else(|| "Could not resolve hook cache directory".to_string())?;

    match providers::claude_hook::install_claude_hook(&settings_path, &script_path, &cache_dir)
        .map_err(|err| err.to_string())?
    {
        providers::claude_hook::HookInstallStatus::Installed => Ok("installed".to_string()),
        providers::claude_hook::HookInstallStatus::AlreadyInstalled => {
            Ok("alreadyInstalled".to_string())
        }
    }
}

#[tauri::command]
async fn is_claude_rate_limits_hook_installed() -> Result<bool, String> {
    let settings_path = providers::claude_hook::default_claude_settings_path()
        .ok_or_else(|| "Could not resolve Claude settings path".to_string())?;
    providers::claude_hook::is_claude_hook_installed(&settings_path).map_err(|err| err.to_string())
}

#[tauri::command]
async fn get_claude_rate_limits_hook_status() -> Result<ClaudeHookStatus, String> {
    let installed = match providers::claude_hook::default_claude_settings_path() {
        Some(settings_path) => providers::claude_hook::is_claude_hook_installed(&settings_path)
            .map_err(|err| err.to_string())?,
        None => false,
    };
    let cache_path = providers::claude_hook::cache_file_path();
    let cache_exists = cache_path.as_ref().is_some_and(|path| path.exists());
    let cache_fresh = cache_path
        .as_ref()
        .map(|path| {
            providers::claude_hook::read_hook_cache(
                path,
                providers::claude_hook::HOOK_CACHE_MAX_AGE,
            )
            .map(|reading| reading.is_some())
        })
        .transpose()
        .map_err(|err| err.to_string())?
        .unwrap_or(false);
    let desktop_tokens_path = providers::claude::default_desktop_tokens_path();
    let desktop_tokens_exists = desktop_tokens_path
        .as_ref()
        .is_some_and(|path| path.exists());
    let desktop_tokens_fresh = desktop_tokens_path
        .as_ref()
        .map(|path| providers::claude::read_desktop_tokens_today(path).map(|event| event.is_some()))
        .transpose()
        .map_err(|err| err.to_string())?
        .unwrap_or(false);
    let desktop_tokens_date = desktop_tokens_path
        .as_deref()
        .map(read_desktop_tokens_date)
        .transpose()
        .map_err(|err| err.to_string())?
        .flatten();
    let desktop_tokens_modified_at = desktop_tokens_path
        .as_deref()
        .map(modified_at_rfc3339)
        .transpose()
        .map_err(|err| err.to_string())?
        .flatten();
    let desktop_bridge_enabled = is_claude_desktop_bridge_enabled().unwrap_or(false);
    let web_cookie_configured = providers::claude_secrets::read_web_cookie()
        .map(|cookie| cookie.is_some())
        .unwrap_or(false);

    Ok(ClaudeHookStatus {
        installed,
        cache_exists,
        cache_fresh,
        cache_path: cache_path.map(|path| path.display().to_string()),
        desktop_tokens_exists,
        desktop_tokens_fresh,
        desktop_tokens_path: desktop_tokens_path.map(|path| path.display().to_string()),
        desktop_tokens_date,
        desktop_tokens_modified_at,
        desktop_bridge_enabled,
        web_cookie_configured,
    })
}

#[tauri::command]
async fn set_claude_web_cookie(cookie: String) -> Result<(), String> {
    let cookie = cookie.trim();
    if !cookie.contains("sessionKey=") || cookie.contains('\n') || cookie.contains('\r') {
        return Err("Cookie must contain sessionKey and cannot contain newlines".to_string());
    }
    providers::claude_secrets::write_web_cookie(cookie).map_err(|err| err.to_string())
}

#[tauri::command]
async fn clear_claude_web_cookie() -> Result<(), String> {
    providers::claude_secrets::clear_web_cookie().map_err(|err| err.to_string())
}

#[tauri::command]
async fn show_dashboard<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let window = match app.get_webview_window("main") {
        Some(window) => window,
        None => WebviewWindowBuilder::new(
            &app,
            "main",
            WebviewUrl::App("/src/dashboard/index.html".into()),
        )
        .title("Agent Usage")
        .inner_size(1080.0, 760.0)
        .min_inner_size(860.0, 620.0)
        .visible(false)
        .build()
        .map_err(|err| err.to_string())?,
    };
    window.show().map_err(|err| err.to_string())?;
    window.set_focus().map_err(|err| err.to_string())?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            get_usage_snapshot,
            install_claude_rate_limits_hook,
            get_claude_rate_limits_hook_status,
            is_claude_rate_limits_hook_installed,
            set_claude_web_cookie,
            clear_claude_web_cookie,
            refresh_usage,
            show_dashboard
        ])
        .setup(|app| {
            let state = app.state::<AppState>().inner().clone();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = bootstrap_scheduler(handle, state).await;
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run agent-usage");
}

async fn bootstrap_scheduler<R: Runtime>(app: AppHandle<R>, state: AppState) -> anyhow::Result<()> {
    let store = HistoryStore::open_default().context("open usage history store")?;
    let interval_secs = std::env::var("AGENT_USAGE_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(300);
    let scheduler = Scheduler::new(SchedulerConfig {
        interval: Duration::from_secs(interval_secs),
    });

    scheduler
        .run(move || {
            let app = app.clone();
            let state = state.clone();
            let store = store.clone();
            async move {
                let previous = state.latest.read().await.clone();
                let snapshot = collect_snapshot(previous.as_ref()).await;
                // Always publish so the UI shows when usage was last checked;
                // only write history rows when the usage itself changed.
                let changed = !previous
                    .as_ref()
                    .is_some_and(|previous| snapshots_equivalent(previous, &snapshot));
                *state.latest.write().await = Some(snapshot.clone());
                let _ = app.emit("usage://snapshot", &snapshot);
                if changed {
                    store.insert_app_snapshot(&snapshot)?;
                }
                Ok(())
            }
        })
        .await;

    Ok(())
}

async fn collect_snapshot(previous: Option<&AppSnapshot>) -> AppSnapshot {
    let claude = ClaudeProvider::default().snapshot().await;
    let codex = CodexProvider::default().snapshot().await;
    AppSnapshot {
        captured_at: chrono::Utc::now(),
        agents: vec![
            agent_usage_or_fallback(Agent::ClaudeCode, claude, previous),
            agent_usage_or_fallback(Agent::Codex, codex, previous),
        ],
    }
}

/// One provider failing must not drop the other agent's data, so a failed
/// provider keeps its previous windows (or reports unavailable on first run).
fn agent_usage_or_fallback(
    agent: Agent,
    result: anyhow::Result<Vec<UsageSnapshot>>,
    previous: Option<&AppSnapshot>,
) -> AgentUsage {
    let windows = match result {
        Ok(windows) => windows,
        Err(err) => {
            eprintln!("{agent:?} usage refresh failed: {err:#}");
            previous
                .and_then(|snapshot| snapshot.agents.iter().find(|usage| usage.agent == agent))
                .map(|usage| usage.windows.clone())
                .unwrap_or_else(|| unavailable_windows(agent))
        }
    };
    AgentUsage { agent, windows }
}

fn unavailable_windows(agent: Agent) -> Vec<UsageSnapshot> {
    [UsageWindow::FiveHour, UsageWindow::Weekly]
        .into_iter()
        .map(|window| UsageSnapshot {
            agent,
            window,
            utilization_pct: None,
            used_tokens: None,
            burn_rate_tokens_per_min: None,
            reset_at: None,
            limit_reached_at: None,
            observed_at: None,
            source: SnapshotSource::Unavailable,
        })
        .collect()
}

fn snapshots_equivalent(left: &AppSnapshot, right: &AppSnapshot) -> bool {
    normalize_agents_for_compare(&left.agents) == normalize_agents_for_compare(&right.agents)
}

fn normalize_agents_for_compare(agents: &[AgentUsage]) -> Vec<AgentUsage> {
    let mut agents = agents.to_vec();
    for agent in &mut agents {
        for window in &mut agent.windows {
            window.observed_at = None;
        }
        agent
            .windows
            .sort_by_key(|window| (window.agent, window.window));
    }
    agents.sort_by_key(|agent| agent.agent);
    agents
}

fn is_claude_desktop_bridge_enabled() -> anyhow::Result<bool> {
    let Some(home) = dirs::home_dir() else {
        return Ok(false);
    };
    let path = home.join("Library/Application Support/Claude/bridge-state.json");
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let value: serde_json::Value = serde_json::from_str(&content)?;
    Ok(value.as_object().is_some_and(|sessions| {
        sessions.values().any(|session| {
            session
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
    }))
}

fn read_desktop_tokens_date(path: &Path) -> anyhow::Result<Option<String>> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let value: Value = serde_json::from_str(&content)?;
    Ok(value
        .pointer("/tokens-today/date")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned))
}

fn modified_at_rfc3339(path: &Path) -> anyhow::Result<Option<String>> {
    let modified_at = match std::fs::metadata(path).and_then(|metadata| metadata.modified()) {
        Ok(modified_at) => modified_at,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let modified_at: chrono::DateTime<chrono::Utc> = modified_at.into();
    Ok(Some(modified_at.to_rfc3339()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_with(agent: Agent, source: SnapshotSource) -> AppSnapshot {
        let mut windows = unavailable_windows(agent);
        for window in &mut windows {
            window.source = source;
            window.used_tokens = Some(42);
        }
        AppSnapshot {
            captured_at: chrono::Utc::now(),
            agents: vec![AgentUsage { agent, windows }],
        }
    }

    #[test]
    fn failed_provider_keeps_previous_windows() {
        let previous = snapshot_with(Agent::ClaudeCode, SnapshotSource::Official);
        let usage = agent_usage_or_fallback(
            Agent::ClaudeCode,
            Err(anyhow::anyhow!("boom")),
            Some(&previous),
        );
        assert_eq!(usage.windows, previous.agents[0].windows);
    }

    #[test]
    fn failed_provider_without_history_is_unavailable() {
        let usage = agent_usage_or_fallback(Agent::Codex, Err(anyhow::anyhow!("boom")), None);
        assert_eq!(usage.windows.len(), 2);
        assert!(usage
            .windows
            .iter()
            .all(|window| window.source == SnapshotSource::Unavailable
                && window.agent == Agent::Codex));
    }

    #[test]
    fn successful_provider_uses_fresh_windows() {
        let previous = snapshot_with(Agent::Codex, SnapshotSource::Official);
        let fresh = unavailable_windows(Agent::Codex);
        let usage = agent_usage_or_fallback(Agent::Codex, Ok(fresh.clone()), Some(&previous));
        assert_eq!(usage.windows, fresh);
    }
}
