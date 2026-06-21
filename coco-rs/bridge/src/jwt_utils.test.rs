use super::Claims;
use super::JwtError;
use super::sign;
use super::verify;

const SECRET: &[u8] = b"test-secret-32-bytes-long-material!";

#[test]
fn round_trips_claims() {
    let claims = Claims::new(3600)
        .sub("user@example.com")
        .aud("coco-bridge")
        .workspace("/tmp/ws")
        .nonce("n1");

    let token = sign(&claims, SECRET);
    let decoded = verify(&token, SECRET).unwrap();
    assert_eq!(decoded, claims);
}

#[test]
fn tampered_signature_rejected() {
    let token = sign(&Claims::new(3600), SECRET);
    let mut tampered = token;
    // Flip last char
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'A' { 'B' } else { 'A' });
    assert_eq!(verify(&tampered, SECRET), Err(JwtError::BadSignature));
}

#[test]
fn wrong_secret_rejected() {
    let token = sign(&Claims::new(3600), SECRET);
    assert_eq!(
        verify(&token, b"different-secret"),
        Err(JwtError::BadSignature)
    );
}

#[test]
fn malformed_token_rejected() {
    assert_eq!(verify("", SECRET), Err(JwtError::Malformed));
    assert_eq!(verify("a.b", SECRET), Err(JwtError::Malformed));
    assert_eq!(verify("a.b.c.d", SECRET), Err(JwtError::Malformed));
}

#[test]
fn expired_token_rejected() {
    let claims = Claims::new(-10); // expired 10 seconds ago
    let token = sign(&claims, SECRET);
    matches!(verify(&token, SECRET), Err(JwtError::Expired { .. }));
}
