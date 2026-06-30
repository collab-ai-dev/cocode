use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_parse_memory_stores_empty_returns_empty() {
    assert!(parse_memory_stores("").is_empty());
    assert!(parse_memory_stores("   ").is_empty());
}

#[test]
fn test_parse_memory_stores_invalid_json_returns_empty() {
    assert!(parse_memory_stores("not json").is_empty());
    assert!(parse_memory_stores("{").is_empty());
}

#[test]
fn test_parse_memory_stores_bare_string_defaults() {
    let stores = try_parse_memory_stores(r#"["/mnt/team-mem"]"#).unwrap();
    assert_eq!(stores.len(), 1);
    let s = &stores[0];
    assert_eq!(s.path.as_path(), std::path::Path::new("/mnt/team-mem"));
    // default mode = rw, default scope = team
    assert_eq!(s.mode, StoreMode::Rw);
    assert_eq!(s.scope, StoreScope::Team);
    // mount derived from last segment
    assert_eq!(s.mount.as_deref(), Some("team-mem"));
    assert_eq!(s.prompt_index, None);
    assert_eq!(s.prompt_index_max_bytes, None);
}

#[test]
fn test_parse_memory_stores_object_form() {
    let json = r#"[{
        "path": "/mnt/shared",
        "mode": "ro",
        "scope": "user",
        "mount": "shared-ro",
        "promptIndex": "index/MEMORY.md",
        "promptIndexMaxBytes": 25000
    }]"#;
    let stores = try_parse_memory_stores(json).unwrap();
    assert_eq!(stores.len(), 1);
    let s = &stores[0];
    assert_eq!(s.mode, StoreMode::Ro);
    assert_eq!(s.scope, StoreScope::User);
    assert_eq!(s.mount.as_deref(), Some("shared-ro"));
    assert_eq!(s.prompt_index.as_deref(), Some("index/MEMORY.md"));
    assert_eq!(s.prompt_index_max_bytes, Some(25000));
}

#[test]
fn test_parse_memory_stores_object_defaults() {
    // Object with only `path` → mode=rw, scope=team, mount derived.
    let stores = try_parse_memory_stores(r#"[{"path": "/data/proj-mem"}]"#).unwrap();
    assert_eq!(stores.len(), 1);
    assert_eq!(stores[0].mode, StoreMode::Rw);
    assert_eq!(stores[0].scope, StoreScope::Team);
    assert_eq!(stores[0].mount.as_deref(), Some("proj-mem"));
}

#[test]
fn test_parse_memory_stores_mount_derivation_explicit_wins() {
    let stores = try_parse_memory_stores(r#"[{"path": "/a/b/c", "mount": "custom"}]"#).unwrap();
    assert_eq!(stores[0].mount.as_deref(), Some("custom"));
}

#[test]
fn test_parse_memory_stores_sanitizes_derived_mount() {
    let stores = try_parse_memory_stores(r#"["/x/team.mem"]"#).unwrap();
    assert_eq!(stores[0].mount.as_deref(), Some("team-mem"));
}

#[test]
fn test_parse_memory_stores_rejects_duplicate_mount() {
    let json = r#"["/x/team", "/y/team"]"#;
    let err = try_parse_memory_stores(json).unwrap_err();
    assert!(err.to_string().contains("duplicate mount"));
}

#[test]
fn test_parse_memory_stores_rejects_duplicate_explicit_mount() {
    let json = r#"[
        {"path": "/x/a", "mount": "m"},
        {"path": "/y/b", "mount": "m"}
    ]"#;
    let err = try_parse_memory_stores(json).unwrap_err();
    assert!(err.to_string().contains("duplicate mount"));
}

#[test]
fn test_parse_memory_stores_rejects_invalid_explicit_mount() {
    let err = try_parse_memory_stores(r#"[{"path": "/x/a", "mount": "bad.mount"}]"#).unwrap_err();
    assert!(err.to_string().contains("mount must match"));
}

#[test]
fn test_parse_memory_stores_rejects_more_than_one_user_scope() {
    let json = r#"[
        {"path": "/u/one", "scope": "user"},
        {"path": "/u/two", "scope": "user"},
        {"path": "/t/three", "scope": "team"}
    ]"#;
    let err = try_parse_memory_stores(json).unwrap_err();
    assert!(err.to_string().contains("more than one scope"));
}

#[test]
fn test_parse_memory_stores_rejects_relative_path() {
    let err = try_parse_memory_stores(r#"["relative/path", "/abs/keep"]"#).unwrap_err();
    assert!(err.to_string().contains("path-absolute"));
}

#[test]
fn test_parse_memory_stores_rejects_host_override_path() {
    let err = try_parse_memory_stores(r#"["//host/share"]"#).unwrap_err();
    assert!(err.to_string().contains("override the host"));
}

#[test]
fn test_prompt_index_path_safety_accept() {
    assert!(is_safe_relative_prompt_index("MEMORY.md"));
    assert!(is_safe_relative_prompt_index("index/team/MEMORY.md"));
    assert!(is_safe_relative_prompt_index("a-b_c.1.md"));
}

#[test]
fn test_prompt_index_path_safety_reject() {
    assert!(!is_safe_relative_prompt_index(""));
    assert!(!is_safe_relative_prompt_index("/abs/path.md"));
    assert!(!is_safe_relative_prompt_index("."));
    assert!(!is_safe_relative_prompt_index(".."));
    assert!(!is_safe_relative_prompt_index("../escape.md"));
    assert!(!is_safe_relative_prompt_index("a/../b.md"));
    assert!(!is_safe_relative_prompt_index("a//b.md"));
    assert!(!is_safe_relative_prompt_index("a/ b.md"));
    assert!(!is_safe_relative_prompt_index("a\\b.md"));
    assert!(!is_safe_relative_prompt_index("sp@ce.md"));
}

#[test]
fn test_parse_memory_stores_rejects_unsafe_prompt_index() {
    let json = r#"[{"path": "/s/store", "prompt_index": "../escape.md"}]"#;
    let err = try_parse_memory_stores(json).unwrap_err();
    assert!(err.to_string().contains("promptIndex segments"));
}

#[test]
fn test_parse_memory_stores_rejects_non_positive_prompt_index_max_bytes() {
    let json = r#"[{"path": "/s/store", "promptIndexMaxBytes": 0}]"#;
    let err = try_parse_memory_stores(json).unwrap_err();
    assert!(err.to_string().contains("promptIndexMaxBytes"));
}
