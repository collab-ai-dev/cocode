use super::*;

#[test]
fn parse_qualified() {
    let id = PluginId::parse("foo@market");
    assert_eq!(id.name, "foo");
    assert_eq!(id.marketplace.as_deref(), Some("market"));
}

#[test]
fn parse_bare() {
    let id = PluginId::parse("foo");
    assert_eq!(id.name, "foo");
    assert!(id.marketplace.is_none());
}

#[test]
fn display_roundtrip() {
    assert_eq!(PluginId::parse("foo@bar").to_string(), "foo@bar");
    assert_eq!(PluginId::parse("foo").to_string(), "foo");
}

#[test]
fn builtin_detection() {
    assert!(PluginId::parse("foo@builtin").is_builtin());
    assert!(!PluginId::parse("foo@market").is_builtin());
    assert!(!PluginId::parse("foo").is_builtin());
}

#[test]
fn inline_detection() {
    assert!(PluginId::parse("foo@inline").is_inline());
    assert!(!PluginId::parse("foo@market").is_inline());
}

#[test]
fn scope_priority_order() {
    assert!(PluginScope::Managed > PluginScope::User);
    assert!(PluginScope::User > PluginScope::Project);
    assert!(PluginScope::Project > PluginScope::Local);
}
