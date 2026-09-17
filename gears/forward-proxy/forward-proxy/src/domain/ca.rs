use toolkit_macros::domain_model;

use crate::domain::error::DomainError;

/// PEM-encoded root CA material: the cert (installed on client machines so
/// they trust the leaf certificates `forward-proxy` mints on the fly) and
/// the private key (used to sign those leaves). Plain strings — no crypto
/// library types — so this stays a valid domain model; actual key/cert
/// generation and signing are infra concerns (`infra::ca_rcgen`).
#[domain_model]
#[derive(Debug, Clone)]
pub struct CaMaterial {
    pub cert_pem: String,
    pub key_pem: String,
}

/// Port for persisting/loading [`CaMaterial`] across restarts, so
/// `forward-proxy` mints leaf certs under the *same* root CA every time it
/// starts — installing a fresh CA on every restart would be a terrible
/// operator experience. Implemented by `infra::ca_store_fs` (filesystem).
pub trait CaStore: Send + Sync {
    /// Returns previously persisted material, if any.
    fn load(&self) -> Result<Option<CaMaterial>, DomainError>;

    /// Persists material so a later `load()` returns it.
    fn save(&self, material: &CaMaterial) -> Result<(), DomainError>;
}

/// Port for generating fresh [`CaMaterial`] when none is persisted yet.
/// Implemented by `infra::ca_rcgen` (uses the `rcgen` crate — the same one
/// hudsucker's own `RcgenAuthority` builds on).
pub trait CaGenerator: Send + Sync {
    fn generate(&self) -> Result<CaMaterial, DomainError>;
}

/// Loads persisted CA material, generating and persisting new material on
/// first run. Pure orchestration over the two ports above — no crypto, no
/// filesystem — so this is unit-testable with fakes (see tests below).
pub fn load_or_generate(
    store: &dyn CaStore,
    generator: &dyn CaGenerator,
) -> Result<CaMaterial, DomainError> {
    if let Some(material) = store.load()? {
        return Ok(material);
    }
    let material = generator.generate()?;
    store.save(&material)?;
    Ok(material)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct FakeStore {
        material: Mutex<Option<CaMaterial>>,
        save_calls: Mutex<u32>,
    }

    impl CaStore for FakeStore {
        fn load(&self) -> Result<Option<CaMaterial>, DomainError> {
            Ok(self
                .material
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        }

        fn save(&self, material: &CaMaterial) -> Result<(), DomainError> {
            *self
                .material
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(material.clone());
            *self
                .save_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            Ok(())
        }
    }

    struct FakeGenerator {
        generate_calls: Mutex<u32>,
    }

    impl CaGenerator for FakeGenerator {
        fn generate(&self) -> Result<CaMaterial, DomainError> {
            *self
                .generate_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            Ok(CaMaterial {
                cert_pem: "generated-cert".to_owned(),
                key_pem: "generated-key".to_owned(),
            })
        }
    }

    #[test]
    fn returns_persisted_material_without_generating() {
        let store = FakeStore {
            material: Mutex::new(Some(CaMaterial {
                cert_pem: "existing-cert".to_owned(),
                key_pem: "existing-key".to_owned(),
            })),
            save_calls: Mutex::new(0),
        };
        let generator = FakeGenerator {
            generate_calls: Mutex::new(0),
        };

        let material = load_or_generate(&store, &generator).expect("should succeed");

        assert_eq!(material.cert_pem, "existing-cert");
        assert_eq!(
            *generator
                .generate_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            0,
            "should not generate"
        );
        assert_eq!(
            *store
                .save_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            0,
            "should not re-save"
        );
    }

    #[test]
    fn generates_and_persists_when_nothing_stored() {
        let store = FakeStore {
            material: Mutex::new(None),
            save_calls: Mutex::new(0),
        };
        let generator = FakeGenerator {
            generate_calls: Mutex::new(0),
        };

        let material = load_or_generate(&store, &generator).expect("should succeed");

        assert_eq!(material.cert_pem, "generated-cert");
        assert_eq!(
            *generator
                .generate_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            1
        );
        assert_eq!(
            *store
                .save_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            1
        );
    }
}
