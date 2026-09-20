//! PortusEchoes backend entrypoint.
//!
//! Tray-first application shell: no persistent main window. All windows are
//! declared in `tauri.conf.json` and created hidden; they are shown on demand
//! and hidden (never destroyed) on close. The overlay renders the authoritative
//! versioned recording lifecycle. See `docs/STRUCTURE.md` for the module map.

#![windows_subsystem = "windows"]

mod app;
mod app_state;
mod audio;
mod cloud_catalog;
mod commands;
mod diagnostics;
mod hotkey;
mod keystore;
mod model_download;
mod models;
mod output;
mod recording;
mod settings;
mod transcription;

use tauri::{Emitter, Manager};

use app_state::AppState;

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    transcription::configure_native_logging();
    if let Some(exit_code) = transcription::local::run_internal_worker_from_process_args() {
        std::process::exit(exit_code);
    }
    if let Some(exit_code) = diagnostics::run_internal_probe_from_process_args() {
        std::process::exit(exit_code);
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::state::get_settings,
            commands::state::get_recording_state,
            commands::state::get_settings_navigation,
            commands::providers::select_active_provider,
            commands::providers::select_openai_model,
            commands::providers::select_openai_custom_model,
            commands::providers::set_openai_custom_model,
            commands::providers::select_groq_model,
            commands::providers::select_groq_custom_model,
            commands::providers::set_groq_custom_model,
            commands::providers::select_local_model,
            commands::providers::select_local_custom_model,
            commands::providers::set_local_custom_model_path,
            commands::hotkey::set_hotkey,
            commands::hotkey::focused_hotkey_key_event,
            commands::hotkey::set_hotkey_capture_active,
            commands::hotkey::reset_focused_hotkey_input,
            commands::providers::complete_onboarding,
            commands::credentials::set_api_key,
            commands::credentials::get_api_key,
            commands::credentials::delete_api_key,
            commands::credentials::get_provider_status,
            commands::hotkey::is_hotkey_registered,
            commands::hotkey::is_hotkey_active,
            commands::diagnostics::run_diagnostics,
            commands::diagnostics::get_diagnostics_state,
            commands::downloads::list_local_models,
            commands::downloads::download_model,
            commands::downloads::cancel_download,
            commands::downloads::delete_model,
        ])
        .setup(|app| {
            let settings_path =
                settings::default_config_path().expect("OS config directory must be resolvable");
            app.manage(AppState::init(settings_path.clone()));
            app.manage(crate::app::windows::SettingsNavigationOwner::default());

            // --- Model download manager (docs/DOWNLOADS.md) ---
            let models_dir = settings_path
                .parent()
                .expect("config path has a parent")
                .join("models");
            app.manage(std::sync::Arc::new(
                model_download::DownloadManager::new(
                    models_dir,
                    model_download::configured_download_paths(),
                )
                .expect("build-time Local model URLs must be valid"),
            ));

            // First run: show the onboarding wizard (docs/GUI.md). The flag
            // is the persistent gate — written on finish or skip-to-end.
            if !app
                .state::<AppState>()
                .settings_snapshot()
                .onboarding_complete
            {
                crate::app::windows::show_centered_window(app.handle(), "onboarding");
            }

            // --- Hotkey engine (docs/HOTKEY.md) ---
            let (hotkey_tx, hotkey_rx) = std::sync::mpsc::channel::<models::HotkeyEvent>();
            app.manage(hotkey::HotkeyEngine::new(hotkey_tx));
            let combo = app.state::<AppState>().hotkey();
            let _ = app
                .state::<hotkey::HotkeyEngine>()
                .start(app.handle(), &combo);
            #[cfg(target_os = "windows")]
            hotkey::install_focused_webview_bridge(app.handle())
                .expect("Settings WebView2 hotkey bridge must initialize");

            // --- Audio engine (docs/AUDIO.md): single owner of recording
            // start/stop. The injected callback commits the authoritative
            // lifecycle snapshot consumed by the overlay and tray.
            app.manage(recording::RecordingCoordinator::new());
            app.manage(recording::LocalProcessingRegistry::new());
            app.manage(recording::CloudProcessingRegistry::new());
            let (audio_engine, audio_rx) = audio::AudioEngine::new({
                let app = app.handle().clone();
                move |event| recording::handle_capture_event(&app, event)
            });
            app.manage(audio_engine);
            let (local_runtime_authority, initial_local_runtime_intent) = {
                let state = app.state::<AppState>();
                (
                    state.local_runtime_authority(),
                    state.local_runtime_intent(),
                )
            };
            let local_engine =
                transcription::local::LocalEngine::with_authority(local_runtime_authority);
            app.manage(local_engine.clone());
            crate::app::local_models::apply_runtime_intent(
                Some(app.handle()),
                Some(initial_local_runtime_intent),
                &local_engine,
            );

            // Completed Cloud buffers are admitted by exact recording ID and
            // dispatched as independent request-scoped jobs by the app shell.
            crate::app::cloud_jobs::start_receiver(app.handle().clone(), audio_rx);

            // Hotkey events drive the audio engine, which owns recording
            // start/stop and Cloud-buffer versus Local-live-feed ownership.
            let dispatch = app.handle().clone();
            std::thread::spawn(move || {
                while let Ok(event) = hotkey_rx.recv() {
                    match event {
                        models::HotkeyEvent::Pressed => {
                            let _ = recording::begin_recording(&dispatch);
                        }
                        models::HotkeyEvent::Released => {
                            recording::stop_recording(&dispatch);
                            dispatch
                                .state::<hotkey::HotkeyEngine>()
                                .acknowledge_release();
                        }
                    }
                }
            });

            // --- Tray menu: Start Recording, Provider, Settings, Diagnostics, Quit ---
            crate::app::tray::setup(app)?;

            // The overlay never intercepts input (docs/OVERLAY.md).
            crate::app::windows::prepare_overlay(app.handle());

            Ok(())
        })
        // Closing the Settings window first asks the frontend to flush dirty
        // fields and complete first-run onboarding. Other windows hide directly.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.label() == "onboarding" {
                    let engine = window.state::<hotkey::HotkeyEngine>();
                    engine.focused_input_lost();
                    engine.set_hotkey_capture_active(false);
                }
                if window.label() == "onboarding" {
                    let _ = window.emit("settings:close-requested", ());
                } else {
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building PortusEchoes");

    app.run(|handle, event| match event {
        tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => {
            handle.state::<hotkey::HotkeyEngine>().stop();
            recording::shutdown(handle);
            for (_, window) in handle.webview_windows() {
                let _ = window.destroy();
            }
        }
        _ => {}
    });
}
