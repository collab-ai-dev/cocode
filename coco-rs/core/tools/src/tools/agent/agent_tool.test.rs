use super::*;
use pretty_assertions::assert_eq;

// ── Implicit-team resolution: the session owns one team; the team_name
//    PARAM is never consulted. ──

#[test]
fn resolve_implicit_team_returns_ctx_team_when_enabled() {
    assert_eq!(
        resolve_implicit_team(true, Some("session-abcd1234")),
        Some("session-abcd1234".to_string())
    );
}

#[test]
fn resolve_implicit_team_none_when_teams_disabled() {
    // Even with an active team context, AgentTeams off ⇒ no implicit team.
    assert_eq!(resolve_implicit_team(false, Some("session-abcd1234")), None);
}

#[test]
fn resolve_implicit_team_none_when_no_team_context() {
    assert_eq!(resolve_implicit_team(true, None), None);
}

// ── Routing pivot: `{name, team_name:"whatever"}` routes identically to
//    `{name}` — both are team spawns when an implicit team exists, because
//    the team_name param is ignored and only the runtime team context drives
//    routing. ──

#[test]
fn team_name_param_is_ignored_routing_keys_on_implicit_team() {
    let implicit = resolve_implicit_team(true, Some("session-abcd1234"));
    // `{name}` only — no team_name param at all.
    let with_name_only = classify_team_spawn(implicit.as_deref(), Some("alice"), false);
    // `{name, team_name:"whatever"}` — the param is never threaded into the
    // classification, so the outcome is identical.
    let with_name_and_param = classify_team_spawn(implicit.as_deref(), Some("alice"), false);
    assert!(with_name_only);
    assert_eq!(with_name_only, with_name_and_param);
}

#[test]
fn team_name_alone_no_name_is_not_a_team_spawn() {
    // A spawn carrying only a team_name (no `name`) does NOT route to a
    // teammate spawn. The implicit team is present, but there is no name.
    let implicit = resolve_implicit_team(true, Some("session-abcd1234"));
    assert!(!classify_team_spawn(implicit.as_deref(), None, false));
}

#[test]
fn no_implicit_team_means_no_team_spawn() {
    // With no implicit team (AgentTeams off or no team context), a named
    // spawn is an ordinary subagent, never a teammate.
    assert!(!classify_team_spawn(None, Some("alice"), false));
}

#[test]
fn fork_spawn_excludes_team_spawn() {
    // A named spawn while fork mode is active routes as a fork, not a
    // teammate (CC orders the team branch on `!isFork`).
    let implicit = resolve_implicit_team(true, Some("session-abcd1234"));
    assert!(!classify_team_spawn(
        implicit.as_deref(),
        Some("alice"),
        true
    ));
}
