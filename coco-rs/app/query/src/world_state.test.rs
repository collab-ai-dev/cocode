use super::*;
use pretty_assertions::assert_eq;

fn server(name: &str, tool_count: usize) -> McpServerSummary {
    McpServerSummary {
        name: name.to_string(),
        tool_count,
        description: None,
    }
}

fn announced(tool_count: usize) -> McpServerAnnouncementState {
    McpServerAnnouncementState {
        tool_count,
        description: None,
    }
}

fn input<'a>(
    model: &'a str,
    deferred: &'a [String],
    loaded: &'a [String],
    agents: &'a [String],
    servers: &'a [McpServerSummary],
    instructions: &'a HashMap<String, String>,
) -> WorldStateInput<'a> {
    WorldStateInput {
        model,
        deferred_tools: deferred,
        loaded_tools: loaded,
        agent_types: agents,
        mcp_servers: servers,
        mcp_instructions: instructions,
    }
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

// ── deferred tools ──────────────────────────────────────────────────────────

#[test]
fn test_diff_deferred_tools_first_turn_announces_everything() {
    let current = ["Read".to_string(), "WebFetch".to_string()]
        .into_iter()
        .collect();

    let delta =
        diff_deferred_tools(&current, &[], &BTreeSet::new()).expect("an empty baseline announces");

    assert_eq!(delta.added_lines, vec!["- Read", "- WebFetch"]);
    assert!(delta.removed_names.is_empty());
}

#[test]
fn test_diff_deferred_tools_unchanged_is_silent() {
    let current: BTreeSet<String> = ["Read".to_string()].into_iter().collect();

    assert!(diff_deferred_tools(&current, &[], &current.clone()).is_none());
}

#[test]
fn test_diff_deferred_tools_discovered_tool_is_not_a_removal() {
    // The model ran ToolSearch: `Read` left the deferred set because its
    // schema is now in the request. Reporting it as removed would be false.
    let previous: BTreeSet<String> = ["Read".to_string()].into_iter().collect();
    let loaded = names(&["Read"]);

    assert!(diff_deferred_tools(&BTreeSet::new(), &loaded, &previous).is_none());
}

#[test]
fn test_diff_deferred_tools_gone_from_registry_is_a_removal() {
    let previous: BTreeSet<String> = ["Read".to_string()].into_iter().collect();

    let delta = diff_deferred_tools(&BTreeSet::new(), &[], &previous)
        .expect("a tool gone from the registry is announced");

    assert_eq!(delta.removed_names, vec!["Read"]);
}

#[test]
fn test_diff_deferred_tools_retired_names_are_never_announced() {
    // A resumed old session can carry retired names in its baseline; the model
    // never used them, so their removal is noise.
    let previous: BTreeSet<String> = ["Frame".to_string(), "TeamCreate".to_string()]
        .into_iter()
        .collect();

    assert!(diff_deferred_tools(&BTreeSet::new(), &[], &previous).is_none());
}

#[test]
fn test_diff_filters_retired_names_out_of_additions_too() {
    // Retired names are dropped on the way in, so they can neither be
    // announced as added nor enter the baseline and be announced as removed
    // later.
    let deferred = names(&["SuggestBackgroundPR", "NewMcpTool"]);
    let instructions = HashMap::new();
    let delta = diff(
        &input("m", &deferred, &[], &[], &[], &instructions),
        &WorldStateSnapshot::default(),
    );

    assert_eq!(
        delta
            .deferred_tools
            .expect("the live tool is announced")
            .added_lines,
        vec!["- NewMcpTool"]
    );
    assert!(
        !delta
            .candidate
            .deferred_tools
            .contains("SuggestBackgroundPR")
    );
}

#[test]
fn test_a_subagent_scope_does_not_inherit_a_false_removal() {
    // The reason the baseline is per scope: a subagent's first turn diffs
    // against its own empty snapshot, not the main session's tool set, so it
    // must not report the main session's tools as removed.
    let mut state = coco_types::ToolAppState::default();
    state.set_world_state_for_scope(
        None,
        WorldStateSnapshot {
            deferred_tools: ["TaskOutput".to_string()].into_iter().collect(),
            ..Default::default()
        },
    );

    let instructions = HashMap::new();
    let delta = diff(
        &input("m", &[], &[], &[], &[], &instructions),
        &state.world_state_for_scope(Some("agent-a")),
    );

    assert!(delta.deferred_tools.is_none());
}

// ── agent types ─────────────────────────────────────────────────────────────

