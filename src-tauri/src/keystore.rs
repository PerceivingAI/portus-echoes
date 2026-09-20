//! OS-backed secure key storage (Windows Credential Manager / Linux Secret
//! Service).
//!
//! Sole persistence owner of `openai_api_key` and `groq_api_key`. Keys never
//! touch `settings.rs`, TOML config, logs, or emitted Tauri events. The
//! owner-approved Settings surface may retrieve key material through explicit
//! IPC for masked/revealable editing; see `docs/CLOUD.md` and `docs/SECURITY.md`.
//! Service/accounts per `docs/SETTINGS.md`: service `portusechoes`.

use crate::models::ProviderId;
use crate::settings::KeyPresence;

/// Keystore service name, per `docs/SETTINGS.md`.
pub const SERVICE: &str = "portusechoes";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStoreError {
    Unavailable,
    ReadFailed,
    WriteFailed,
    DeleteFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeystoreError {
    UnsupportedProvider,
    EmptyKey,
    Store(CredentialStoreError),
}

fn account(provider: ProviderId) -> Result<&'static str, KeystoreError> {
    match provider {
        ProviderId::Openai => Ok("openai_api_key"),
        ProviderId::Groq => Ok("groq_api_key"),
        ProviderId::Local => Err(KeystoreError::UnsupportedProvider),
    }
}

/// Storage backend abstraction. The production backend is the OS credential
/// store; tests use an in-memory backend so no OS state is touched.
pub trait CredentialStore {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, CredentialStoreError>;
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialStoreError>;
    fn delete(&self, service: &str, account: &str) -> Result<(), CredentialStoreError>;
}

/// OS credential store backend (keyring).
#[derive(Debug, Clone, Copy, Default)]
pub struct PlatformStore;

impl PlatformStore {
    fn entry(service: &str, account: &str) -> Result<keyring::Entry, CredentialStoreError> {
        keyring::Entry::new(service, account).map_err(|_| CredentialStoreError::Unavailable)
    }
}

impl CredentialStore for PlatformStore {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, CredentialStoreError> {
        let entry = match Self::entry(service, account) {
            Ok(entry) => entry,
            Err(_) => return Ok(None),
        };
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry | keyring::Error::NoDefaultStore) => Ok(None),
            Err(_) => Err(CredentialStoreError::ReadFailed),
        }
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialStoreError> {
        Self::entry(service, account)?
            .set_password(value)
            .map_err(|_| CredentialStoreError::WriteFailed)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), CredentialStoreError> {
        let entry = match Self::entry(service, account) {
            Ok(entry) => entry,
            Err(_) => return Ok(()),
        };
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry | keyring::Error::NoDefaultStore) => Ok(()),
            Err(_) => Err(CredentialStoreError::DeleteFailed),
        }
    }
}

/// Read/write/delete API keys. Keys are trimmed and must be non-empty —
/// the provider state machine treats empty keys as absent.
#[derive(Debug, Clone)]
pub struct Keystore<S: CredentialStore> {
    store: S,
}

impl<S: CredentialStore> Keystore<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    pub fn set_key_typed(&self, provider: ProviderId, key: &str) -> Result<(), KeystoreError> {
        let key = key.trim();
        if key.is_empty() {
            return Err(KeystoreError::EmptyKey);
        }
        self.store
            .set(SERVICE, account(provider)?, key)
            .map_err(KeystoreError::Store)
    }

    pub fn get_key_typed(&self, provider: ProviderId) -> Result<Option<String>, KeystoreError> {
        self.store
            .get(SERVICE, account(provider)?)
            .map_err(KeystoreError::Store)
    }

    /// Idempotent: deleting a key that does not exist succeeds.
    pub fn delete_key_typed(&self, provider: ProviderId) -> Result<(), KeystoreError> {
        self.store
            .delete(SERVICE, account(provider)?)
            .map_err(KeystoreError::Store)
    }

    pub fn has_key_typed(&self, provider: ProviderId) -> Result<bool, KeystoreError> {
        Ok(self
            .get_key_typed(provider)?
            .is_some_and(|key| !key.trim().is_empty()))
    }

    pub fn key_presence_typed(&self) -> Result<KeyPresence, KeystoreError> {
        Ok(KeyPresence {
            openai: self.has_key_typed(ProviderId::Openai)?,
            groq: self.has_key_typed(ProviderId::Groq)?,
        })
    }
}

