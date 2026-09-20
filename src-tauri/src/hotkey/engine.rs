//! Active hotkey registration lifecycle and platform dispatch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::models::HotkeyEvent;

#[cfg(target_os = "windows")]
use super::combo::ComboState;
use super::combo::{parse_combo, HotkeyError, ParsedCombo};

pub(crate) struct HotkeyEngine {
    inner: Mutex<EngineState>,
    tx: Sender<HotkeyEvent>,
    capture_suppressed: Arc<AtomicBool>,
}

#[derive(Default)]
struct EngineState {
    active: Option<ActiveRegistration>,
    pending: Option<ActiveRegistration>,
}

struct ActiveRegistration {
    platform: PlatformRegistration,
    enabled: Arc<AtomicBool>,
    hold_in_flight: Arc<AtomicBool>,
    #[allow(dead_code)]
    sink: HotkeyEventSink,
    #[cfg(target_os = "windows")]
    focused_state: Mutex<ComboState>,
    #[cfg(target_os = "windows")]
    trigger_vk: u32,
    #[cfg(target_os = "windows")]
    required_mods: u8,
}

impl ActiveRegistration {
    fn disable(&self) {
        self.enabled.store(false, Ordering::SeqCst);
    }

    fn enable(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }

    fn stop_and_join(self) {
        self.disable();
        self.platform.stop_and_join();
    }
    fn reset_hold_state(&self) {
        self.hold_in_flight.store(false, Ordering::SeqCst);
        self.sink.pressed.store(false, Ordering::SeqCst);
    }

}

pub(crate) struct PreparedHotkey {
    registration: Option<ActiveRegistration>,
}

impl Drop for PreparedHotkey {
    fn drop(&mut self) {
        if let Some(registration) = self.registration.take() {
            registration.stop_and_join();
        }
    }
}

pub(super) struct PlatformRegistration {
    stop: Box<dyn FnOnce() + Send>,
    join: std::thread::JoinHandle<()>,
}

impl PlatformRegistration {
    pub(super) fn new(stop: Box<dyn FnOnce() + Send>, join: std::thread::JoinHandle<()>) -> Self {
        Self { stop, join }
    }

    fn stop_and_join(self) {
        (self.stop)();
        let _ = self.join.join();
    }
}

#[derive(Clone)]
pub(super) struct HotkeyEventSink {
    tx: Sender<HotkeyEvent>,
    enabled: Arc<AtomicBool>,
    hold_in_flight: Arc<AtomicBool>,
    pressed: Arc<AtomicBool>,
    capture_suppressed: Arc<AtomicBool>,
}

impl HotkeyEventSink {
    pub(super) fn send(&self, event: HotkeyEvent) -> bool {
        if !self.enabled.load(Ordering::SeqCst) || self.capture_suppressed.load(Ordering::SeqCst) {
            return false;
        }
        let accepted = match event {
            HotkeyEvent::Pressed => !self.pressed.swap(true, Ordering::SeqCst),
            HotkeyEvent::Released => self.pressed.swap(false, Ordering::SeqCst),
        };
        if !accepted {
            return false;
        }
        if event == HotkeyEvent::Pressed {
            self.hold_in_flight.store(true, Ordering::SeqCst);
        }
        self.tx.send(event).is_ok()
    }

    pub(super) fn force_release(&self) {
        self.hold_in_flight.store(false, Ordering::SeqCst);
        if self.pressed.swap(false, Ordering::SeqCst) {
            let _ = self.tx.send(HotkeyEvent::Released);
        }
    }

    pub(super) fn is_pressed(&self) -> bool {
        self.pressed.load(Ordering::SeqCst)
    }
}

