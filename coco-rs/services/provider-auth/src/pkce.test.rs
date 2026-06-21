use super::*;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::Digest;
use sha2::Sha256;

#[test]
fn challenge_is_s256_of_verifier() {
    let p = generate_pkce();
    let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(p.code_verifier.as_bytes()));
    assert_eq!(p.code_challenge, expected);
}

#[test]
fn pkce_and_state_are_random_and_nonempty() {
    assert_ne!(generate_pkce().code_verifier, generate_pkce().code_verifier);
    assert_ne!(generate_state(), generate_state());
    assert!(!generate_state().is_empty());
}
