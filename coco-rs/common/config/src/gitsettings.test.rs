use super::*;

fn settings_with(include: Option<bool>) -> Settings {
    Settings {
        include_git_instructions: include,
        ..Settings::default()
    }
}

fn empty_env() -> EnvSnapshot {
    EnvSnapshot::from_pairs(Vec::<(EnvKey, String)>::new())
}

fn env_with(value: &str) -> EnvSnapshot {
    EnvSnapshot::from_pairs(vec![(
        EnvKey::CocoDisableGitInstructions,
        value.to_string(),
    )])
}

#[test]
fn defaults_to_true_when_unset() {
    assert!(should_include_git_instructions(
        &settings_with(None),
        &empty_env()
    ));
}

#[test]
fn setting_false_disables() {
    assert!(!should_include_git_instructions(
        &settings_with(Some(false)),
        &empty_env()
    ));
}

#[test]
fn env_truthy_overrides_setting_true() {
    assert!(!should_include_git_instructions(
        &settings_with(Some(true)),
        &env_with("1")
    ));
}

#[test]
fn env_falsy_overrides_setting_false() {
    assert!(should_include_git_instructions(
        &settings_with(Some(false)),
        &env_with("0")
    ));
}
