//! Tray menu construction, provider state, and menu-event dispatch.

use parking_lot::Mutex;
use tauri::menu::{
    CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItem, MenuItemBuilder,
    PredefinedMenuItem, SubmenuBuilder,
};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::app_state::AppState;
use crate::{models, recording};

use super::{emit_error_code, windows, APP_ERROR_EVENT};

const PROVIDER_LOCAL_ID: &str = "provider-local";
const PROVIDER_OPENAI_ID: &str = "provider-openai";
const PROVIDER_GROQ_ID: &str = "provider-groq";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProviderMenuEntry {
    provider: models::ProviderId,
    id: &'static str,
    label: &'static str,
    checked: bool,
}

fn provider_menu_entries(active_provider: Option<models::ProviderId>) -> [ProviderMenuEntry; 3] {
    [
        (models::ProviderId::Local, PROVIDER_LOCAL_ID, "Local"),
        (models::ProviderId::Openai, PROVIDER_OPENAI_ID, "OpenAI"),
        (models::ProviderId::Groq, PROVIDER_GROQ_ID, "Groq"),
    ]
    .map(|(provider, id, label)| ProviderMenuEntry {
        provider,
        id,
        label,
        checked: active_provider == Some(provider),
    })
}

struct ProviderMenuState {
    items: [(models::ProviderId, CheckMenuItem<tauri::Wry>); 3],
    refresh_lock: Mutex<()>,
}

struct RecordingMenuState {
    item: MenuItem<tauri::Wry>,
}

pub(crate) fn refresh_provider_menu(app: &AppHandle) -> Result<(), models::UserErrorCode> {
    let Some(provider_menu) = app.try_state::<ProviderMenuState>() else {
        return Ok(());
    };
    let _refresh = provider_menu.refresh_lock.lock();
    let active_provider = app.state::<AppState>().settings_snapshot().active_provider;
    for (provider, item) in &provider_menu.items {
        item.set_checked(active_provider == Some(*provider))
            .map_err(|_| models::UserErrorCode::Unexpected)?;
    }
    Ok(())
}

pub(crate) fn sync_recording_menu(app: &AppHandle, recording_visible: bool) {
    if let Some(menu) = app.try_state::<RecordingMenuState>() {
        let label = if recording_visible {
            "Stop Recording"
        } else {
            "Start Recording"
        };
        let _ = menu.item.set_text(label);
    }
}

fn select_tray_provider(app: &AppHandle, provider: models::ProviderId) {
    if let Err(code) =
        super::providers::select_active_provider(app, &app.state::<AppState>(), provider)
    {
        emit_error_code(app, APP_ERROR_EVENT, code);
    }
}

pub(crate) fn setup(app: &mut tauri::App) -> tauri::Result<()> {
    let record_item =
        MenuItemBuilder::with_id("start-recording", "Start Recording").build(&*app)?;
    let active_provider = app.state::<AppState>().settings_snapshot().active_provider;
    let provider_entries = provider_menu_entries(active_provider);
    let provider_local_item =
        CheckMenuItemBuilder::with_id(provider_entries[0].id, provider_entries[0].label)
            .checked(provider_entries[0].checked)
            .build(&*app)?;
    let provider_openai_item =
        CheckMenuItemBuilder::with_id(provider_entries[1].id, provider_entries[1].label)
            .checked(provider_entries[1].checked)
            .build(&*app)?;
    let provider_groq_item =
        CheckMenuItemBuilder::with_id(provider_entries[2].id, provider_entries[2].label)
            .checked(provider_entries[2].checked)
            .build(&*app)?;
    let provider_submenu = SubmenuBuilder::with_id(&*app, "provider", "Provider")
        .items(&[
            &provider_local_item,
            &provider_openai_item,
            &provider_groq_item,
        ])
        .build()?;
    let settings_item = MenuItemBuilder::with_id("settings", "Settings").build(&*app)?;
    let diagnostics_item = MenuItemBuilder::with_id("diagnostics", "Diagnostics").build(&*app)?;
    let separator = PredefinedMenuItem::separator(&*app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(&*app)?;
    let menu = MenuBuilder::new(&*app)
        .items(&[
            &record_item,
            &separator,
            &provider_submenu,
            &settings_item,
            &diagnostics_item,
            &quit_item,
        ])
        .build()?;

    let icon = app
        .default_window_icon()
        .expect("bundle icons must be configured")
        .clone();

    let tray = TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("PortusEchoes")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "start-recording" => {
                let _ = recording::toggle_recording(app);
            }
            PROVIDER_LOCAL_ID => select_tray_provider(app, models::ProviderId::Local),
            PROVIDER_OPENAI_ID => select_tray_provider(app, models::ProviderId::Openai),
            PROVIDER_GROQ_ID => select_tray_provider(app, models::ProviderId::Groq),
            "settings" => {
                windows::request_settings_window(app, models::SettingsNavigationTab::Settings)
            }
            "diagnostics" => {
                windows::request_settings_window(app, models::SettingsNavigationTab::Diagnostics)
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(&*app)?;

    app.manage(ProviderMenuState {
        items: [
            (models::ProviderId::Local, provider_local_item),
            (models::ProviderId::Openai, provider_openai_item),
            (models::ProviderId::Groq, provider_groq_item),
        ],
        refresh_lock: Mutex::new(()),
    });
    app.manage(tray);
    app.manage(RecordingMenuState {
        item: record_item.clone(),
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_shows_all_providers_in_fixed_order() {
        assert_eq!(
            provider_menu_entries(None),
            [
                ProviderMenuEntry {
                    provider: models::ProviderId::Local,
                    id: PROVIDER_LOCAL_ID,
                    label: "Local",
                    checked: false,
                },
                ProviderMenuEntry {
                    provider: models::ProviderId::Openai,
                    id: PROVIDER_OPENAI_ID,
                    label: "OpenAI",
                    checked: false,
                },
                ProviderMenuEntry {
                    provider: models::ProviderId::Groq,
                    id: PROVIDER_GROQ_ID,
                    label: "Groq",
                    checked: false,
                },
            ]
        );
    }

    #[test]
    fn marks_the_active_provider_without_filtering_other_items() {
        let entries = provider_menu_entries(Some(models::ProviderId::Openai));
        assert_eq!(entries.len(), 3);
        assert!(!entries[0].checked);
        assert!(entries[1].checked);
        assert!(!entries[2].checked);
    }
}
