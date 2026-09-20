//! Build-time Standard Cloud model catalog shared with the frontend environment map.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::models::{AppSettings, CloudModelKind, CloudRoute, ProviderId};

#[derive(Debug, Deserialize)]
struct StandardCloudModel {
    id: String,
    #[allow(dead_code)]
    label: String,
    route: CloudRoute,
}

static OPENAI_MODELS: OnceLock<Vec<StandardCloudModel>> = OnceLock::new();
static GROQ_MODELS: OnceLock<Vec<StandardCloudModel>> = OnceLock::new();

fn parse_catalog(raw: &'static str) -> Vec<StandardCloudModel> {
    serde_json::from_str(raw).expect("build-validated Standard Cloud catalog must deserialize")
}

fn catalog(provider: ProviderId) -> Option<&'static [StandardCloudModel]> {
    match provider {
        ProviderId::Openai => Some(
            OPENAI_MODELS
                .get_or_init(|| parse_catalog(env!("PORTUS_OPENAI_MODELS")))
                .as_slice(),
        ),
        ProviderId::Groq => Some(
            GROQ_MODELS
                .get_or_init(|| parse_catalog(env!("PORTUS_GROQ_MODELS")))
                .as_slice(),
        ),
        ProviderId::Local => None,
    }
}

/// First build-time Standard model ID for clean-install selection defaults.
/// The catalog is the same build-validated source used for recording-route
/// resolution; no provider model ID is hardcoded in Rust.
pub(crate) fn first_standard_model_id(provider: ProviderId) -> Option<&'static str> {
    catalog(provider)?.first().map(|model| model.id.as_str())
}

/// Whether an IPC-supplied Standard model ID belongs to that provider's
/// build-compiled catalog. Custom IDs deliberately use a different command.
pub(crate) fn is_standard_model_id(provider: ProviderId, model_id: &str) -> bool {
    catalog(provider).is_some_and(|models| route_in_catalog(models, model_id).is_some())
}

fn route_in_catalog(models: &[StandardCloudModel], model_id: &str) -> Option<CloudRoute> {
    models
        .iter()
        .find(|model| model.id == model_id)
        .map(|model| model.route)
}

fn recording_route_in_catalog(
    settings: &AppSettings,
    provider: ProviderId,
    models: &[StandardCloudModel],
) -> Option<CloudRoute> {
    let (model_id, kind) = match provider {
        ProviderId::Openai => (&settings.openai_model, settings.openai_model_kind),
        ProviderId::Groq => (&settings.groq_model, settings.groq_model_kind),
        ProviderId::Local => return None,
    };

    if model_id.trim().is_empty() {
        return None;
    }

    match kind {
        Some(CloudModelKind::Standard) => route_in_catalog(models, model_id),
        Some(CloudModelKind::Custom) | None => Some(CloudRoute::CompletedAudio),
    }
}

/// Resolve only Standard Cloud selections from one recording-start settings
/// snapshot. Custom IDs deliberately never enter the Standard catalog lookup,
/// even when their text equals a Standard ID.
pub(crate) fn recording_route(settings: &AppSettings, provider: ProviderId) -> Option<CloudRoute> {
    recording_route_in_catalog(settings, provider, catalog(provider)?)
}

