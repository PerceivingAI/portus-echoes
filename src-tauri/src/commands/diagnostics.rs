use tauri::{AppHandle, State};

use crate::app_state::AppState;
use crate::diagnostics::{DiagnosticsRunError, DiagnosticsStorageData};

/// Get the cached Diagnostics storage state loaded at startup or updated by the
/// most recent successful explicit run.
#[tauri::command]
pub fn get_diagnostics_state(state: State<'_, AppState>) -> DiagnosticsStorageData {
    state.diagnostics_snapshot()
}

/// Run the on-demand Diagnostics suite using one immutable input snapshot.
/// A completed result becomes authoritative only after durable persistence.
#[tauri::command]
pub async fn run_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<DiagnosticsStorageData, DiagnosticsRunError> {
    let _permit = state.diagnostics_run_guard().try_acquire()?;
    let local_vad_model_path = crate::transcription::local::resolve_bundled_vad_model(&app).ok();
    let inputs = state.capture_diagnostics_inputs(local_vad_model_path);

    let data =
        tauri::async_runtime::spawn_blocking(move || crate::diagnostics::run_on_demand(inputs))
            .await
            .map_err(|_| DiagnosticsRunError::WorkerFailed)??;

    state.commit_diagnostics(data.clone())?;
    Ok(data)
}
