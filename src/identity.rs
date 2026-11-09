//! Per-run TLS identity; no certificates, pairing codes, or user data are persisted.
use anyhow::Result;
use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    ClientConfig, DigitallySignedStruct, Error, ServerConfig, SignatureScheme,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub struct Identity {
    pub config: Arc<ServerConfig>,
    pub fingerprint: String,
}

pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|n| format!("{n:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn new_identity() -> Result<Identity> {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["linkdesk.local".into()])?;
    let der = cert.der().clone();
    let fingerprint = fingerprint(der.as_ref());
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![der],
            PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
        )?;
    Ok(Identity {
        config: Arc::new(config),
        fingerprint,
    })
}

// This verifier pins the exact certificate fingerprint supplied by the user.
// Only the probe connection uses None: it transmits no PIN/name/video and must
// be followed by explicit fingerprint confirmation before a new pinned session.
#[derive(Debug)]
struct PinnedVerifier {
    expected: Option<String>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _chain: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, Error> {
        if let Some(expected) = &self.expected {
            if &fingerprint(cert.as_ref()) != expected {
                return Err(Error::General(
                    "host fingerprint changed; pair again".into(),
                ));
            }
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn client_config(expected: Option<String>) -> Arc<ClientConfig> {
    Arc::new(
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedVerifier { expected }))
            .with_no_client_auth(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fingerprint_is_stable_and_identities_are_ephemeral() {
        assert_eq!(fingerprint(b"test"), fingerprint(b"test"));
        assert_ne!(
            new_identity().unwrap().fingerprint,
            new_identity().unwrap().fingerprint
        );
    }
}
