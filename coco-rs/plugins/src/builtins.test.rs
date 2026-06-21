use super::*;
use std::sync::Mutex;
use std::sync::OnceLock;

/// All `builtins::tests` mutate a single process-wide registry. Without a
/// guard, parallel cargo-test workers race and cause flakes. Each test
/// acquires this mutex first, then calls `fresh_registry()`.
fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn fresh_registry() {
    clear_builtin_plugins();
}

#[test]
fn register_and_lookup() {
    let _guard = test_lock();
    fresh_registry();
    register_builtin_plugin(BuiltinPluginDefinition {
        name: "core-tools".into(),
        description: "Core toolset".into(),
        version: Some("1.0.0".into()),
        default_enabled: true,
        is_available: None,
        skills: vec![],
        hooks: None,
        mcp_servers: HashMap::new(),
    });

    let def = get_builtin_plugin_definition("core-tools").unwrap();
    assert_eq!(def.name, "core-tools");
    assert!(def.default_enabled);
}

#[test]
fn is_builtin_plugin_id_check() {
    assert!(is_builtin_plugin_id("foo@builtin"));
    assert!(!is_builtin_plugin_id("foo@market"));
    assert!(!is_builtin_plugin_id("foo"));
}

#[test]
fn enabled_state_resolves_correctly() {
    let _guard = test_lock();
    fresh_registry();
    register_builtin_plugin(BuiltinPluginDefinition {
        name: "default-on".into(),
        description: "On by default".into(),
        version: None,
        default_enabled: true,
        is_available: None,
        skills: vec![],
        hooks: None,
        mcp_servers: HashMap::new(),
    });
    register_builtin_plugin(BuiltinPluginDefinition {
        name: "default-off".into(),
        description: "Off by default".into(),
        version: None,
        default_enabled: false,
        is_available: None,
        skills: vec![],
        hooks: None,
        mcp_servers: HashMap::new(),
    });

    // No overrides — defaults apply.
    let no_overrides = HashMap::new();
    let (enabled, disabled) = get_builtin_plugins(&no_overrides);
    assert!(enabled.iter().any(|p| p.name == "default-on"));
    assert!(disabled.iter().any(|p| p.name == "default-off"));

    // User override — flip both.
    let mut overrides = HashMap::new();
    overrides.insert("default-on@builtin".into(), false);
    overrides.insert("default-off@builtin".into(), true);
    let (enabled, disabled) = get_builtin_plugins(&overrides);
    assert!(enabled.iter().any(|p| p.name == "default-off"));
    assert!(disabled.iter().any(|p| p.name == "default-on"));
}

#[test]
fn unavailable_plugin_omitted_entirely() {
    let _guard = test_lock();
    fresh_registry();
    register_builtin_plugin(BuiltinPluginDefinition {
        name: "missing-os".into(),
        description: "Only on Mars".into(),
        version: None,
        default_enabled: true,
        is_available: Some(|| false),
        skills: vec![],
        hooks: None,
        mcp_servers: HashMap::new(),
    });
    let (enabled, disabled) = get_builtin_plugins(&HashMap::new());
    assert!(enabled.is_empty());
    assert!(disabled.is_empty());
}
