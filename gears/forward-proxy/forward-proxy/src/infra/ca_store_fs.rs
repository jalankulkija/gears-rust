//! Filesystem-backed [`CaStore`]: persists the root CA cert/key as two PEM
//! files so `forward-proxy` mints leaf certificates under the same root
//! across restarts (see `domain::ca` module docs for why that matters).

use std::path::PathBuf;

use crate::domain::ca::{CaMaterial, CaStore};
use crate::domain::error::DomainError;

pub struct FsCaStore {
    cert_path: PathBuf,
    key_path: PathBuf,
}

impl FsCaStore {
    #[must_use]
    pub fn new(cert_path: PathBuf, key_path: PathBuf) -> Self {
        Self {
            cert_path,
            key_path,
        }
    }
}

impl CaStore for FsCaStore {
    fn load(&self) -> Result<Option<CaMaterial>, DomainError> {
        if !self.cert_path.exists() || !self.key_path.exists() {
            return Ok(None);
        }
        let cert_pem = std::fs::read_to_string(&self.cert_path).map_err(|e| {
            DomainError::ca_unavailable(format!(
                "failed to read CA cert at {}: {e}",
                self.cert_path.display()
            ))
        })?;
        let key_pem = std::fs::read_to_string(&self.key_path).map_err(|e| {
            DomainError::ca_unavailable(format!(
                "failed to read CA key at {}: {e}",
                self.key_path.display()
            ))
        })?;
        Ok(Some(CaMaterial { cert_pem, key_pem }))
    }

    fn save(&self, material: &CaMaterial) -> Result<(), DomainError> {
        std::fs::write(&self.cert_path, &material.cert_pem).map_err(|e| {
            DomainError::ca_unavailable(format!(
                "failed to write CA cert to {}: {e}",
                self.cert_path.display()
            ))
        })?;
        std::fs::write(&self.key_path, &material.key_pem).map_err(|e| {
            DomainError::ca_unavailable(format!(
                "failed to write CA key to {}: {e}",
                self.key_path.display()
            ))
        })?;
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn load_returns_none_when_files_missing() {
        let dir = std::env::temp_dir().join(format!("fp-ca-test-{}", uuid::Uuid::new_v4()));
        let store = FsCaStore::new(dir.join("cert.pem"), dir.join("key.pem"));
        assert!(store.load().expect("should not error").is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("fp-ca-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let store = FsCaStore::new(dir.join("cert.pem"), dir.join("key.pem"));

        let material = CaMaterial {
            cert_pem: "cert-content".to_owned(),
            key_pem: "key-content".to_owned(),
        };
        store.save(&material).expect("save should succeed");

        let loaded = store
            .load()
            .expect("load should succeed")
            .expect("should exist");
        assert_eq!(loaded.cert_pem, "cert-content");
        assert_eq!(loaded.key_pem, "key-content");

        std::fs::remove_dir_all(&dir).ok();
    }
}