#[test]
fn test_diff_agent_types_first_emission_is_initial() {
    let current: BTreeSet<String> = ["Explore".to_string()].into_iter().collect();

    let delta = diff_agent_types(&current, &BTreeSet::new()).expect("announces");

    assert!(delta.is_initial, "first emission uses the catalog header");
    assert_eq!(delta.added_lines, vec!["- Explore"]);
}

#[test]
fn test_diff_agent_types_later_emission_is_not_initial() {
    let previous: BTreeSet<String> = ["Explore".to_string()].into_iter().collect();
    let current: BTreeSet<String> = ["Explore".to_string(), "Plan".to_string()]
        .into_iter()
        .collect();

    let delta = diff_agent_types(&current, &previous).expect("announces the addition");

    assert!(!delta.is_initial, "an added type is not a fresh catalog");
    assert_eq!(delta.added_lines, vec!["- Plan"]);
    assert!(delta.removed_types.is_empty());
}

#[test]
fn test_diff_agent_types_unchanged_is_silent() {
    let current: BTreeSet<String> = ["Explore".to_string()].into_iter().collect();

    assert!(diff_agent_types(&current, &current.clone()).is_none());
}

// ── mcp servers ─────────────────────────────────────────────────────────────

#[test]
fn test_diff_mcp_servers_tool_count_change_is_announced() {
    // Counts are part of the comparison so a reconnect that changes the
    // exposed surface re-announces.
    let current: BTreeMap<_, _> = [("github".to_string(), announced(20))]
        .into_iter()
        .collect();
    let previous: BTreeMap<_, _> = [("github".to_string(), announced(14))]
        .into_iter()
        .collect();

    let delta = diff_mcp_servers(&current, &previous).expect("count change announces");
    assert_eq!(
        delta.servers[0].tool_count, 20,
        "listing shows the new count"
    );
}

#[test]
fn test_diff_mcp_servers_unchanged_is_silent() {
    let current: BTreeMap<_, _> = [("github".to_string(), announced(14))]
        .into_iter()
        .collect();

    assert!(diff_mcp_servers(&current, &current.clone()).is_none());
}

#[test]
fn test_diff_mcp_servers_caps_the_listing_and_reports_the_remainder() {
    let current: BTreeMap<_, _> = (0..12)
        .map(|i| (format!("server-{i:02}"), announced(1)))
        .collect();

    let delta = diff_mcp_servers(&current, &BTreeMap::new()).expect("announces");

    assert_eq!(delta.servers.len(), MAX_ANNOUNCED_MCP_SERVERS);
    assert_eq!(delta.omitted, 4);
    // BTreeMap order: the cap keeps the lexicographically first names.
    assert_eq!(delta.servers[0].name, "server-00");
    assert_eq!(delta.servers[7].name, "server-07");
}

// ── mcp instructions ────────────────────────────────────────────────────────

#[test]
fn test_diff_mcp_instructions_same_text_is_silent() {
    let instructions: HashMap<String, String> = [("github".to_string(), "use gh".to_string())]
        .into_iter()
        .collect();
    let digests: BTreeMap<_, _> = [("github".to_string(), ContentDigest::of("use gh"))]
        .into_iter()
        .collect();

    assert!(diff_mcp_instructions(&instructions, &digests, &digests.clone()).is_none());
}

#[test]
fn test_diff_mcp_instructions_edited_text_re_announces_the_body() {
    // Digests decide *whether* to announce; the full body is still what the
    // model gets, since it has to act on the text and not the hash.
    let instructions: HashMap<String, String> = [("github".to_string(), "use gh v2".to_string())]
        .into_iter()
        .collect();
    let current: BTreeMap<_, _> = [("github".to_string(), ContentDigest::of("use gh v2"))]
        .into_iter()
        .collect();
    let previous: BTreeMap<_, _> = [("github".to_string(), ContentDigest::of("use gh"))]
        .into_iter()
        .collect();

    let delta = diff_mcp_instructions(&instructions, &current, &previous).expect("announces");

    assert_eq!(delta.added_blocks, vec!["## github\n\nuse gh v2"]);
}

#[test]
fn test_diff_mcp_instructions_disconnect_is_announced() {
    let previous: BTreeMap<_, _> = [("github".to_string(), ContentDigest::of("use gh"))]
        .into_iter()
        .collect();

    let delta = diff_mcp_instructions(&HashMap::new(), &BTreeMap::new(), &previous)
        .expect("announces the disconnect");

    assert_eq!(delta.removed_names, vec!["github"]);
    assert!(delta.added_blocks.is_empty());
}

// ── model ───────────────────────────────────────────────────────────────────

