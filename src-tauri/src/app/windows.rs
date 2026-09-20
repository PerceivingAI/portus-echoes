//! Application window geometry and recording-overlay behavior.

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition};

use crate::models;

/// Overlay geometry, per `docs/OVERLAY.md`.
const OVERLAY_WIDTH: i32 = 180;
const OVERLAY_HEIGHT: i32 = 48;
const OVERLAY_BOTTOM_MARGIN: i32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

const TRAY_QUADRANT_MARGIN_X: i32 = 80;
const TRAY_QUADRANT_MARGIN_Y: i32 = 80;

#[derive(Default)]
pub(crate) struct SettingsNavigationOwner {
    current: parking_lot::Mutex<Option<models::SettingsNavigationState>>,
}

impl SettingsNavigationOwner {
    fn request(&self, tab: models::SettingsNavigationTab) -> models::SettingsNavigationState {
        let mut current = self.current.lock();
        let revision = current
            .map(|snapshot| snapshot.revision)
            .unwrap_or(0)
            .saturating_add(1);
        let snapshot = models::SettingsNavigationState { tab, revision };
        *current = Some(snapshot);
        snapshot
    }

    pub(crate) fn snapshot(&self) -> Option<models::SettingsNavigationState> {
        *self.current.lock()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScreenQuadrant {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl ScreenQuadrant {
    fn from_rects(tray: PixelRect, monitor: PixelRect) -> Self {
        let tray_center_x = i64::from(tray.x) + i64::from(tray.width) / 2;
        let tray_center_y = i64::from(tray.y) + i64::from(tray.height) / 2;
        let monitor_center_x = i64::from(monitor.x) + i64::from(monitor.width) / 2;
        let monitor_center_y = i64::from(monitor.y) + i64::from(monitor.height) / 2;

        match (
            tray_center_x >= monitor_center_x,
            tray_center_y >= monitor_center_y,
        ) {
            (false, false) => ScreenQuadrant::TopLeft,
            (true, false) => ScreenQuadrant::TopRight,
            (false, true) => ScreenQuadrant::BottomLeft,
            (true, true) => ScreenQuadrant::BottomRight,
        }
    }
}

fn tray_quadrant_position(
    tray: PixelRect,
    monitor: PixelRect,
    work_area: PixelRect,
    window_width: u32,
    window_height: u32,
    margin_x: i32,
    margin_y: i32,
) -> PhysicalPosition<i32> {
    let quadrant = ScreenQuadrant::from_rects(tray, monitor);

    let (x, y) = match quadrant {
        ScreenQuadrant::TopLeft => (work_area.x + margin_x, work_area.y + margin_y),
        ScreenQuadrant::TopRight => (
            work_area.x + work_area.width as i32 - window_width as i32 - margin_x,
            work_area.y + margin_y,
        ),
        ScreenQuadrant::BottomLeft => (
            work_area.x + margin_x,
            work_area.y + work_area.height as i32 - window_height as i32 - margin_y,
        ),
        ScreenQuadrant::BottomRight => (
            work_area.x + work_area.width as i32 - window_width as i32 - margin_x,
            work_area.y + work_area.height as i32 - window_height as i32 - margin_y,
        ),
    };

    let max_x = (work_area.x + work_area.width as i32 - window_width as i32).max(work_area.x);
    let max_y = (work_area.y + work_area.height as i32 - window_height as i32).max(work_area.y);

    PhysicalPosition::new(x.clamp(work_area.x, max_x), y.clamp(work_area.y, max_y))
}

pub(crate) fn show_centered_window(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.center();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn position_window_in_tray_quadrant(app: &AppHandle, window: &tauri::WebviewWindow) -> bool {
    let Some(tray_rect) = app
        .tray_by_id("main")
        .and_then(|tray| tray.rect().ok().flatten())
    else {
        return false;
    };

    let scale_factor = window.scale_factor().unwrap_or(1.0);
    let tray_position = tray_rect.position.to_physical::<i32>(scale_factor);
    let tray_size = tray_rect.size.to_physical::<u32>(scale_factor);
    let tray_center_x = f64::from(tray_position.x) + f64::from(tray_size.width) / 2.0;
    let tray_center_y = f64::from(tray_position.y) + f64::from(tray_size.height) / 2.0;

    let Ok(Some(monitor)) = window.monitor_from_point(tray_center_x, tray_center_y) else {
        return false;
    };
    let Ok(window_size) = window.outer_size() else {
        return false;
    };
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let work_area = monitor.work_area();
    let target = tray_quadrant_position(
        PixelRect {
            x: tray_position.x,
            y: tray_position.y,
            width: tray_size.width,
            height: tray_size.height,
        },
        PixelRect {
            x: monitor_position.x,
            y: monitor_position.y,
            width: monitor_size.width,
            height: monitor_size.height,
        },
        PixelRect {
            x: work_area.position.x,
            y: work_area.position.y,
            width: work_area.size.width,
            height: work_area.size.height,
        },
        window_size.width,
        window_size.height,
        TRAY_QUADRANT_MARGIN_X,
        TRAY_QUADRANT_MARGIN_Y,
    );

    window.set_position(target).is_ok()
}

pub(crate) fn request_settings_window(app: &AppHandle, tab: models::SettingsNavigationTab) {
    let snapshot = app.state::<SettingsNavigationOwner>().request(tab);
    show_window_from_tray(app, "onboarding");
    let _ = app.emit(models::SETTINGS_NAVIGATION_EVENT, snapshot);
}

pub(crate) fn show_window_from_tray(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        if !position_window_in_tray_quadrant(app, &window) {
            let _ = window.center();
        }
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn overlay_position(
    work_area: PixelRect,
    window_width: u32,
    window_height: u32,
    margin_bottom: i32,
) -> PhysicalPosition<i32> {
    let x = work_area.x + (work_area.width as i32 - window_width as i32) / 2;
    let y = work_area.y + work_area.height as i32 - window_height as i32 - margin_bottom;
    PhysicalPosition::new(x, y)
}

fn calculate_overlay_position(window: &tauri::WebviewWindow) -> Option<PhysicalPosition<i32>> {
    let monitor = window.primary_monitor().ok()??;
    let scale_factor = monitor.scale_factor();
    let work_area = monitor.work_area();
    let width = (f64::from(OVERLAY_WIDTH) * scale_factor).round() as u32;
    let height = (f64::from(OVERLAY_HEIGHT) * scale_factor).round() as u32;
    let margin_bottom = (f64::from(OVERLAY_BOTTOM_MARGIN) * scale_factor).round() as i32;

    Some(overlay_position(
        PixelRect {
            x: work_area.position.x,
            y: work_area.position.y,
            width: work_area.size.width,
            height: work_area.size.height,
        },
        width,
        height,
        margin_bottom,
    ))
}

fn show_overlay(app: &AppHandle) {
    let Some(window) = app.get_webview_window("overlay") else {
        return;
    };
    if !window.is_visible().unwrap_or(false) {
        if let Some(target) = calculate_overlay_position(&window) {
            let _ = window.set_position(target);
        }
        let _ = window.show();
        #[cfg(target_os = "windows")]
        let _ = window.set_ignore_cursor_events(true);
    }
}
pub(crate) fn hide_overlay(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.hide();
    }
}

fn recording_interaction_visible(phase: models::RecordingPhase) -> bool {
    matches!(
        phase,
        models::RecordingPhase::Preparing
            | models::RecordingPhase::Recording
            | models::RecordingPhase::Muted
    )
}

static OVERLAY_HIDE_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static SCHEDULED_HIDE_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(crate) fn schedule_overlay_hide(app: &AppHandle, duration: std::time::Duration) {
    let gen = OVERLAY_HIDE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    SCHEDULED_HIDE_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(duration).await;
        if OVERLAY_HIDE_GENERATION.load(std::sync::atomic::Ordering::SeqCst) == gen {
            SCHEDULED_HIDE_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
            hide_overlay(&app);
        }
    });
}

pub(crate) fn cancel_overlay_hide() {
    OVERLAY_HIDE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    SCHEDULED_HIDE_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
}

pub(crate) fn is_overlay_hide_scheduled() -> bool {
    SCHEDULED_HIDE_ACTIVE.load(std::sync::atomic::Ordering::SeqCst)
}

pub(crate) fn overlay_window_visible(
    phase: models::RecordingPhase,
    capability: crate::output::DeliveryCapability,
) -> bool {
    match capability {
        crate::output::DeliveryCapability::Clipboard => {
            recording_interaction_visible(phase) || phase == models::RecordingPhase::Finalizing
        }
        crate::output::DeliveryCapability::Inject => recording_interaction_visible(phase),
    }
}

pub(crate) fn sync_recording_ui_with_capability(
    app: &AppHandle,
    phase: models::RecordingPhase,
    capability: crate::output::DeliveryCapability,
) {
    let recording_visible = recording_interaction_visible(phase);
    let overlay_visible = overlay_window_visible(phase, capability);

    if overlay_visible {
        cancel_overlay_hide();
        show_overlay(app);
    } else if phase == models::RecordingPhase::Idle
        && capability == crate::output::DeliveryCapability::Clipboard
        && is_overlay_hide_scheduled()
    {
        // Preserve overlay visibility for the scheduled 2.0s duration so the user
        // sees "On Clipboard!" before the window automatically closes.
    } else {
        cancel_overlay_hide();
        hide_overlay(app);
    }
    super::tray::sync_recording_menu(app, recording_visible);
}
pub(crate) fn prepare_overlay(app: &AppHandle) {
    if let Some(overlay) = app.get_webview_window("overlay") {
        if let Some(pos) = calculate_overlay_position(&overlay) {
            let _ = overlay.set_position(pos);
        }
        #[cfg(target_os = "windows")]
        let _ = overlay.set_ignore_cursor_events(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_interaction_visibility_is_shared_and_excludes_finalizing() {
        assert!(!recording_interaction_visible(models::RecordingPhase::Idle));
        assert!(recording_interaction_visible(
            models::RecordingPhase::Preparing
        ));
        assert!(recording_interaction_visible(
            models::RecordingPhase::Recording
        ));
        assert!(recording_interaction_visible(
            models::RecordingPhase::Muted
        ));
        assert!(!recording_interaction_visible(
            models::RecordingPhase::Finalizing
        ));
    }
    #[test]
    fn overlay_window_visibility_respects_delivery_capability() {
        use crate::output::DeliveryCapability;

        // Inject mode hides immediately on finalizing
        assert!(!overlay_window_visible(models::RecordingPhase::Finalizing, DeliveryCapability::Inject));
        assert!(overlay_window_visible(models::RecordingPhase::Recording, DeliveryCapability::Inject));

        // Clipboard mode keeps overlay visible during finalizing
        assert!(overlay_window_visible(models::RecordingPhase::Finalizing, DeliveryCapability::Clipboard));
        assert!(overlay_window_visible(models::RecordingPhase::Recording, DeliveryCapability::Clipboard));
        assert!(!overlay_window_visible(models::RecordingPhase::Idle, DeliveryCapability::Clipboard));
    }
    #[test]
    fn settings_navigation_owner_keeps_latest_versioned_tray_request() {
        let owner = SettingsNavigationOwner::default();
        assert_eq!(owner.snapshot(), None);

        let settings = owner.request(models::SettingsNavigationTab::Settings);
        assert_eq!(
            settings,
            models::SettingsNavigationState {
                tab: models::SettingsNavigationTab::Settings,
                revision: 1,
            }
        );
        assert_eq!(owner.snapshot(), Some(settings));

        let diagnostics = owner.request(models::SettingsNavigationTab::Diagnostics);
        assert_eq!(
            diagnostics,
            models::SettingsNavigationState {
                tab: models::SettingsNavigationTab::Diagnostics,
                revision: 2,
            }
        );
        assert_eq!(owner.snapshot(), Some(diagnostics));
    }

    #[test]
    fn overlay_position_maintains_bottom_margin_across_dpi_scales() {
        let work_area = PixelRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1032, // 1080p with 48px taskbar
        };
        // 1.0x scale (180x48 window, 12px margin)
        let pos_100 = overlay_position(work_area, 180, 48, 12);
        assert_eq!(pos_100.x, (1920 - 180) / 2);
        assert_eq!(pos_100.y, 1032 - 48 - 12); // 972

        // 1.5x scale (270x72 window, 18px margin)
        let pos_150 = overlay_position(work_area, 270, 72, 18);
        assert_eq!(pos_150.x, (1920 - 270) / 2);
        assert_eq!(pos_150.y, 1032 - 72 - 18); // 942
    }

    const MONITOR: PixelRect = PixelRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    const WORK_AREA: PixelRect = PixelRect {
        x: 0,
        y: 40,
        width: 1920,
        height: 1000,
    };

    #[test]
    fn detects_discrete_screen_quadrant_matching_tray_center() {
        assert_eq!(
            ScreenQuadrant::from_rects(
                PixelRect {
                    x: 8,
                    y: 8,
                    width: 24,
                    height: 24,
                },
                MONITOR,
            ),
            ScreenQuadrant::TopLeft
        );
        assert_eq!(
            ScreenQuadrant::from_rects(
                PixelRect {
                    x: 1888,
                    y: 8,
                    width: 24,
                    height: 24,
                },
                MONITOR,
            ),
            ScreenQuadrant::TopRight
        );
        assert_eq!(
            ScreenQuadrant::from_rects(
                PixelRect {
                    x: 8,
                    y: 1048,
                    width: 24,
                    height: 24,
                },
                MONITOR,
            ),
            ScreenQuadrant::BottomLeft
        );
        assert_eq!(
            ScreenQuadrant::from_rects(
                PixelRect {
                    x: 1888,
                    y: 1048,
                    width: 24,
                    height: 24,
                },
                MONITOR,
            ),
            ScreenQuadrant::BottomRight
        );
    }

    #[test]
    fn maps_each_tray_quadrant_to_fixed_screen_edge_margins() {
        let cases = [
            (
                PixelRect {
                    x: 8,
                    y: 8,
                    width: 24,
                    height: 24,
                },
                PhysicalPosition::new(
                    WORK_AREA.x + TRAY_QUADRANT_MARGIN_X,
                    WORK_AREA.y + TRAY_QUADRANT_MARGIN_Y,
                ),
            ),
            (
                PixelRect {
                    x: 1888,
                    y: 8,
                    width: 24,
                    height: 24,
                },
                PhysicalPosition::new(
                    WORK_AREA.x + WORK_AREA.width as i32 - 650 - TRAY_QUADRANT_MARGIN_X,
                    WORK_AREA.y + TRAY_QUADRANT_MARGIN_Y,
                ),
            ),
            (
                PixelRect {
                    x: 8,
                    y: 1048,
                    width: 24,
                    height: 24,
                },
                PhysicalPosition::new(
                    WORK_AREA.x + TRAY_QUADRANT_MARGIN_X,
                    WORK_AREA.y + WORK_AREA.height as i32 - 450 - TRAY_QUADRANT_MARGIN_Y,
                ),
            ),
            (
                PixelRect {
                    x: 1888,
                    y: 1048,
                    width: 24,
                    height: 24,
                },
                PhysicalPosition::new(
                    WORK_AREA.x + WORK_AREA.width as i32 - 650 - TRAY_QUADRANT_MARGIN_X,
                    WORK_AREA.y + WORK_AREA.height as i32 - 450 - TRAY_QUADRANT_MARGIN_Y,
                ),
            ),
        ];

        for (tray, expected) in cases {
            assert_eq!(
                tray_quadrant_position(
                    tray,
                    MONITOR,
                    WORK_AREA,
                    650,
                    450,
                    TRAY_QUADRANT_MARGIN_X,
                    TRAY_QUADRANT_MARGIN_Y,
                ),
                expected
            );
        }
    }

    #[test]
    fn uses_the_tray_monitor_with_negative_desktop_coordinates() {
        let monitor = PixelRect {
            x: -1920,
            ..MONITOR
        };
        let work_area = PixelRect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1040,
        };
        let tray = PixelRect {
            x: -32,
            y: 1048,
            width: 24,
            height: 24,
        };

        assert_eq!(
            tray_quadrant_position(
                tray,
                monitor,
                work_area,
                650,
                450,
                TRAY_QUADRANT_MARGIN_X,
                TRAY_QUADRANT_MARGIN_Y,
            ),
            PhysicalPosition::new(
                work_area.x + work_area.width as i32 - 650 - TRAY_QUADRANT_MARGIN_X,
                work_area.y + work_area.height as i32 - 450 - TRAY_QUADRANT_MARGIN_Y,
            )
        );
    }

    #[test]
    fn keeps_an_oversized_window_at_the_work_area_origin() {
        let small_work_area = PixelRect {
            x: 100,
            y: 200,
            width: 500,
            height: 300,
        };

        assert_eq!(
            tray_quadrant_position(
                PixelRect {
                    x: 580,
                    y: 480,
                    width: 20,
                    height: 20,
                },
                PixelRect {
                    x: 100,
                    y: 200,
                    width: 500,
                    height: 300,
                },
                small_work_area,
                650,
                450,
                TRAY_QUADRANT_MARGIN_X,
                TRAY_QUADRANT_MARGIN_Y,
            ),
            PhysicalPosition::new(100, 200)
        );
    }
}