/// Production keystore backed by the OS credential store.
pub fn platform() -> Keystore<PlatformStore> {
    Keystore::new(PlatformStore)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemoryStore {
        entries: Mutex<HashMap<(String, String), String>>,
    }

    impl CredentialStore for MemoryStore {
        fn get(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<String>, CredentialStoreError> {
            Ok(self
                .entries
                .lock()
                .get(&(service.to_string(), account.to_string()))
                .cloned())
        }
        fn set(
            &self,
            service: &str,
            account: &str,
            value: &str,
        ) -> Result<(), CredentialStoreError> {
            self.entries.lock().insert(
                (service.to_string(), account.to_string()),
                value.to_string(),
            );
            Ok(())
        }
        fn delete(&self, service: &str, account: &str) -> Result<(), CredentialStoreError> {
            self.entries
                .lock()
                .remove(&(service.to_string(), account.to_string()));
            Ok(())
        }
    }

    fn memory() -> Keystore<MemoryStore> {
        Keystore::new(MemoryStore::default())
    }

    #[test]
    fn set_get_delete_roundtrip() {
        let ks = memory();
        assert_eq!(ks.get_key_typed(ProviderId::Openai).unwrap(), None);

        ks.set_key_typed(ProviderId::Openai, "sk-test-123").unwrap();
        assert_eq!(
            ks.get_key_typed(ProviderId::Openai).unwrap().as_deref(),
            Some("sk-test-123")
        );
        assert!(ks.has_key_typed(ProviderId::Openai).unwrap());

        ks.delete_key_typed(ProviderId::Openai).unwrap();
        assert_eq!(ks.get_key_typed(ProviderId::Openai).unwrap(), None);
        assert!(!ks.has_key_typed(ProviderId::Openai).unwrap());
    }

    #[test]
    fn provider_accounts_are_exact_distinct_and_route_neutral() {
        assert_eq!(account(ProviderId::Openai), Ok("openai_api_key"));
        assert_eq!(account(ProviderId::Groq), Ok("groq_api_key"));
        assert_eq!(
            account(ProviderId::Local),
            Err(KeystoreError::UnsupportedProvider)
        );

        let source = include_str!("keystore.rs");
        for forbidden_route_account in [
            ["live_", "api_key"].concat(),
            ["realtime_", "api_key"].concat(),
            ["transcribe_", "api_key"].concat(),
            ["completed_", "api_key"].concat(),
        ] {
            assert!(
                !source.contains(&forbidden_route_account),
                "credential persistence must remain provider-scoped, not route-scoped: {forbidden_route_account}"
            );
        }
    }

    #[test]
    fn provider_credentials_never_cross_read_or_cross_delete() {
        let ks = memory();
        ks.set_key_typed(ProviderId::Openai, "openai-secret")
            .unwrap();
        ks.set_key_typed(ProviderId::Groq, "groq-secret").unwrap();

        assert_eq!(
            ks.get_key_typed(ProviderId::Openai).unwrap().as_deref(),
            Some("openai-secret")
        );
        assert_eq!(
            ks.get_key_typed(ProviderId::Groq).unwrap().as_deref(),
            Some("groq-secret")
        );

        ks.delete_key_typed(ProviderId::Openai).unwrap();
        assert_eq!(ks.get_key_typed(ProviderId::Openai).unwrap(), None);
        assert_eq!(
            ks.get_key_typed(ProviderId::Groq).unwrap().as_deref(),
            Some("groq-secret")
        );

        ks.set_key_typed(ProviderId::Openai, "openai-secret-2")
            .unwrap();
        ks.delete_key_typed(ProviderId::Groq).unwrap();
        assert_eq!(ks.get_key_typed(ProviderId::Groq).unwrap(), None);
        assert_eq!(
            ks.get_key_typed(ProviderId::Openai).unwrap().as_deref(),
            Some("openai-secret-2")
        );
    }

    #[test]
    fn keys_are_trimmed_and_empty_rejected() {
        let ks = memory();
        assert_eq!(
            ks.set_key_typed(ProviderId::Groq, "   "),
            Err(KeystoreError::EmptyKey)
        );
        ks.set_key_typed(ProviderId::Groq, "  gsk-abc\n").unwrap();
        assert_eq!(
            ks.get_key_typed(ProviderId::Groq).unwrap().as_deref(),
            Some("gsk-abc")
        );
    }

    #[test]
    fn delete_is_idempotent() {
        let ks = memory();
        ks.delete_key_typed(ProviderId::Groq).unwrap();
        ks.delete_key_typed(ProviderId::Groq).unwrap();
    }

    #[test]
    fn local_provider_has_no_key() {
        let ks = memory();
        assert_eq!(
            ks.set_key_typed(ProviderId::Local, "x"),
            Err(KeystoreError::UnsupportedProvider)
        );
        assert_eq!(
            ks.get_key_typed(ProviderId::Local),
            Err(KeystoreError::UnsupportedProvider)
        );
        assert_eq!(
            ks.delete_key_typed(ProviderId::Local),
            Err(KeystoreError::UnsupportedProvider)
        );
        assert_eq!(
            ks.has_key_typed(ProviderId::Local),
            Err(KeystoreError::UnsupportedProvider)
        );
    }

    #[test]
    fn key_presence_reflects_store_without_exposing_material() {
        let ks = memory();
        ks.set_key_typed(ProviderId::Openai, "sk-test-123").unwrap();
        let presence = ks.key_presence_typed().unwrap();
        assert!(presence.openai);
        assert!(!presence.groq);
    }

    /// Real OS credential store roundtrip. Uses a dedicated test service so
    /// nothing user-facing is touched; cleans up after itself. Run explicitly:
    /// `cargo test -- --ignored`
    #[test]
    #[ignore = "touches the real OS credential store"]
    fn platform_store_roundtrip() {
        let service = "portusechoes-test";
        let account = "openai_api_key";

        PlatformStore.delete(service, account).unwrap();
        assert_eq!(PlatformStore.get(service, account).unwrap(), None);

        PlatformStore
            .set(service, account, "sk-roundtrip-test")
            .unwrap();
        assert_eq!(
            PlatformStore.get(service, account).unwrap().as_deref(),
            Some("sk-roundtrip-test")
        );

        PlatformStore.delete(service, account).unwrap();
        assert_eq!(PlatformStore.get(service, account).unwrap(), None);
    }
}
