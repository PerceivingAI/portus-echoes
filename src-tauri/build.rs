use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

const COMMANDS: &[&str] = &[
    "get_settings",
    "get_recording_state",
    "get_settings_navigation",
    "select_active_provider",
    "select_openai_model",
    "select_openai_custom_model",
    "set_openai_custom_model",
    "select_groq_model",
    "select_groq_custom_model",
    "set_groq_custom_model",
    "select_local_model",
    "select_local_custom_model",
    "set_local_custom_model_path",
    "set_hotkey",
    "focused_hotkey_key_event",
    "set_hotkey_capture_active",
    "reset_focused_hotkey_input",
    "complete_onboarding",
    "set_api_key",
    "get_api_key",
    "delete_api_key",
    "get_provider_status",
    "is_hotkey_registered",
    "is_hotkey_active",
    "run_diagnostics",
    "get_diagnostics_state",
    "list_local_models",
    "download_model",
    "cancel_download",
    "delete_model",
];

const LOCAL_MODELS_ENV: &str = "LOCAL_MODELS";
const OPENAI_MODELS_ENV: &str = "OPENAI_MODELS";
const GROQ_MODELS_ENV: &str = "GROQ_MODELS";
const RUST_LOCAL_MODELS_ENV: &str = "PORTUS_LOCAL_MODEL_URLS";
const RUST_OPENAI_MODELS_ENV: &str = "PORTUS_OPENAI_MODELS";
const RUST_GROQ_MODELS_ENV: &str = "PORTUS_GROQ_MODELS";
const RECORDING_LIMIT_ENV: &str = "RECORDING_LIMIT_SECS";
const RUST_RECORDING_LIMIT_ENV: &str = "PORTUS_RECORDING_LIMIT_SECS";

fn env_files(root: &Path, mode: &str) -> [PathBuf; 4] {
    [
        root.join(".env"),
        root.join(".env.local"),
        root.join(format!(".env.{mode}")),
        root.join(format!(".env.{mode}.local")),
    ]
}

fn vite_env_value(key: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={key}");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri must have a repository parent");
    let mode = if tauri_build::is_dev() {
        "development"
    } else {
        "production"
    };
    let files = env_files(root, mode);
    for path in &files {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    if let Ok(value) = env::var(key) {
        return Some(value);
    }

    let mut value = None;
    for path in files {
        if !path.is_file() {
            continue;
        }
        let entries = dotenvy::from_path_iter(&path)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
        for entry in entries {
            let (name, candidate) =
                entry.unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
            if name == key {
                value = Some(candidate);
            }
        }
    }
    value
}

fn configured_local_model_urls() -> Vec<String> {
    let Some(raw) = vite_env_value(LOCAL_MODELS_ENV) else {
        return Vec::new();
    };
    if raw.trim().is_empty() {
        return Vec::new();
    }

    let entries: serde_json::Value = serde_json::from_str(&raw)
        .unwrap_or_else(|error| panic!("{LOCAL_MODELS_ENV} must be valid JSON: {error}"));
    let entries = entries
        .as_array()
        .unwrap_or_else(|| panic!("{LOCAL_MODELS_ENV} must be a JSON array"));
    assert!(
        entries.len() <= 2,
        "{LOCAL_MODELS_ENV} supports at most two standard models"
    );

    let mut seen = HashSet::new();
    entries
        .iter()
        .map(|entry| {
            let download_path = entry
                .as_object()
                .and_then(|object| object.get("downloadPath"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| {
                    panic!("{LOCAL_MODELS_ENV} contains an invalid local model entry")
                });
            let parsed = url::Url::parse(download_path).unwrap_or_else(|error| {
                panic!("{LOCAL_MODELS_ENV} contains an invalid downloadPath: {error}")
            });
            assert!(
                parsed.scheme() == "https" && parsed.host_str().is_some(),
                "{LOCAL_MODELS_ENV} downloadPath must be an absolute HTTPS URL"
            );
            assert!(
                seen.insert(download_path),
                "{LOCAL_MODELS_ENV} contains a duplicate downloadPath"
            );
            download_path.to_string()
        })
        .collect()
}
fn configured_cloud_models(key: &str) -> String {
    let Some(raw) = vite_env_value(key) else {
        return "[]".to_string();
    };
    if raw.trim().is_empty() {
        return "[]".to_string();
    }

    let entries: serde_json::Value = serde_json::from_str(&raw)
        .unwrap_or_else(|error| panic!("{key} must be valid JSON: {error}"));
    let entries = entries
        .as_array()
        .unwrap_or_else(|| panic!("{key} must be a JSON array"));
    assert!(
        entries.len() <= 2,
        "{key} supports at most two standard models"
    );

    let mut seen_ids = HashSet::new();
    for entry in entries {
        let object = entry
            .as_object()
            .unwrap_or_else(|| panic!("{key} contains an invalid model entry"));
        let id = object
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("{key} contains an invalid model entry"));
        assert!(
            object
                .get("label")
                .and_then(serde_json::Value::as_str)
                .is_some(),
            "{key} contains an invalid model entry"
        );
        assert!(
            seen_ids.insert(id),
            "{key} contains duplicate Standard model id '{id}'"
        );
        let route = object
            .get("route")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("{key} contains an invalid model entry"));
        assert!(
            matches!(route, "completed" | "live"),
            "{key} route must be exactly 'completed' or 'live'"
        );
    }

    serde_json::to_string(entries).expect("validated cloud model catalog must serialize")
}

