use super::SECRET_BYTES;
use super::account_name_for_workspace;
use super::decode_secret;
use super::derive_secret_from_material;
use super::generate_fresh_secret;

#[test]
fn derive_is_stable() {
    let a = derive_secret_from_material("/tmp/ws", "install-1");
    let b = derive_secret_from_material("/tmp/ws", "install-1");
    assert_eq!(a, b);
}

#[test]
fn derive_differs_when_inputs_differ() {
    let a = derive_secret_from_material("/tmp/ws-a", "install");
    let b = derive_secret_from_material("/tmp/ws-b", "install");
    assert_ne!(a, b);
    let c = derive_secret_from_material("/tmp/ws", "install-a");
    let d = derive_secret_from_material("/tmp/ws", "install-b");
    assert_ne!(c, d);
}

#[test]
fn derive_avoids_concat_collision() {
    // Without the separator, ("a","bc") and ("ab","c") would hash the
    // same bytes. The NUL between fields prevents this.
    let x = derive_secret_from_material("a", "bc");
    let y = derive_secret_from_material("ab", "c");
    assert_ne!(x, y);
}

#[test]
fn account_name_has_prefix_and_is_short() {
    let name = account_name_for_workspace("/very/long/workspace/path/with/many/levels");
    assert!(name.starts_with("work-secret:"));
    // prefix (12 chars) + 16-char hash-prefix = 28 total
    assert_eq!(name.len(), 12 + 16);
}

#[cfg(unix)]
#[test]
fn generate_fresh_returns_decoding_target_bytes() {
    let encoded = generate_fresh_secret().unwrap();
    let decoded = decode_secret(&encoded).unwrap();
    assert_eq!(decoded.len(), SECRET_BYTES);

    // Two consecutive generations should be different (entropy check).
    let another = generate_fresh_secret().unwrap();
    assert_ne!(encoded, another);
}
