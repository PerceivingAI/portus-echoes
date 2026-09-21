//! TOML-backed application settings.
//!
//! Load/save of `AppSettings` at the OS-specific `config.toml` path, plus the
//! provider configuration state machine. Secrets are never stored here — see
//! `keystore.rs`. Schema and persistence contract: `docs/SETTINGS.md`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::models::{AppSettings, LocalModelKind, ProviderId};

/// OS-specific config path per `docs/SETTINGS.md`:
/// - Windows: `%APPDATA%\PortusEchoes\config.toml`
/// - Linux: `~/.local/share/portusechoes/config.toml`
pub fn default_config_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = dirs::config_dir().map(|p| p.join("PortusEchoes"));
    #[cfg(target_os = "linux")]
    let base = dirs::data_dir().map(|p| p.join("portusechoes"));
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    let base = None;
    base.map(|dir| dir.join("config.toml"))
}

/// Load settings from `path`.
///
/// - Missing file → defaults (first run).
/// - Corrupt or unreadable file → the file is preserved as
///   `config.toml.corrupt` and defaults are returned (recovery path).
/// - Partial file → missing keys fall back to defaults (serde).
pub fn load(path: &Path) -> AppSettings {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return AppSettings::default(),
        Err(_) => return preserve_and_default(path),
    };
    match toml::from_str::<AppSettings>(&raw) {
        Ok(settings) => settings,
        Err(_) => preserve_and_default(path),
    }
}

fn preserve_and_default(path: &Path) -> AppSettings {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".corrupt");
    // Best effort: keep the broken file for inspection; defaults apply either way.
    let _ = fs::rename(path, backup);
    AppSettings::default()
}

/// Persist settings to `path`, creating parent directories. The write is
/// staged through a temp file and renamed so a crash mid-write cannot leave
/// a torn config.
pub fn save(path: &Path, settings: &AppSettings) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = toml::to_string_pretty(settings)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Provider configuration state machine (docs/SETTINGS.md)
// ---------------------------------------------------------------------------

/// Which providers currently hold a non-empty API key in the keystore.
/// Supplied by `keystore.rs`; `settings.rs` itself never touches secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyPresence {
    pub openai: bool,
    pub groq: bool,
}

/// Full computed state of the provider configuration state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderStatus {
    pub openai_configured: bool,
    pub groq_configured: bool,
    pub local_configured: bool,
    pub active_provider: Option<ProviderId>,
    pub active_configured: bool,
}

fn local_model_configured(settings: &AppSettings) -> bool {
    if settings.local_model_path.is_empty() {
        return false;
    }
    match settings.local_model_kind {
        Some(LocalModelKind::Standard) => Path::new(&settings.local_model_path).is_file(),
        Some(LocalModelKind::Custom) => true,
        None => false,
    }
}

