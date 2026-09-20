use std::fs;
use std::io;
use std::path::Path;

use super::types::DiagnosticsStorageData;

/// Read the latest completed diagnostics snapshot from `diagnostics.json`.
/// Missing, unreadable, malformed, or old-schema files become the neutral never-run state.
pub fn load_storage(path: &Path) -> DiagnosticsStorageData {
    if !path.is_file() {
        return DiagnosticsStorageData::default();
    }

    let parsed = fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<DiagnosticsStorageData>(&raw).ok());

    match parsed {
        Some(data) if data.snapshot.is_some() && !data.last_scan_display.trim().is_empty() => data,
        _ => DiagnosticsStorageData::default(),
    }
}

/// Atomically replace `diagnostics.json` with one complete latest snapshot.
/// Never-run/partial data is not a persistable diagnostics result.
pub fn save_storage(path: &Path, data: &DiagnosticsStorageData) -> io::Result<()> {
    if data.snapshot.is_none() || data.last_scan_display.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "diagnostics storage requires one complete snapshot",
        ));
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(data)?;
    fs::write(&tmp, json)?;

    if let Err(error) = replace_storage_file(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn replace_storage_file(tmp: &Path, path: &Path) -> io::Result<()> {
    fs::rename(tmp, path)
}

#[cfg(target_os = "windows")]
fn replace_storage_file(tmp: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::winbase::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};

    let tmp_wide: Vec<u16> = tmp
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let path_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let moved = unsafe {
        MoveFileExW(
            tmp_wide.as_ptr(),
            path_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Format current SystemTime into "DD MMM YYYY" string (e.g. "21 Mar 2026").
pub(super) fn format_current_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (secs / 86400) as i64;
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146097) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!("{:02} {} {}", d, months[(m - 1) as usize], year)
}
