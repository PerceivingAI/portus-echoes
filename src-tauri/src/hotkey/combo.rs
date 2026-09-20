//! Platform-neutral hotkey parsing and hold-state semantics.

use crate::models::HotkeyEvent;

pub const MODBIT_CTRL: u8 = 1;
pub const MODBIT_ALT: u8 = 2;
pub const MODBIT_SHIFT: u8 = 4;
pub const MODBIT_SUPER: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyError {
    InvalidCombination,
    UnsupportedKey,
    RegistrationFailed,
    PlatformFailure,
    WorkerStopped,
}

/// A parsed hotkey combo: required modifiers plus one non-modifier key
/// (lowercased name, e.g. `"space"`, `"f9"`, `"v"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCombo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_: bool,
    pub key: String,
}

impl ParsedCombo {
    pub fn required_mask(&self) -> u8 {
        (if self.ctrl { MODBIT_CTRL } else { 0 })
            | (if self.alt { MODBIT_ALT } else { 0 })
            | (if self.shift { MODBIT_SHIFT } else { 0 })
            | (if self.super_ { MODBIT_SUPER } else { 0 })
    }
}

/// Parse a validated combo string (`"Ctrl+Alt+Space"`). Accepts the
/// modifier synonyms the settings validator allows. The key is NOT
/// validated against platform key tables here — that happens at engine
/// start, which surfaces unknown keys as errors.
pub fn parse_combo(combo: &str) -> Result<ParsedCombo, HotkeyError> {
    let parts: Vec<&str> = combo.split('+').map(str::trim).collect();
    let (mods, key) = parts.split_at(parts.len() - 1);
    let key = key[0];
    if key.is_empty() || canonical_modifier(key).is_some() {
        return Err(HotkeyError::InvalidCombination);
    }
    let mut parsed = ParsedCombo {
        ctrl: false,
        alt: false,
        shift: false,
        super_: false,
        key: key.to_ascii_lowercase(),
    };
    for m in mods {
        match canonical_modifier(m) {
            Some("ctrl") => parsed.ctrl = true,
            Some("alt") => parsed.alt = true,
            Some("shift") => parsed.shift = true,
            Some("super") => parsed.super_ = true,
            _ => return Err(HotkeyError::InvalidCombination),
        }
    }
    Ok(parsed)
}

fn canonical_modifier(token: &str) -> Option<&'static str> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some("ctrl"),
        "alt" => Some("alt"),
        "shift" => Some("shift"),
        "super" | "meta" | "win" | "cmd" => Some("super"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Decision state machine (shared, platform-agnostic, fully testable)
// ---------------------------------------------------------------------------

/// Tracks combo state and decides when to emit events.
///
/// Two feed styles:
/// - Windows: every keystroke arrives (`on_key_down`/`on_key_up`); the
///   machine tracks modifier state itself.
/// - X11: only grabbed-key events arrive, with the modifier state attached
///   (`on_grabbed_press`/`on_grabbed_release`).
pub struct ComboState {
    required: u8,
    #[allow(dead_code)] // used on Windows, untouched on X11
    mods: u8,
    active: bool,
}

impl ComboState {
    pub fn new(combo: &ParsedCombo) -> Self {
        Self {
            required: combo.required_mask(),
            mods: 0,
            active: false,
        }
    }

    #[allow(dead_code)] // used by the decision-machine tests
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// A key went down. `is_trigger`: it is the combo's key. `mod_bit`: the
    /// modifier bit it represents (0 for non-modifiers). Left/right variants
    /// are mapped to the same bit by the caller.
    #[allow(dead_code)] // Windows feed style and unit tests
    pub fn on_key_down(&mut self, is_trigger: bool, mod_bit: u8) -> Option<HotkeyEvent> {
        if mod_bit != 0 {
            self.mods |= mod_bit;
        }
        // Exact modifier match: extra held modifiers suppress the combo.
        // `active` also suppresses OS auto-repeat — a held combo cannot
        // produce a second Pressed.
        if is_trigger && !self.active && self.mods == self.required {
            self.active = true;
            return Some(HotkeyEvent::Pressed);
        }
        None
    }

    /// Focused-window adapters provide the modifier snapshot carried by the
    /// current browser/native event. This avoids depending on seeing modifier
    /// key-down events that occurred before the window gained focus.
    #[allow(dead_code)] // Windows feed style and unit tests
    pub fn on_key_down_with_mods(
        &mut self,
        is_trigger: bool,
        mod_bit: u8,
        current_mods: u8,
    ) -> Option<HotkeyEvent> {
        self.mods = current_mods;
        self.on_key_down(is_trigger, mod_bit)
    }

    /// A key went up. Releasing the trigger key OR any required modifier
    /// ends the hold.
    #[allow(dead_code)] // Windows feed style and unit tests
    pub fn on_key_up(&mut self, is_trigger: bool, mod_bit: u8) -> Option<HotkeyEvent> {
        let released =
            if self.active && (is_trigger || (mod_bit != 0 && self.required & mod_bit != 0)) {
                self.active = false;
                Some(HotkeyEvent::Released)
            } else {
                None
            };
        if mod_bit != 0 {
            self.mods &= !mod_bit;
        }
        released
    }

    /// Focused-window counterpart to `on_key_down_with_mods`.
    #[allow(dead_code)] // Windows feed style and unit tests
    pub fn on_key_up_with_mods(
        &mut self,
        is_trigger: bool,
        mod_bit: u8,
        current_mods: u8,
    ) -> Option<HotkeyEvent> {
        let released =
            if self.active && (is_trigger || (mod_bit != 0 && self.required & mod_bit != 0)) {
                self.active = false;
                Some(HotkeyEvent::Released)
            } else {
                None
            };
        self.mods = current_mods;
        released
    }

    #[allow(dead_code)] // Windows feed style and unit tests
    pub fn reset(&mut self) -> Option<HotkeyEvent> {
        self.mods = 0;
        if self.active {
            self.active = false;
            Some(HotkeyEvent::Released)
        } else {
            None
        }
    }

    /// X11 style: grabbed-key press carrying the current modifier state.
    #[allow(dead_code)] // X11 feed style — unused on Windows
    pub fn on_grabbed_press(&mut self, mods: u8) -> Option<HotkeyEvent> {
        if !self.active && mods == self.required {
            self.active = true;
            return Some(HotkeyEvent::Pressed);
        }
        None
    }

    /// X11 style: grabbed-key release.
    #[allow(dead_code)] // X11 feed style — unused on Windows
    pub fn on_grabbed_release(&mut self) -> Option<HotkeyEvent> {
        if self.active {
            self.active = false;
            return Some(HotkeyEvent::Released);
        }
        None
    }
}
