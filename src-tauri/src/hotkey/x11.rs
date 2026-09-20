//! Linux X11 global hold-to-talk registration.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, GrabMode, KeyButMask, ModMask};
use x11rb::protocol::xkb::ConnectionExt as _;
use x11rb::protocol::Event;

use super::combo::{
    ComboState, HotkeyError, ParsedCombo, MODBIT_ALT, MODBIT_CTRL, MODBIT_SHIFT, MODBIT_SUPER,
};
use super::engine::{HotkeyEventSink, PlatformRegistration};

pub(super) fn key_to_keysym(key: &str) -> Option<u32> {
    let sym = match key {
        "space" => 0x0020,
        "tab" => 0xFF09,
        "enter" | "return" => 0xFF0D,
        "esc" | "escape" => 0xFF1B,
        "backspace" => 0xFF08,
        "delete" | "del" => 0xFFFF,
        "insert" | "ins" => 0xFF63,
        "home" => 0xFF50,
        "end" => 0xFF57,
        "pageup" | "pgup" => 0xFF55,
        "pagedown" | "pgdn" => 0xFF56,
        "up" => 0xFF52,
        "down" => 0xFF54,
        "left" => 0xFF51,
        "right" => 0xFF53,
        "capslock" => 0xFFE5,
        "numlock" => 0xFF7F,
        "scrolllock" => 0xFF14,
        "printscreen" => 0xFF61,
        "pause" => 0xFF13,
        _ => {
            if let Some(f) = key
                .strip_prefix('f')
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| (1..=24).contains(n))
            {
                0xFFBE + f - 1
            } else if key.chars().count() == 1 {
                // Printable ASCII maps 1:1 to keysyms.
                let c = key.as_bytes()[0];
                if !(0x20..=0x7E).contains(&c) {
                    return None;
                }
                c as u32
            } else {
                return None;
            }
        }
    };
    Some(sym)
}

fn required_modmask(combo: &ParsedCombo) -> u16 {
    let mut mask = 0u16;
    if combo.shift {
        mask |= u16::from(KeyButMask::SHIFT);
    }
    if combo.ctrl {
        mask |= u16::from(KeyButMask::CONTROL);
    }
    if combo.alt {
        mask |= u16::from(KeyButMask::MOD1);
    }
    if combo.super_ {
        mask |= u16::from(KeyButMask::MOD4);
    }
    mask
}

pub fn start(
    combo: ParsedCombo,
    sink: HotkeyEventSink,
) -> Result<PlatformRegistration, HotkeyError> {
    let keysym = key_to_keysym(&combo.key).ok_or(HotkeyError::UnsupportedKey)?;
    let required_mask = combo.required_mask();
    let grab_mask = required_modmask(&combo);
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_thread = shutdown.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<StopHandle, HotkeyError>>();
    let join = std::thread::spawn(move || {
        let _ = run(
            keysym,
            grab_mask,
            required_mask,
            sink,
            shutdown_thread,
            ready_tx,
        );
    });
    match ready_rx.recv() {
        Ok(Ok(stop)) => Ok(PlatformRegistration::new(stop, join)),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(_) => Err(HotkeyError::WorkerStopped),
    }
}

type StopHandle = Box<dyn FnOnce() + Send>;

