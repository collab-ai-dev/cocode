use super::*;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::ModelCatalogEntry;
use crate::state::ProviderStatus;
use crate::state::ProviderUnavailableReason;
use coco_types::ModelRole;
use coco_types::ReasoningEffort;

fn catalog_entry(provider: &str, provider_display: &str, model_id: &str) -> ModelCatalogEntry {
    ModelCatalogEntry {
        provider: provider.to_string(),
        provider_display: provider_display.to_string(),
        model_id: model_id.to_string(),
        display_name: model_id.to_string(),
        context_window: Some(200_000),
        supported_efforts: vec![ReasoningEffort::Auto],
        default_effort: Some(ReasoningEffort::Auto),
    }
}

fn seed_catalog(state: &mut AppState) {
    state.session.model_catalog = vec![
        catalog_entry("anthropic", "Anthropic", "claude-sonnet-4-6"),
        catalog_entry("openai", "OpenAI", "gpt-5-4"),
    ];
}

fn teammate_row(name: &str, team: &str, agent_type: &str) -> crate::state::SubagentInstance {
    crate::state::SubagentInstance {
        kind: crate::state::SubagentKind::Teammate,
        agent_id: format!("{name}@{team}"),
        agent_type: agent_type.into(),
        description: String::new(),
        status: crate::state::SubagentStatus::Running,
        color: None,
        team_name: Some(team.into()),
        started_at_ms: None,
        last_tool_name: None,
        tool_count: 0,
        total_tokens: 0,
        is_backgrounded: false,
        recent_activities: Vec::new(),
        final_message: None,
        completed_at_ms: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cost_usd: 0.0,
    }
}

/// `team_roster` (gap 8) lists only running teammates from
/// `session.subagents`, excluding plain subagents, and uses the bare name
/// (not `name@team`) as the `set_teammate_mode` target.
#[test]
fn team_roster_lists_only_running_teammates() {
    let mut state = AppState::new();
    state
        .session
        .subagents
        .push(teammate_row("researcher", "my-team", "explore"));
    let mut plain = teammate_row("worker", "my-team", "build");
    plain.kind = crate::state::SubagentKind::Subagent; // not a teammate
    state.session.subagents.push(plain);

    team_roster(&mut state);

    let r = match state.ui.modal.as_ref() {
        Some(ModalState::TeamRoster(r)) => r.clone(),
        _ => panic!("expected TeamRoster modal"),
    };
    assert_eq!(r.team_name, "my-team");
    assert_eq!(r.members.len(), 1, "plain subagents must be excluded");
    assert_eq!(r.members[0].name, "researcher");
    assert_eq!(r.members[0].agent_type, "explore");
    // No team.json in this unit context ⇒ the member falls back to `Default`
    // (mirrors `permissionModeFromString(undefined)`). The seed-from-team.json
    // path — picker reflecting a STORED non-default mode — is exercised at the
    // coordinator layer (`team_file` round-trip) rather than here, where
    // constructing a `TeamFile` would cross the module-privacy boundary.
    assert_eq!(r.members[0].mode, coco_types::PermissionMode::Default);
}

/// `cycle_model` opens the picker for the Main role from the
/// session-frozen model catalog (provider-grouped because the seeder
/// sorts on provider_display).
#[test]
fn cycle_model_uses_session_model_catalog() {
    let mut state = AppState::new();
    seed_catalog(&mut state);
    state.session.model = "claude-sonnet-4-6".to_string();
    state.session.provider = "anthropic".to_string();
    cycle_model(&mut state);
    let m = match state.ui.modal.as_ref() {
        Some(ModalState::ModelPicker(m)) => m.clone(),
        other => panic!("expected ModelPicker, got {other:?}"),
    };
    assert_eq!(m.role, ModelRole::Main);
    assert!(!m.entries.is_empty(), "picker should have builtin entries");
    let providers: Vec<&str> = m.entries.iter().map(|e| e.provider.as_str()).collect();
    assert!(providers.contains(&"anthropic"));
    assert!(providers.contains(&"openai"));
    // The current Main model is marked.
    let current = m.entries.iter().find(|e| e.is_current_for_role).unwrap();
    assert_eq!(current.model_id, "claude-sonnet-4-6");
}