fn configured_language(key: &str) -> String {
    let raw = vite_env_value(key).unwrap_or_else(|| "auto".to_string());
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "auto" {
        "auto".to_string()
    } else {
        assert!(
            trimmed.len() == 2 && trimmed.chars().all(|c| c.is_ascii_alphabetic()),
            "configured language in {key} must be 'auto' or a 2-character ISO-639-1 code (e.g. 'fr', 'en')"
        );
        trimmed.to_lowercase()
    }
}

fn configured_segment_delivery() -> String {
    let raw = vite_env_value("LOCAL_SEGMENT_DELIVERY").unwrap_or_else(|| "false".to_string());
    let trimmed = raw.trim().to_lowercase();
    if trimmed.is_empty() || trimmed == "false" {
        "false".to_string()
    } else if trimmed == "true" {
        "true".to_string()
    } else {
        panic!("LOCAL_SEGMENT_DELIVERY must be 'true' or 'false', found: {raw:?}");
    }
}

fn configured_max_length() -> String {
    let raw = vite_env_value("LOCAL_MAX_LENGTH").unwrap_or_else(|| "0".to_string());
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "0" {
        "0".to_string()
    } else {
        let val: u32 = trimmed
            .parse()
            .unwrap_or_else(|error| panic!("LOCAL_MAX_LENGTH must be a non-negative integer: {error}"));
        val.to_string()
    }
}

fn configured_recording_limit() -> String {
    let raw = vite_env_value(RECORDING_LIMIT_ENV);
    match raw {
        None => "1800".to_string(),
        Some(val) => {
            let trimmed = val.trim();
            if trimmed.is_empty() || trimmed == "0" {
                "0".to_string()
            } else {
                let limit: u32 = trimmed
                    .parse()
                    .unwrap_or_else(|error| panic!("RECORDING_LIMIT_SECS must be a non-negative integer: {error}"));
                limit.to_string()
            }
        }
    }
}

fn main() {
    let configured_urls = serde_json::to_string(&configured_local_model_urls())
        .expect("local model URL list must serialize");
    println!("cargo:rustc-env={RUST_LOCAL_MODELS_ENV}={configured_urls}");

    let openai_models = configured_cloud_models(OPENAI_MODELS_ENV);
    let groq_models = configured_cloud_models(GROQ_MODELS_ENV);
    println!("cargo:rustc-env={RUST_OPENAI_MODELS_ENV}={openai_models}");
    println!("cargo:rustc-env={RUST_GROQ_MODELS_ENV}={groq_models}");

    let local_lang = configured_language("LOCAL_LANGUAGE");
    let openai_lang = configured_language("OPENAI_LANGUAGE");
    let groq_lang = configured_language("GROQ_LANGUAGE");
    println!("cargo:rustc-env=PORTUS_LOCAL_LANGUAGE={local_lang}");
    println!("cargo:rustc-env=PORTUS_OPENAI_LANGUAGE={openai_lang}");
    println!("cargo:rustc-env=PORTUS_GROQ_LANGUAGE={groq_lang}");
    let local_segment_delivery = configured_segment_delivery();
    let local_max_length = configured_max_length();
    println!("cargo:rustc-env=PORTUS_LOCAL_SEGMENT_DELIVERY={local_segment_delivery}");
    println!("cargo:rustc-env=PORTUS_LOCAL_MAX_LENGTH={local_max_length}");
    let rec_limit = configured_recording_limit();
    println!("cargo:rustc-env={RUST_RECORDING_LIMIT_ENV}={rec_limit}");

    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to build Tauri application metadata");
}
