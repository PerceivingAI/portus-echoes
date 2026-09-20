//! Tauri IPC bridge. No backend subsystem outside this module owns
//! `#[tauri::command]` handlers.

pub(crate) mod credentials;
pub(crate) mod diagnostics;
pub(crate) mod downloads;
pub(crate) mod hotkey;
pub(crate) mod providers;
pub(crate) mod state;

use crate::models::UserErrorCode;

pub(super) type CommandResult<T> = Result<T, UserErrorCode>;
