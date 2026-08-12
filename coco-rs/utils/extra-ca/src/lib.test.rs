use super::*;

const VALID_CERT: &str = include_str!("../tests/fixtures/root.pem");

const INVALID_DER_CERT: &str = "-----BEGIN CERTIFICATE-----\nMAMBAf8=\n-----END CERTIFICATE-----\n";

#[test]
fn parses_and_validates_a_certificate_bundle() {
    let outcome = parse_and_validate_pem(VALID_CERT.as_bytes());

    assert_eq!(outcome.accepted.len(), 1);
    assert_eq!(outcome.rejected, 0);
    assert!(!outcome.no_pem_blocks);
}

#[test]
fn keeps_valid_certificates_and_counts_invalid_blocks() {
    let pem = format!("{VALID_CERT}\n{INVALID_DER_CERT}");
    let outcome = parse_and_validate_pem(pem.as_bytes());

    assert_eq!(outcome.accepted.len(), 1);
    assert_eq!(outcome.rejected, 1);
    assert!(!outcome.no_pem_blocks);
}

#[test]
fn non_pem_input_is_distinguished_from_rejected_certificates() {
    let outcome = parse_and_validate_pem(b"not a certificate");

    assert!(outcome.accepted.is_empty());
    assert_eq!(outcome.rejected, 0);
    assert!(outcome.no_pem_blocks);
}

#[test]
fn capped_reader_rejects_oversized_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("oversized.pem");
    std::fs::write(&path, vec![b'x'; MAX_EXTRA_CA_BUNDLE_BYTES as usize + 1])
        .expect("write fixture");

    assert!(matches!(
        read_bundle_capped(&path),
        Err(BundleReadError::TooLarge)
    ));
}

#[test]
fn validated_roots_can_build_a_reqwest_client() {
    let outcome = parse_and_validate_pem(VALID_CERT.as_bytes());
    let mut builder = reqwest::Client::builder();
    for der in outcome.accepted {
        let certificate = reqwest::Certificate::from_der(&der).expect("validated DER");
        builder = builder.add_root_certificate(certificate);
    }

    builder.build().expect("client with extra root");
}
