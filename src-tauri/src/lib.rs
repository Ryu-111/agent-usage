pub mod core;
pub mod providers;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use core::history::HistoryStore;
use core::model::{AgentUsage, AppSnapshot};
use core::scheduler::{Scheduler, SchedulerConfig};
use providers::claude::ClaudeProvider;
use providers::codex::CodexProvider;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
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
    if let Some(snapshot) = state.latest.read().await.clone() {
        if chrono::Utc::now() - snapshot.captured_at < chrono::Duration::seconds(30) {
            return Ok(snapshot);
        }
    }

    let snapshot = collect_snapshot().await.map_err(|err| err.to_string())?;
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

    Ok(ClaudeHookStatus {
        installed,
        cache_exists,
        cache_fresh,
        cache_path: cache_path.map(|path| path.display().to_string()),
        desktop_tokens_exists,
        desktop_tokens_fresh,
        desktop_tokens_path: desktop_tokens_path.map(|path| path.display().to_string()),
    })
}

#[tauri::command]
async fn show_dashboard<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|err| err.to_string())?;
        window.set_focus().map_err(|err| err.to_string())?;
    }
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
                let snapshot = collect_snapshot().await?;
                store.insert_app_snapshot(&snapshot)?;
                *state.latest.write().await = Some(snapshot.clone());
                let _ = app.emit("usage://snapshot", &snapshot);
                Ok(())
            }
        })
        .await;

    Ok(())
}

async fn collect_snapshot() -> anyhow::Result<AppSnapshot> {
    let claude = ClaudeProvider::default().snapshot().await?;
    let codex = CodexProvider::default().snapshot().await?;
    Ok(AppSnapshot {
        captured_at: chrono::Utc::now(),
        agents: vec![
            AgentUsage {
                agent: core::model::Agent::ClaudeCode,
                windows: claude,
            },
            AgentUsage {
                agent: core::model::Agent::Codex,
                windows: codex,
            },
        ],
    })
}
