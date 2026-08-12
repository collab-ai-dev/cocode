//! Bounded, opt-in extra TLS roots for coco HTTP clients.
//!
//! [`ENV_COCO_EXTRA_CA_BUNDLE`] points at a PEM bundle. Usable certificates
//! are validated once with rustls and added to reqwest's built-in roots.

use std::io::Read;
use std::sync::OnceLock;

use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

/// Environment variable containing the path to an additive PEM CA bundle.
pub const ENV_COCO_EXTRA_CA_BUNDLE: &str = "COCO_EXTRA_CA_BUNDLE";

/// Maximum bytes read from an extra CA bundle.
pub const MAX_EXTRA_CA_BUNDLE_BYTES: u64 = 1024 * 1024;

/// Build a reqwest client builder with the configured extra roots applied.
pub fn client_builder() -> reqwest::ClientBuilder {
    with_extra_root_certificates(reqwest::Client::builder())
}

/// Build a default reqwest client with the configured extra roots applied.
///
/// Callers with an existing error boundary should use [`client_builder`] and
/// propagate `build` failures. This convenience preserves
/// [`reqwest::Client::new`]'s infallible public contract; if the customized
/// builder unexpectedly fails, it warns and falls back to reqwest defaults.
pub fn client() -> reqwest::Client {
    match client_builder().build() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                %error,
                "HTTP client with extra roots failed to build; using reqwest defaults"
            );
            reqwest::Client::new()
        }
    }
}

/// Add the configured extra roots to an existing reqwest builder.
pub fn with_extra_root_certificates(mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    for der in extra_root_ders() {
        match reqwest::Certificate::from_der(der) {
            Ok(certificate) => builder = builder.add_root_certificate(certificate),
            Err(error) => tracing::warn!(
                %error,
                "validated extra CA was rejected by reqwest; skipping certificate"
            ),
        }
    }
    builder
}

/// Add the configured extra roots to an existing blocking reqwest builder.
pub fn with_extra_root_certificates_blocking(
    mut builder: reqwest::blocking::ClientBuilder,
) -> reqwest::blocking::ClientBuilder {
    for der in extra_root_ders() {
        match reqwest::Certificate::from_der(der) {
            Ok(certificate) => builder = builder.add_root_certificate(certificate),
            Err(error) => tracing::warn!(
                %error,
                "validated extra CA was rejected by reqwest; skipping certificate"
            ),
        }
    }
    builder
}

/// Process-wide extra roots as validated DER.
///
/// The empty slice means the environment variable was unset or the configured
/// bundle produced no usable certificates.
pub fn extra_root_ders() -> &'static [Vec<u8>] {
    static ROOTS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    ROOTS.get_or_init(load_extra_root_ders).as_slice()
}

fn load_extra_root_ders() -> Vec<Vec<u8>> {
    let path = match std::env::var_os(ENV_COCO_EXTRA_CA_BUNDLE) {
        Some(path) if !path.is_empty() => std::path::PathBuf::from(path),
        _ => return Vec::new(),
    };

    let bytes = match read_bundle_capped(&path) {
        Ok(bytes) => bytes,
        Err(BundleReadError::Io(error)) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "extra CA bundle is unreadable; continuing with built-in roots"
            );
            return Vec::new();
        }
        Err(BundleReadError::TooLarge) => {
            tracing::warn!(
                path = %path.display(),
                max_bytes = MAX_EXTRA_CA_BUNDLE_BYTES,
                "extra CA bundle exceeds the size cap; continuing with built-in roots"
            );
            return Vec::new();
        }
    };

    let outcome = parse_and_validate_pem(&bytes);
    if outcome.no_pem_blocks {
        tracing::warn!(
            path = %path.display(),
            "extra CA bundle contains no PEM certificates; continuing with built-in roots"
        );
        return Vec::new();
    }
    if outcome.rejected > 0 {
        tracing::warn!(
            path = %path.display(),
            accepted = outcome.accepted.len(),
            rejected = outcome.rejected,
            "extra CA bundle contained unusable certificates"
        );
    }
    if outcome.accepted.is_empty() {
        tracing::warn!(
            path = %path.display(),
            "extra CA bundle produced no usable certificates; continuing with built-in roots"
        );
    } else {
        tracing::info!(
            path = %path.display(),
            accepted = outcome.accepted.len(),
            "loaded extra TLS root certificates"
        );
    }
    outcome.accepted
}

#[derive(Debug)]
enum BundleReadError {
    Io(std::io::Error),
    TooLarge,
}

fn read_bundle_capped(path: &std::path::Path) -> Result<Vec<u8>, BundleReadError> {
    let file = std::fs::File::open(path).map_err(BundleReadError::Io)?;
    let mut bytes = Vec::new();
    let count = file
        .take(MAX_EXTRA_CA_BUNDLE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(BundleReadError::Io)?;
    if count as u64 > MAX_EXTRA_CA_BUNDLE_BYTES {
        return Err(BundleReadError::TooLarge);
    }
    Ok(bytes)
}

#[derive(Debug, Default)]
struct ParseOutcome {
    accepted: Vec<Vec<u8>>,
    rejected: usize,
    no_pem_blocks: bool,
}

fn parse_and_validate_pem(pem: &[u8]) -> ParseOutcome {
    let mut accepted = Vec::new();
    let mut rejected = 0;
    let mut saw_block = false;
    let mut roots = RootCertStore::empty();

    for item in CertificateDer::pem_slice_iter(pem) {
        saw_block = true;
        match item {
            Ok(der) => match roots.add(der.clone()) {
                Ok(()) => accepted.push(der.as_ref().to_vec()),
                Err(_) => rejected += 1,
            },
            Err(_) => rejected += 1,
        }
    }

    ParseOutcome {
        accepted,
        rejected,
        no_pem_blocks: !saw_block,
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
