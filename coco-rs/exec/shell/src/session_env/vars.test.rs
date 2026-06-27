use super::*;

#[test]
fn set_get_delete() {
    let v = SessionEnvVars::new();
    assert!(v.is_empty());
    v.set("FOO", "bar");
    assert!(!v.is_empty());
    let snap = v.snapshot();
    assert_eq!(snap.get("FOO").map(String::as_str), Some("bar"));

    assert_eq!(v.delete("FOO").as_deref(), Some("bar"));
    assert!(v.is_empty());
}

#[test]
fn clear_drops_all() {
    let v = SessionEnvVars::new();
    v.set("A", "1");
    v.set("B", "2");
    v.clear();
    assert!(v.is_empty());
}

#[test]
fn clones_share_storage() {
    let a = SessionEnvVars::new();
    let b = a.clone();
    a.set("X", "1");
    assert_eq!(b.snapshot().get("X").map(String::as_str), Some("1"));
}
