//! Application-level provider switching side effects shared by IPC and tray.

use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::AppState;
use crate::models::{AppSettings, ProviderId, UserErrorCode};

use super::{local_models, tray};

pub(crate) fn select_active_provider(
    app: &AppHandle,
    state: &AppState,
    provider: ProviderId,
) -> Result<AppSettings, UserErrorCode> {
    crate::recording::abort_active_recording(app);
    let (settings, local_runtime_intent) = state.persist_active_provider(provider)?;
    if provider == ProviderId::Local {
        crate::transcription::cloud::completed::close_completed_file_client();
    }
    if let Some(engine) = app.try_state::<crate::transcription::local::LocalEngine>() {
        local_models::apply_runtime_intent(Some(app), local_runtime_intent, &engine);
    }

    let _ = app.emit("settings:active-provider-changed", provider);
    // Provider persistence and runtime side effects are authoritative. A tray
    // redraw failure must not roll back or report failure for the user's
    // already-applied provider choice; a later refresh can reconcile the
    // checkmark from persisted AppState.
    let _ = tray::refresh_provider_menu(app);

    Ok(settings)
}

#[cfg(test)]
mod tests {
    #[test]
    fn provider_switch_source_preserves_committed_side_effect_order() {
        let source = include_str!("providers.rs");
        let aborted = source
            .find("crate::recording::abort_active_recording(app);")
            .expect("active recording must be aborted before provider switch");
        let persisted = source
            .find("state.persist_active_provider(provider)?")
            .expect("provider selection must persist first");
        let event = source
            .find("app.emit(\"settings:active-provider-changed\", provider)")
            .expect("committed provider selection must notify Settings");
        let tray = source
            .find("let _ = tray::refresh_provider_menu(app);")
            .expect("tray refresh must remain best-effort");
        let returned = source
            .find("Ok(settings)")
            .expect("provider selection must return committed settings");
        assert!(aborted < persisted);
        assert!(persisted < event);
        assert!(event < tray);
        assert!(tray < returned);
        assert!(
            source[persisted..event].contains("close_completed_file_client()"),
            "entering Local must clear the completed-file client before frontend notification"
        );
        assert!(
            source[persisted..event].contains("apply_runtime_intent"),
            "provider runtime intent must be applied before frontend notification"
        );
    }
}
