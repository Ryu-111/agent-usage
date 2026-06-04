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
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    latest: Arc<RwLock<Option<AppSnapshot>>>,
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
    let snapshot = collect_snapshot().await.map_err(|err| err.to_string())?;
    *state.latest.write().await = Some(snapshot.clone());
    let _ = app.emit("usage://snapshot", &snapshot);
    Ok(snapshot)
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
    let scheduler = Scheduler::new(SchedulerConfig {
        interval: Duration::from_secs(300),
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