#[test]
fn test_diff_model_first_turn_is_silent() {
    // Nothing has been claimed yet that could be wrong.
    assert!(diff_model("claude-opus-5", None).is_none());
}

#[test]
fn test_diff_model_unchanged_is_silent() {
    assert!(diff_model("claude-opus-5", Some("claude-opus-5")).is_none());
}

#[test]
fn test_diff_model_switch_reports_both_ids() {
    let info = diff_model("claude-opus-5", Some("claude-sonnet-5")).expect("announces");

    assert_eq!(info.previous, "claude-sonnet-5");
    assert_eq!(info.current, "claude-opus-5");
}

// ── whole-snapshot behaviour ────────────────────────────────────────────────

#[test]
fn test_diff_against_its_own_candidate_is_entirely_silent() {
    // The property the resume fix rests on: replaying the persisted baseline
    // must leave nothing to announce. If this breaks, a resumed session
    // re-announces its whole inventory on top of a history that already has it.
    let deferred = names(&["WebFetch"]);
    let agents = names(&["Explore"]);
    let servers = vec![server("github", 14)];
    let instructions: HashMap<String, String> = [("github".to_string(), "use gh".to_string())]
        .into_iter()
        .collect();
    let input = input(
        "claude-opus-5",
        &deferred,
        &[],
        &agents,
        &servers,
        &instructions,
    );

    let first = diff(&input, &WorldStateSnapshot::default());
    let second = diff(&input, &first.candidate);

    assert!(second.deferred_tools.is_none());
    assert!(second.agent_listing.is_none());
    assert!(second.mcp_servers.is_none());
    assert!(second.mcp_instructions.is_none());
    assert!(second.model_switch.is_none());
}

#[test]
fn test_adopt_fired_leaves_unfired_sections_for_the_next_turn() {
    // A generator that was disabled or timed out never reached history, so its
    // baseline must not advance — otherwise the announcement is lost for good.
    let deferred = names(&["WebFetch"]);
    let agents = names(&["Explore"]);
    let instructions = HashMap::new();
    let input = input("m", &deferred, &[], &agents, &[], &instructions);
    let delta = diff(&input, &WorldStateSnapshot::default());

    let mut baseline = WorldStateSnapshot::default();
    delta.adopt_fired(
        &mut baseline,
        &[AttachmentKind::DeferredToolsDelta].into_iter().collect(),
    );

    assert!(
        !baseline.deferred_tools.is_empty(),
        "fired section advances"
    );
    assert!(
        baseline.agent_types.is_empty(),
        "unfired section is retried"
    );

    let retry = diff(&input, &baseline);
    assert!(retry.deferred_tools.is_none());
    assert!(
        retry.agent_listing.is_some(),
        "the missed announcement retries"
    );
}

#[test]
fn test_adopt_fired_always_advances_the_model_section() {
    // A suppressed switch notice must not re-fire every turn for the rest of
    // the session: the mismatch it reports never resolves on its own.
    let instructions = HashMap::new();
    let input = input("claude-opus-5", &[], &[], &[], &[], &instructions);
    let previous = WorldStateSnapshot {
        model: Some("claude-sonnet-5".to_string()),
        ..Default::default()
    };
    let delta = diff(&input, &previous);
    assert!(delta.model_switch.is_some());

    let mut baseline = previous;
    delta.adopt_fired(&mut baseline, &HashSet::new());

    assert_eq!(baseline.model.as_deref(), Some("claude-opus-5"));
    assert!(diff(&input, &baseline).model_switch.is_none());
}

#[test]
fn test_retain_surviving_clears_only_what_compaction_dropped() {
    let mut baseline = WorldStateSnapshot {
        model: Some("claude-opus-5".to_string()),
        deferred_tools: ["WebFetch".to_string()].into_iter().collect(),
        agent_types: ["Explore".to_string()].into_iter().collect(),
        mcp_servers: [("github".to_string(), announced(14))]
            .into_iter()
            .collect(),
        mcp_instruction_digests: [("github".to_string(), ContentDigest::of("x"))]
            .into_iter()
            .collect(),
    };

    retain_surviving(&mut baseline, |kind| {
        kind == AttachmentKind::DeferredToolsDelta
    });

    assert!(!baseline.deferred_tools.is_empty(), "survivor is kept");
    assert!(
        baseline.agent_types.is_empty(),
        "dropped section re-announces"
    );
    assert!(baseline.mcp_servers.is_empty());
    assert!(baseline.mcp_instruction_digests.is_empty());
    assert_eq!(
        baseline.model.as_deref(),
        Some("claude-opus-5"),
        "model is not an announcement that has to survive in history"
    );
}