impl HotkeyEngine {
    pub(crate) fn new(tx: Sender<HotkeyEvent>) -> Self {
        Self {
            inner: Mutex::new(EngineState::default()),
            tx,
            capture_suppressed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Startup/direct replacement path. Registration is prepared while muted,
    /// then atomically replaces the current runtime registration.
    pub(crate) fn start(&self, app: &tauri::AppHandle, combo: &str) -> Result<(), HotkeyError> {
        let prepared = self.prepare(app, combo)?;
        self.commit_immediate(prepared);
        Ok(())
    }

    /// Validate and establish a new global registration without allowing it to
    /// emit recording events yet. Dropping the returned value unregisters it.
    pub(crate) fn prepare(
        &self,
        app: &tauri::AppHandle,
        combo: &str,
    ) -> Result<PreparedHotkey, HotkeyError> {
        let parsed = parse_combo(combo)?;
        #[cfg(target_os = "linux")]
        {
            if !super::wayland_detected() {
                let stop_old = {
                    let mut state = self.inner.lock();
                    let hold_in_flight = state
                        .active
                        .as_ref()
                        .is_some_and(|active| active.hold_in_flight.load(Ordering::SeqCst));
                    if !hold_in_flight {
                        state.active.take()
                    } else {
                        None
                    }
                };
                if let Some(registration) = stop_old {
                    registration.stop_and_join();
                }
            }
        }
        let registration = self.build_registration(app, parsed)?;
        Ok(PreparedHotkey {
            registration: Some(registration),
        })
    }

    /// Commit a persisted shortcut replacement. If the currently active
    /// shortcut has a press whose release has not yet been consumed, keep the
    /// old registration live and the new registration muted until that release
    /// is acknowledged. Otherwise switch immediately.
    pub(crate) fn commit_prepared(&self, mut prepared: PreparedHotkey) {
        let registration = prepared
            .registration
            .take()
            .expect("prepared hotkey must own one registration");
        let (stop_active, stop_pending) = {
            let mut state = self.inner.lock();
            let hold_in_flight = state
                .active
                .as_ref()
                .is_some_and(|active| active.hold_in_flight.load(Ordering::SeqCst));
            if hold_in_flight {
                (None, state.pending.replace(registration))
            } else {
                if let Some(active) = state.active.as_ref() {
                    active.disable();
                }
                registration.enable();
                (state.active.replace(registration), state.pending.take())
            }
        };
        if let Some(registration) = stop_pending {
            registration.stop_and_join();
        }
        if let Some(registration) = stop_active {
            registration.stop_and_join();
        }
    }

    /// The dispatcher calls this only after the shared recording runtime has
    /// consumed a physical hotkey release. This is the handoff point for a
    /// shortcut replacement requested during that hold.
    pub(crate) fn acknowledge_release(&self) {
        let mut stop_active = None;
        {
            let mut state = self.inner.lock();
            if let Some(active) = state.active.as_ref() {
                active.hold_in_flight.store(false, Ordering::SeqCst);
            }
            if let Some(pending) = state.pending.take() {
                if let Some(active) = state.active.as_ref() {
                    active.disable();
                }
                pending.enable();
                stop_active = state.active.replace(pending);
            }
        }
        if let Some(registration) = stop_active {
            registration.stop_and_join();
        }
    }
    pub(crate) fn reset_hold_state(&self) {
        let mut stop_active = None;
        {
            let mut state = self.inner.lock();
            if let Some(active) = state.active.as_ref() {
                active.reset_hold_state();
            }
            if let Some(pending) = state.pending.take() {
                if let Some(active) = state.active.as_ref() {
                    active.disable();
                }
                pending.enable();
                stop_active = state.active.replace(pending);
            }
        }
        if let Some(registration) = stop_active {
            registration.stop_and_join();
        }
    }


    pub(crate) fn stop(&self) {
        let (active, pending) = {
            let mut state = self.inner.lock();
            if let Some(active) = state.active.as_ref() {
                active.disable();
            }
            if let Some(pending) = state.pending.as_ref() {
                pending.disable();
            }
            (state.active.take(), state.pending.take())
        };
        if let Some(registration) = pending {
            registration.stop_and_join();
        }
        if let Some(registration) = active {
            registration.stop_and_join();
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.inner
            .lock()
            .active
            .as_ref()
            .is_some_and(|active| active.enabled.load(Ordering::SeqCst))
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn focused_windows_key_event(
        &self,
        vk: u32,
        current_mods: u8,
        pressed: bool,
        repeat: bool,
    ) -> bool {
        self.focused_key_event(
            vk,
            super::windows::modifier_bit(vk),
            current_mods,
            pressed,
            repeat,
        )
    }

    pub(crate) fn focused_browser_key_event(
        &self,
        key: &str,
        current_mods: u8,
        pressed: bool,
        repeat: bool,
    ) -> bool {
        #[cfg(target_os = "windows")]
        {
            let mod_bit = super::windows::browser_modifier_bit(key);
            let vk = super::windows::browser_key_to_vk(key).unwrap_or(0);
            return self.focused_key_event(vk, mod_bit, current_mods, pressed, repeat);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (key, current_mods, pressed, repeat);
            false
        }
    }

    #[cfg(target_os = "windows")]
    fn focused_key_event(
        &self,
        vk: u32,
        mod_bit: u8,
        current_mods: u8,
        pressed: bool,
        repeat: bool,
    ) -> bool {
        if self.capture_suppressed.load(Ordering::SeqCst) {
            return false;
        }
        let state = self.inner.lock();
        let Some(active) = state
            .active
            .as_ref()
            .filter(|active| active.enabled.load(Ordering::SeqCst))
        else {
            return false;
        };
        let is_trigger = vk != 0 && vk == active.trigger_vk;
        let mut focused = active.focused_state.lock();
        let was_active = focused.is_active();
        let event = if pressed {
            if repeat {
                None
            } else {
                focused.on_key_down_with_mods(is_trigger, mod_bit, current_mods)
            }
        } else {
            focused.on_key_up_with_mods(is_trigger, mod_bit, current_mods)
        };
        let handled = is_trigger
            && if pressed {
                current_mods == active.required_mods
                    && (was_active || event == Some(HotkeyEvent::Pressed))
            } else {
                was_active
            };
        drop(focused);
        if let Some(event) = event {
            active.sink.send(event);
        }
        handled
    }

    pub(crate) fn set_hotkey_capture_active(&self, suppressed: bool) {
        if self.capture_suppressed.swap(suppressed, Ordering::SeqCst) == suppressed {
            return;
        }
        if suppressed {
            #[cfg(target_os = "windows")]
            let sink = self.inner.lock().active.as_ref().map(|active| {
                active.focused_state.lock().reset();
                active.sink.clone()
            });
            #[cfg(not(target_os = "windows"))]
            let sink = self
                .inner
                .lock()
                .active
                .as_ref()
                .map(|active| active.sink.clone());

            if let Some(sink) = sink {
                sink.force_release();
            }
        }
    }

    pub(crate) fn focused_input_lost(&self) {
        #[cfg(target_os = "windows")]
        {
            let sink = self.inner.lock().active.as_ref().and_then(|active| {
                active
                    .focused_state
                    .lock()
                    .reset()
                    .map(|_| active.sink.clone())
            });
            if let Some(sink) = sink {
                sink.send(HotkeyEvent::Released);
            }
        }
    }

    fn commit_immediate(&self, mut prepared: PreparedHotkey) {
        let registration = prepared
            .registration
            .take()
            .expect("prepared hotkey must own one registration");
        let (active, pending) = {
            let mut state = self.inner.lock();
            if let Some(active) = state.active.as_ref() {
                active.disable();
            }
            registration.enable();
            (state.active.replace(registration), state.pending.take())
        };
        if let Some(registration) = pending {
            registration.stop_and_join();
        }
        if let Some(registration) = active {
            registration.stop_and_join();
        }
    }

    fn build_registration(
        &self,
        app: &tauri::AppHandle,
        parsed: ParsedCombo,
    ) -> Result<ActiveRegistration, HotkeyError> {
        let enabled = Arc::new(AtomicBool::new(false));
        let hold_in_flight = Arc::new(AtomicBool::new(false));
        let sink = HotkeyEventSink {
            tx: self.tx.clone(),
            enabled: enabled.clone(),
            hold_in_flight: hold_in_flight.clone(),
            pressed: Arc::new(AtomicBool::new(false)),
            capture_suppressed: self.capture_suppressed.clone(),
        };

        #[cfg(target_os = "windows")]
        let trigger_vk =
            super::windows::key_to_vk(&parsed.key).ok_or(HotkeyError::UnsupportedKey)?;
        #[cfg(target_os = "windows")]
        let required_mods = parsed.required_mask();
        #[cfg(target_os = "windows")]
        let focused_state = Mutex::new(ComboState::new(&parsed));

        #[cfg(target_os = "windows")]
        let platform = {
            let _ = app;
            super::windows::start(parsed, sink.clone())?
        };
        #[cfg(target_os = "linux")]
        let platform = if super::wayland_detected() {
            super::wayland::start(app, parsed, sink.clone())?
        } else {
            super::x11::start(parsed, sink.clone())?
        };
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        let platform = {
            let _ = (app, parsed);
            return Err(HotkeyError::PlatformFailure);
        };

        Ok(ActiveRegistration {
            platform,
            enabled,
            hold_in_flight,
            sink,
            #[cfg(target_os = "windows")]
            focused_state,
            #[cfg(target_os = "windows")]
            trigger_vk,
            #[cfg(target_os = "windows")]
            required_mods,
        })
    }

    #[cfg(all(test, target_os = "windows"))]
    pub(super) fn start_windows_for_test(&self, parsed: ParsedCombo) -> Result<(), HotkeyError> {
        let enabled = Arc::new(AtomicBool::new(false));
        let hold_in_flight = Arc::new(AtomicBool::new(false));
        let sink = HotkeyEventSink {
            tx: self.tx.clone(),
            enabled: enabled.clone(),
            hold_in_flight: hold_in_flight.clone(),
            pressed: Arc::new(AtomicBool::new(false)),
            capture_suppressed: self.capture_suppressed.clone(),
        };
        let trigger_vk =
            super::windows::key_to_vk(&parsed.key).ok_or(HotkeyError::UnsupportedKey)?;
        let required_mods = parsed.required_mask();
        let focused_state = Mutex::new(ComboState::new(&parsed));
        let platform = super::windows::start(parsed, sink.clone())?;
        self.commit_immediate(PreparedHotkey {
            registration: Some(ActiveRegistration {
                platform,
                enabled,
                hold_in_flight,
                sink,
                focused_state,
                trigger_vk,
                required_mods,
            }),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    fn test_registration(
        engine: &HotkeyEngine,
        tx: Sender<HotkeyEvent>,
        enabled: bool,
    ) -> (ActiveRegistration, HotkeyEventSink) {
        let enabled_flag = Arc::new(AtomicBool::new(enabled));
        let hold_in_flight = Arc::new(AtomicBool::new(false));
        let sink = HotkeyEventSink {
            tx,
            enabled: enabled_flag.clone(),
            hold_in_flight: hold_in_flight.clone(),
            pressed: Arc::new(AtomicBool::new(false)),
            capture_suppressed: engine.capture_suppressed.clone(),
        };
        #[cfg(target_os = "windows")]
        let parsed = parse_combo("Ctrl+Alt+Space").unwrap();
        (
            ActiveRegistration {
                platform: PlatformRegistration::new(Box::new(|| {}), std::thread::spawn(|| {})),
                enabled: enabled_flag,
                hold_in_flight,
                sink: sink.clone(),
                #[cfg(target_os = "windows")]
                focused_state: Mutex::new(ComboState::new(&parsed)),
                #[cfg(target_os = "windows")]
                trigger_vk: super::super::windows::key_to_vk(&parsed.key).unwrap(),
                #[cfg(target_os = "windows")]
                required_mods: parsed.required_mask(),
            },
            sink,
        )
    }

    #[test]
    fn active_status_tracks_registration_ownership() {
        let (tx, _rx) = channel();
        let engine = HotkeyEngine::new(tx.clone());
        assert!(!engine.is_active());

        let (registration, _) = test_registration(&engine, tx, true);
        engine.inner.lock().active = Some(registration);
        assert!(engine.is_active());

        engine.stop();
        assert!(!engine.is_active());
    }

    #[test]
    fn queued_old_release_keeps_replacement_muted_until_dispatch_acknowledges_it() {
        let (tx, rx) = channel();
        let engine = HotkeyEngine::new(tx.clone());
        let (active, active_sink) = test_registration(&engine, tx.clone(), true);
        engine.inner.lock().active = Some(active);

        active_sink.send(HotkeyEvent::Pressed);
        active_sink.send(HotkeyEvent::Released);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Pressed)
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Released)
        );

        let (pending, pending_sink) = test_registration(&engine, tx, false);
        engine.commit_prepared(PreparedHotkey {
            registration: Some(pending),
        });
        pending_sink.send(HotkeyEvent::Pressed);
        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());

        engine.acknowledge_release();
        pending_sink.send(HotkeyEvent::Pressed);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Pressed)
        );
    }

    #[test]
    fn replacement_during_hold_activates_only_after_release_is_consumed() {
        let (tx, rx) = channel();
        let engine = HotkeyEngine::new(tx.clone());
        let (active, active_sink) = test_registration(&engine, tx.clone(), true);
        engine.inner.lock().active = Some(active);

        active_sink.send(HotkeyEvent::Pressed);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Pressed)
        );

