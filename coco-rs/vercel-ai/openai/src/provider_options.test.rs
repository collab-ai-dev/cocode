use std::collections::BTreeMap;

use serde_json::json;

use super::*;

#[test]
fn empty_map_is_all_defaults() {
    let cfg = parse_provider_options(&BTreeMap::new()).expect("empty is ok");
    assert_eq!(cfg.reasoning_store, ResponsesStorePolicy::ServerDefault);
}

#[test]
fn parses_stateless_reasoning_store() {
    let mut opts = BTreeMap::new();
    opts.insert("reasoning_store".to_string(), json!("stateless"));
    let cfg = parse_provider_options(&opts).expect("valid");
    assert_eq!(cfg.reasoning_store, ResponsesStorePolicy::Stateless);
}

#[test]
fn parses_server_reasoning_store() {
    let mut opts = BTreeMap::new();
    opts.insert("reasoning_store".to_string(), json!("server"));
    let cfg = parse_provider_options(&opts).expect("valid");
    assert_eq!(cfg.reasoning_store, ResponsesStorePolicy::ServerDefault);
}

#[test]
fn unknown_field_is_rejected() {
    // `deny_unknown_fields` surfaces typos at startup rather than silently
    // shipping the default.
    let mut opts = BTreeMap::new();
    opts.insert("reasoning_stor".to_string(), json!("stateless"));
    assert!(parse_provider_options(&opts).is_err());
}

#[test]
fn invalid_value_is_rejected() {
    let mut opts = BTreeMap::new();
    opts.insert("reasoning_store".to_string(), json!("bogus"));
    assert!(parse_provider_options(&opts).is_err());
}