/// Compute the state machine for the current settings + keystore state.
pub fn provider_status(settings: &AppSettings, keys: KeyPresence) -> ProviderStatus {
    let openai_configured =
        settings.openai_model_kind.is_some() && !settings.openai_model.is_empty() && keys.openai;
    let groq_configured =
        settings.groq_model_kind.is_some() && !settings.groq_model.is_empty() && keys.groq;
    let local_configured = local_model_configured(settings);
    let active_configured = match settings.active_provider {
        Some(ProviderId::Openai) => openai_configured,
        Some(ProviderId::Groq) => groq_configured,
        Some(ProviderId::Local) => local_configured,
        None => false,
    };
    ProviderStatus {
        openai_configured,
        groq_configured,
        local_configured,
        active_provider: settings.active_provider,
        active_configured,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CloudModelKind;
    use std::fs;

    fn keys(openai: bool, groq: bool) -> KeyPresence {
        KeyPresence { openai, groq }
    }

    // --- load / save ---

    #[test]
    fn missing_config_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert_eq!(load(&path), AppSettings::default());
    }

    #[test]
    fn partial_config_loads_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "language = \"de\"\n").unwrap();
        let s = load(&path);
        assert_eq!(s.language, "de");
        assert_eq!(s.hotkey, "Ctrl+Alt+Space");
    }


    #[test]
    fn unset_active_provider_deserializes_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "active_provider = \"\"\n").unwrap();
        assert_eq!(load(&path).active_provider, None);
    }

    #[test]
    fn corrupt_config_is_preserved_and_defaults_apply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "this is [ not valid toml").unwrap();
        assert_eq!(load(&path), AppSettings::default());
        let corrupt = dir.path().join("config.toml.corrupt");
        assert!(corrupt.is_file(), "corrupt file must be preserved");
        assert_eq!(
            fs::read_to_string(corrupt).unwrap(),
            "this is [ not valid toml"
        );
    }

    #[test]
    fn save_then_load_persists_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let s = AppSettings {
            active_provider: Some(ProviderId::Groq),
            openai_model: "environment-openai-model".into(),
            openai_model_kind: Some(CloudModelKind::Standard),
            openai_custom_model: "user-openai-model".into(),
            groq_model: "environment-groq-model".into(),
            groq_model_kind: Some(CloudModelKind::Custom),
            groq_custom_model: "user-groq-model".into(),
            local_model: "Whisper Small Q8".into(),
            local_model_path: "C:\\models\\environment-local.bin".into(),
            local_model_kind: Some(LocalModelKind::Standard),
            local_custom_model_path: "C:\\models\\user-local.bin".into(),
            language: "fr".into(),
            hotkey: "Alt+Q".into(),
            onboarding_complete: true,
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path), s);
    }

    #[test]
    fn save_creates_parent_dirs_and_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("config.toml");
        save(&path, &AppSettings::default()).unwrap();
        assert!(path.is_file());
        assert!(!dir
            .path()
            .join("a")
            .join("b")
            .join("config.toml.tmp")
            .exists());
    }

    // --- state machine ---

    #[test]
    fn openai_and_groq_require_model_and_key() {
        let mut settings = AppSettings::default();
        settings.openai_model = "environment-openai-id".to_string();
        settings.openai_model_kind = Some(CloudModelKind::Standard);
        settings.groq_model = "environment-groq-id".to_string();
        settings.groq_model_kind = Some(CloudModelKind::Custom);
        assert!(provider_status(&settings, keys(true, false)).openai_configured);
        assert!(!provider_status(&settings, keys(false, false)).openai_configured);
        assert!(provider_status(&settings, keys(false, true)).groq_configured);
        assert!(!provider_status(&settings, keys(false, false)).groq_configured);

        settings.openai_model.clear();
        assert!(!provider_status(&settings, keys(true, false)).openai_configured);

        settings.openai_model = "environment-openai-id".to_string();
        settings.openai_model_kind = None;
        assert!(!provider_status(&settings, keys(true, false)).openai_configured);
    }

    #[test]
    fn standard_local_requires_selected_downloaded_file() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("environment-model.bin");
        fs::write(&model, b"weights").unwrap();

        let mut settings = AppSettings {
            local_model_path: model.to_string_lossy().into_owned(),
            local_model_kind: Some(LocalModelKind::Standard),
            ..Default::default()
        };
        assert!(provider_status(&settings, keys(false, false)).local_configured);

        settings.local_model_path = dir
            .path()
            .join("missing.bin")
            .to_string_lossy()
            .into_owned();
        assert!(!provider_status(&settings, keys(false, false)).local_configured);

        settings.local_model_path.clear();
        assert!(!provider_status(&settings, keys(false, false)).local_configured);
    }

    #[test]
    fn custom_local_requires_only_non_empty_user_value() {
        let mut settings = AppSettings {
            local_model_path: "not-a-file-and-not-a-bin-extension".to_string(),
            local_model_kind: Some(LocalModelKind::Custom),
            ..Default::default()
        };
        assert!(provider_status(&settings, keys(false, false)).local_configured);

        settings.local_model_path.clear();
        assert!(!provider_status(&settings, keys(false, false)).local_configured);
    }
}