        let (pending, pending_sink) = test_registration(&engine, tx, false);
        engine.commit_prepared(PreparedHotkey {
            registration: Some(pending),
        });

        pending_sink.send(HotkeyEvent::Pressed);
        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());

        active_sink.send(HotkeyEvent::Released);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Released)
        );

        engine.acknowledge_release();
        active_sink.send(HotkeyEvent::Pressed);
        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
        pending_sink.send(HotkeyEvent::Pressed);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Pressed)
        );
    }

    #[test]
    fn reset_hold_state_clears_pressed_and_allows_subsequent_presses() {
        let (tx, rx) = channel();
        let engine = HotkeyEngine::new(tx.clone());
        let (active, active_sink) = test_registration(&engine, tx.clone(), true);
        engine.inner.lock().active = Some(active);

        assert!(active_sink.send(HotkeyEvent::Pressed));
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Pressed)
        );

        assert!(!active_sink.send(HotkeyEvent::Pressed));

        engine.reset_hold_state();

        assert!(active_sink.send(HotkeyEvent::Pressed));
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Pressed)
        );
    }

    #[test]
    fn duplicate_input_sources_emit_one_transition_pair() {
        let (tx, rx) = channel();
        let engine = HotkeyEngine::new(tx.clone());
        let (active, first_source) = test_registration(&engine, tx, true);
        let second_source = first_source.clone();
        engine.inner.lock().active = Some(active);

        assert!(first_source.send(HotkeyEvent::Pressed));
        assert!(!second_source.send(HotkeyEvent::Pressed));
        assert!(second_source.send(HotkeyEvent::Released));
        assert!(!first_source.send(HotkeyEvent::Released));
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Pressed)
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Released)
        );
        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn shortcut_capture_forces_one_release_and_suppresses_new_transitions() {
        let (tx, rx) = channel();
        let engine = HotkeyEngine::new(tx.clone());
        let (active, source) = test_registration(&engine, tx, true);
        engine.inner.lock().active = Some(active);

        assert!(source.send(HotkeyEvent::Pressed));
        engine.set_hotkey_capture_active(true);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Pressed)
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Released)
        );
        assert!(!source.send(HotkeyEvent::Pressed));

        engine.set_hotkey_capture_active(false);
        assert!(source.send(HotkeyEvent::Pressed));
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Pressed)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn focused_sources_share_combo_semantics_and_release_on_modifier_up() {
        use super::super::combo::{MODBIT_ALT, MODBIT_CTRL};
        use winapi::um::winuser::VK_SPACE;

        let (tx, rx) = channel();
        let engine = HotkeyEngine::new(tx.clone());
        let (active, _) = test_registration(&engine, tx, true);
        engine.inner.lock().active = Some(active);

        assert!(!engine.focused_browser_key_event("Control", MODBIT_CTRL, true, false));
        assert!(!engine.focused_browser_key_event("Alt", MODBIT_CTRL | MODBIT_ALT, true, false));
        assert!(engine.focused_browser_key_event(" ", MODBIT_CTRL | MODBIT_ALT, true, false));
        assert!(engine.focused_windows_key_event(
            VK_SPACE as u32,
            MODBIT_CTRL | MODBIT_ALT,
            true,
            false
        ));
        assert!(engine.focused_browser_key_event(" ", MODBIT_CTRL | MODBIT_ALT, true, true));
        assert!(!engine.focused_browser_key_event("Alt", MODBIT_CTRL, false, false));

        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Pressed)
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Released)
        );
        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn focused_source_releases_when_settings_loses_focus() {
        use super::super::combo::{MODBIT_ALT, MODBIT_CTRL};

        let (tx, rx) = channel();
        let engine = HotkeyEngine::new(tx.clone());
        let (active, _) = test_registration(&engine, tx, true);
        engine.inner.lock().active = Some(active);
        engine.focused_browser_key_event(" ", MODBIT_CTRL | MODBIT_ALT, true, false);

        engine.focused_input_lost();

        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Pressed)
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(HotkeyEvent::Released)
        );
        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
    }
}

impl Drop for HotkeyEngine {
    fn drop(&mut self) {
        let state = self.inner.get_mut();
        if let Some(active) = state.active.take() {
            active.stop_and_join();
        }
        if let Some(pending) = state.pending.take() {
            pending.stop_and_join();
        }
    }
}
