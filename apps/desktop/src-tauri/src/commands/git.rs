use crate::state::AppState;
use tauri::{AppHandle, Emitter, Manager, State};
use workspace_model::RepositorySnapshot;

#[tauri::command]
pub fn git_status(state: State<'_, AppState>) -> Result<RepositorySnapshot, String> {
    state.with_app(|app| Ok(app.ui.repository.clone()))
}

#[tauri::command]
pub async fn git_refresh(app: AppHandle) -> Result<RepositorySnapshot, String> {
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.git_refresh()
    })
    .await
    .map_err(|e| format!("Git refresh task failed: {e}"))?
}

#[tauri::command]
pub async fn git_stage(app: AppHandle, paths: Vec<String>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.git_stage(paths)
    })
    .await
    .map_err(|e| format!("Git stage task failed: {e}"))?
}

#[tauri::command]
pub async fn git_unstage(app: AppHandle, paths: Vec<String>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.git_unstage(paths)
    })
    .await
    .map_err(|e| format!("Git unstage task failed: {e}"))?
}

#[tauri::command]
pub async fn git_commit(app: AppHandle, message: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.git_commit(message)
    })
    .await
    .map_err(|e| format!("Git commit task failed: {e}"))?
}

#[tauri::command]
pub async fn git_generate_commit_message(app: AppHandle) -> Result<String, String> {
    let progress_app = app.clone();
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.with_app(|app| {
            app.generate_commit_message(&|message: &str| {
                let _ = progress_app.emit("commit:progress", message.to_string());
            })
        })
    })
    .await
    .map_err(|e| format!("Generate commit message task failed: {e}"))?
}

#[tauri::command]
pub async fn git_push(app: AppHandle) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.git_push()
    })
    .await
    .map_err(|e| format!("Git push task failed: {e}"))?
}

#[tauri::command]
pub async fn git_commit_and_push(app: AppHandle, message: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.git_commit_and_push(message)
    })
    .await
    .map_err(|e| format!("Git commit-and-push task failed: {e}"))?
}