/// Require the current Standard selection to remain compatible with the route
/// frozen for an already-admitted recording. This preserves completion-time
/// model selection only within the same transport lifecycle; Custom, unknown,
/// incomplete, or cross-route selections fail closed rather than entering the
/// wrong provider transport.
pub(crate) fn current_selection_matches_route(
    settings: &AppSettings,
    provider: ProviderId,
    frozen_route: CloudRoute,
) -> bool {
    recording_route(settings, provider) == Some(frozen_route)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, route: CloudRoute) -> StandardCloudModel {
        StandardCloudModel {
            id: id.to_string(),
            label: id.to_string(),
            route,
        }
    }

    fn catalog_fixture() -> Vec<StandardCloudModel> {
        vec![
            model("completed-model", CloudRoute::CompletedAudio),
            model("live-model", CloudRoute::LiveAudio),
        ]
    }

    #[test]
    fn standard_route_lookup_uses_only_explicit_catalog_metadata() {
        let models = catalog_fixture();
        assert_eq!(
            route_in_catalog(&models, "completed-model"),
            Some(CloudRoute::CompletedAudio)
        );
        assert_eq!(
            route_in_catalog(&models, "live-model"),
            Some(CloudRoute::LiveAudio)
        );
        assert_eq!(route_in_catalog(&models, "unknown-model"), None);
    }

    #[test]
    fn provider_selection_reads_only_that_providers_snapshot_fields() {
        let models = catalog_fixture();
        let settings = AppSettings {
            openai_model: "live-model".to_string(),
            openai_model_kind: Some(CloudModelKind::Standard),
            groq_model: "completed-model".to_string(),
            groq_model_kind: Some(CloudModelKind::Standard),
            ..Default::default()
        };

        assert_eq!(
            recording_route_in_catalog(&settings, ProviderId::Openai, &models),
            Some(CloudRoute::LiveAudio)
        );
        assert_eq!(
            recording_route_in_catalog(&settings, ProviderId::Groq, &models),
            Some(CloudRoute::CompletedAudio)
        );
        assert_eq!(
            recording_route_in_catalog(&settings, ProviderId::Local, &models),
            None
        );
    }

    #[test]
    fn custom_selection_routes_to_completed_audio() {
        let models = catalog_fixture();
        for provider in [ProviderId::Openai, ProviderId::Groq] {
            let mut settings = AppSettings::default();
            match provider {
                ProviderId::Openai => {
                    settings.openai_model = "custom-model".to_string();
                    settings.openai_model_kind = Some(CloudModelKind::Custom);
                }
                ProviderId::Groq => {
                    settings.groq_model = "custom-model".to_string();
                    settings.groq_model_kind = Some(CloudModelKind::Custom);
                }
                ProviderId::Local => unreachable!(),
            }
            assert_eq!(
                recording_route_in_catalog(&settings, provider, &models),
                Some(CloudRoute::CompletedAudio)
            );
        }
    }

    #[test]
    fn incomplete_or_unknown_standard_selection_fails_closed() {
        let models = catalog_fixture();
        for (model_id, kind) in [
            ("", Some(CloudModelKind::Standard)),
            ("", Some(CloudModelKind::Custom)),
            ("unknown-model", Some(CloudModelKind::Standard)),
        ] {
            let settings = AppSettings {
                openai_model: model_id.to_string(),
                openai_model_kind: kind,
                ..Default::default()
            };
            assert_eq!(
                recording_route_in_catalog(&settings, ProviderId::Openai, &models),
                None
            );
        }
    }

    #[test]
    fn current_selection_must_remain_on_the_frozen_route() {
        let models = catalog_fixture();
        let mut settings = AppSettings {
            openai_model: "completed-model".to_string(),
            openai_model_kind: Some(CloudModelKind::Standard),
            ..Default::default()
        };

        assert_eq!(
            recording_route_in_catalog(&settings, ProviderId::Openai, &models),
            Some(CloudRoute::CompletedAudio)
        );

        settings.openai_model = "live-model".to_string();
        assert_ne!(
            recording_route_in_catalog(&settings, ProviderId::Openai, &models),
            Some(CloudRoute::CompletedAudio)
        );

        settings.openai_model_kind = Some(CloudModelKind::Custom);
        settings.openai_model = "custom-model-id".to_string();
        assert_eq!(
            recording_route_in_catalog(&settings, ProviderId::Openai, &models),
            Some(CloudRoute::CompletedAudio),
            "Custom selection routes to CompletedAudio"
        );
    }

    #[test]
    fn route_resolution_uses_the_recording_start_snapshot_only() {
        let models = catalog_fixture();
        let mut current = AppSettings {
            openai_model: "live-model".to_string(),
            openai_model_kind: Some(CloudModelKind::Standard),
            ..Default::default()
        };
        let recording_start_snapshot = current.clone();
        let frozen =
            recording_route_in_catalog(&recording_start_snapshot, ProviderId::Openai, &models);

        current.openai_model = "completed-model".to_string();
        assert_eq!(frozen, Some(CloudRoute::LiveAudio));
        assert_eq!(
            recording_route_in_catalog(&current, ProviderId::Openai, &models),
            Some(CloudRoute::CompletedAudio)
        );
    }
    #[test]
    fn route_resolver_contains_no_transport_or_hardcoded_standard_model_semantics() {
        let source = include_str!("cloud_catalog.rs");
        for forbidden in [
            ["req", "west"].concat(),
            ["multi", "part"].concat(),
            ["Web", "Socket"].concat(),
            ["gpt-", "transcribe"].concat(),
            ["gpt-live-", "transcribe"].concat(),
            ["whisper-large-", "v3"].concat(),
        ] {
            assert!(
                !source.contains(&forbidden),
                "route resolution must use catalog metadata only, never transport logic or hardcoded Standard model semantics: {forbidden}"
            );
        }
    }

    #[test]
    fn standard_selection_validation_accepts_only_build_catalog_ids() {
        for provider in [ProviderId::Openai, ProviderId::Groq] {
            if let Some(first) = catalog(provider).and_then(|models| models.first()) {
                assert!(is_standard_model_id(provider, &first.id));
            }
            assert!(!is_standard_model_id(
                provider,
                "not-a-build-standard-model"
            ));
        }
        assert!(!is_standard_model_id(
            ProviderId::Local,
            "not-a-cloud-provider"
        ));
    }

    #[test]
    fn production_catalog_lookup_matches_the_build_compiled_route_when_populated() {
        for provider in [ProviderId::Openai, ProviderId::Groq] {
            let Some(first_standard) = catalog(provider).and_then(|models| models.first()) else {
                continue;
            };
            let mut settings = AppSettings::default();
            match provider {
                ProviderId::Openai => {
                    settings.openai_model = first_standard.id.clone();
                    settings.openai_model_kind = Some(CloudModelKind::Standard);
                }
                ProviderId::Groq => {
                    settings.groq_model = first_standard.id.clone();
                    settings.groq_model_kind = Some(CloudModelKind::Standard);
                }
                ProviderId::Local => unreachable!(),
            }
            assert_eq!(
                recording_route(&settings, provider),
                Some(first_standard.route)
            );
        }
    }
}