fn run(
    keysym: u32,
    grab_mask: u16,
    required_mask: u8,
    sink: HotkeyEventSink,
    shutdown: Arc<AtomicBool>,
    ready_tx: std::sync::mpsc::Sender<Result<StopHandle, HotkeyError>>,
) -> Result<(), HotkeyError> {
    let (conn, screen_num) = x11rb::connect(None).map_err(|_| HotkeyError::PlatformFailure)?;
    let root = conn.setup().roots[screen_num].root;
    let keycode = keysym_to_keycode(&conn, keysym).ok_or(HotkeyError::UnsupportedKey)?;

    // Enable detectable auto-repeat via the XKB extension so the X server
    // suppresses synthetic KeyRelease events while a key is held down.
    // This guarantees that multi-minute push-to-talk holds are never broken
    // by synthetic release jitter under CPU load.
    if conn
        .xkb_use_extension(1, 0)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some()
    {
        let _ = conn.xkb_per_client_flags(
            x11rb::protocol::xkb::ID::USE_CORE_KBD.into(),
            x11rb::protocol::xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
            x11rb::protocol::xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
            Default::default(),
            Default::default(),
            Default::default(),
        );
    }
    // Grab for every Lock/NumLock permutation so the combo works
    // regardless of CapsLock/NumLock state (X11 gotcha).
    let variants = [
        grab_mask,
        grab_mask | u16::from(KeyButMask::LOCK),
        grab_mask | u16::from(KeyButMask::MOD2),
        grab_mask | u16::from(KeyButMask::LOCK) | u16::from(KeyButMask::MOD2),
    ];
    for mask in variants {
        conn.grab_key(
            false,
            root,
            ModMask::from(mask),
            keycode,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .map_err(|_| HotkeyError::PlatformFailure)?
        .check()
        .map_err(|_| HotkeyError::RegistrationFailed)?;
    }

    let shutdown_stop = shutdown.clone();
    ready_tx
        .send(Ok(Box::new(move || {
            shutdown_stop.store(true, Ordering::SeqCst);
        })))
        .map_err(|_| HotkeyError::WorkerStopped)?;

    let mut state = ComboState::new(&ParsedCombo {
        ctrl: required_mask & MODBIT_CTRL != 0,
        alt: required_mask & MODBIT_ALT != 0,
        shift: required_mask & MODBIT_SHIFT != 0,
        super_: required_mask & MODBIT_SUPER != 0,
        key: String::new(),
    });
    let mut pending_event = None;
    loop {
        if shutdown.load(Ordering::SeqCst) {
            let _ = conn.ungrab_key(keycode, root, ModMask::ANY);
            let _ = conn.flush();
            return Ok(());
        }
        let event = match pending_event.take() {
            Some(event) => Some(event),
            None => match conn.poll_for_event() {
                Ok(Some(e)) => Some(e),
                Ok(None) => None,
                Err(_) => return Err(HotkeyError::PlatformFailure),
            },
        };
        match event {
            Some(Event::KeyPress(e)) if e.detail == keycode => {
                let mods = u16::from(e.state);
                if let Some(ev) = state.on_grabbed_press(xmods_to_bits(mods)) {
                    sink.send(ev);
                }
            }
            Some(Event::KeyRelease(e)) if e.detail == keycode => {
                let next = conn
                    .poll_for_event()
                    .map_err(|_| HotkeyError::PlatformFailure)?;
                let is_repeat = matches!(
                    next.as_ref(),
                    Some(Event::KeyPress(p)) if p.detail == keycode && p.time == e.time
                );
                if !is_repeat {
                    pending_event = next;
                    if let Some(ev) = state.on_grabbed_release() {
                        sink.send(ev);
                    }
                }
            }
            Some(_) => {}
            None => {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn xmods_to_bits(mask: u16) -> u8 {
    let mut out = 0u8;
    if mask & u16::from(KeyButMask::CONTROL) != 0 {
        out |= MODBIT_CTRL;
    }
    if mask & u16::from(KeyButMask::MOD1) != 0 {
        out |= MODBIT_ALT;
    }
    if mask & u16::from(KeyButMask::SHIFT) != 0 {
        out |= MODBIT_SHIFT;
    }
    if mask & u16::from(KeyButMask::MOD4) != 0 {
        out |= MODBIT_SUPER;
    }
    out
}

fn keysym_to_keycode<C: Connection>(conn: &C, keysym: u32) -> Option<u8> {
    let setup = conn.setup();
    let lo = setup.min_keycode;
    let hi = setup.max_keycode;
    let count = hi - lo + 1;
    let mapping = conn.get_keyboard_mapping(lo, count).ok()?.reply().ok()?;
    for (i, syms) in mapping
        .keysyms
        .chunks(mapping.keysyms_per_keycode as usize)
        .enumerate()
    {
        if syms.contains(&keysym) {
            return Some(lo + i as u8);
        }
    }
    None
}