#[test]
fn cycle_model_marks_provider_config_unavailable() {
    let mut state = AppState::new();
    seed_catalog(&mut state);
    state.session.provider_statuses.insert(
        "openai".to_string(),
        ProviderStatus {
            provider_display: "OpenAI".to_string(),
            unavailable_reasons: vec![ProviderUnavailableReason::MissingApiKey {
                env_key: "OPENAI_API_KEY".to_string(),
            }],
        },
    );

    cycle_model(&mut state);
    let m = match state.ui.modal.as_ref() {
        Some(ModalState::ModelPicker(m)) => m,
        other => panic!("expected ModelPicker, got {other:?}"),
    };
    let openai = m.entries.iter().find(|e| e.provider == "openai").unwrap();
    assert_eq!(
        openai.unavailable_reasons,
        vec![ProviderUnavailableReason::MissingApiKey {
            env_key: "OPENAI_API_KEY".to_string()
        }]
    );
}

#[test]
fn cycle_model_adds_unavailable_provider_without_models() {
    let mut state = AppState::new();
    seed_catalog(&mut state);
    state.session.provider_statuses.insert(
        "custom".to_string(),
        ProviderStatus {
            provider_display: "Custom".to_string(),
            unavailable_reasons: Vec::new(),
        },
    );

    cycle_model(&mut state);
    let m = match state.ui.modal.as_ref() {
        Some(ModalState::ModelPicker(m)) => m,
        other => panic!("expected ModelPicker, got {other:?}"),
    };
    let custom = m.entries.iter().find(|e| e.provider == "custom").unwrap();
    assert!(custom.model_id.is_empty());
    assert_eq!(
        custom.unavailable_reasons,
        vec![ProviderUnavailableReason::NoModels]
    );
}

/// Role cycle wraps via Tab/Shift+Tab over the canonical 9-role order
/// and rebuilds the entries each time (clearing the filter).
#[test]
fn cycle_model_role_wraps_and_resets_filter() {
    let mut state = AppState::new();
    seed_catalog(&mut state);
    state.session.model = "claude-sonnet-4-6".to_string();
    state.session.provider = "anthropic".to_string();
    cycle_model(&mut state);
    if let Some(ModalState::ModelPicker(m)) = state.ui.modal.as_mut() {
        m.filter = "ignored".to_string();
    }
    cycle_model_role(&mut state, 1);
    let m = match state.ui.modal.as_ref() {
        Some(ModalState::ModelPicker(m)) => m,
        _ => unreachable!(),
    };
    assert_eq!(m.role, ModelRole::Fast);
    assert!(m.filter.is_empty(), "Tab should reset filter");

    // Wrap-around: from Main, Shift+Tab (delta = -1) goes to Subagent.
    cycle_model_role(&mut state, -1); // Fast → Main
    cycle_model_role(&mut state, -1); // Main → Subagent (wrap)
    let m = match state.ui.modal.as_ref() {
        Some(ModalState::ModelPicker(m)) => m,
        _ => unreachable!(),
    };
    assert_eq!(m.role, ModelRole::Subagent);
}

#[test]
fn build_model_entries_empty_catalog_has_no_prefix_inference() {
    let mut state = AppState::new();
    state.session.model = "claude-sonnet-4-6".to_string();
    state.session.provider = "anthropic".to_string();

    let entries = build_model_entries(&state, ModelRole::Main);

    assert!(entries.is_empty());
}

/// `next_role` cycles deterministically over the canonical order and
/// stays consistent with the state content adapter (see surface_content/pickers.rs).
#[test]
fn next_role_cycles_canonical_order() {
    assert_eq!(next_role(ModelRole::Main, 1), ModelRole::Fast);
    assert_eq!(next_role(ModelRole::Subagent, 1), ModelRole::Main);
    assert_eq!(next_role(ModelRole::Main, -1), ModelRole::Subagent);
    assert_eq!(next_role(ModelRole::Plan, 2), ModelRole::Review);
}

#[test]
fn build_model_entries_applies_available_model_allowlist_to_catalog() {
    let mut state = AppState::new();
    seed_catalog(&mut state);
    state.session.available_models = Some(vec!["gpt-5-4".to_string()]);

    let entries = build_model_entries(&state, ModelRole::Main);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].provider, "openai");
    assert_eq!(entries[0].model_id, "gpt-5-4");
}
