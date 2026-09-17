//! [`CaGenerator`] impl backed by `hudsucker`'s own re-exported `rcgen`
//! crate (not a separately-versioned `rcgen` dependency — see
//! `Cargo.toml`'s comment on the `hudsucker` dependency for why that
//! matters: this generated material must be loadable by hudsucker's own
//! `RcgenAuthority::new` via `rcgen::Issuer::from_ca_cert_pem`).

use hudsucker::rcgen::{
    BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose,
};

use crate::domain::ca::{CaGenerator, CaMaterial};
use crate::domain::error::DomainError;

pub struct RcgenCaGenerator;

impl CaGenerator for RcgenCaGenerator {
    fn generate(&self) -> Result<CaMaterial, DomainError> {
        let key_pair = KeyPair::generate().map_err(|e| {
            DomainError::ca_unavailable(format!("failed to generate key pair: {e}"))
        })?;

        let mut params = CertificateParams::new(Vec::new())
            .map_err(|e| DomainError::ca_unavailable(format!("failed to build CA params: {e}")))?;
        params
            .distinguished_name
            .push(DnType::CommonName, "forward-proxy local CA");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];

        let cert = params.self_signed(&key_pair).map_err(|e| {
            DomainError::ca_unavailable(format!("failed to self-sign CA cert: {e}"))
        })?;

        Ok(CaMaterial {
            cert_pem: cert.pem(),
            key_pem: key_pair.serialize_pem(),
        })
    }
}
