use super::*;
use serde_json::json;

#[test]
fn test_generate_word_slug_format() {
    let slug = generate_word_slug();
    // At least 3 parts (adjective-verb-noun), though some words contain dashes
    assert!(
        slug.split('-').count() >= 3,
        "slug should have at least 3 parts: {slug}"
    );
    assert!(!slug.is_empty());
}

#[test]
fn test_generate_word_slug_uniqueness() {
    let slugs: Vec<String> = (0..10).map(|_| generate_word_slug()).collect();
    // Not all identical (statistically near-impossible)
    let unique: std::collections::HashSet<&str> = slugs.iter().map(String::as_str).collect();
    assert!(unique.len() > 1, "expected varied slugs, got: {slugs:?}");
}

#[test]
fn test_slug_cache_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let plans_dir = dir.path();

    let slug1 = get_plan_slug("test-session-1", plans_dir);
    let slug2 = get_plan_slug("test-session-1", plans_dir);
    assert_eq!(slug1, slug2, "same session should return cached slug");

    let slug3 = get_plan_slug("test-session-2", plans_dir);
    // Different sessions may (rarely) collide, but usually differ
    let _ = slug3; // just ensure it doesn't panic

    clear_plan_slug("test-session-1");
    let slug4 = get_plan_slug("test-session-1", plans_dir);
    // After clearing, a new slug is generated (may differ)
    let _ = slug4;
}

#[test]
fn test_set_plan_slug() {
    let dir = tempfile::tempdir().unwrap();
    set_plan_slug("set-test", "custom-slug-here");
    let slug = get_plan_slug("set-test", dir.path());
    assert_eq!(slug, "custom-slug-here");
    clear_plan_slug("set-test");
}

#[test]
fn test_resolve_plans_directory_default() {
    let config = PathBuf::from("/home/user/.coco");
    let result = resolve_plans_directory(&config, None, None);
    assert_eq!(result, PathBuf::from("/home/user/.coco/plans"));
}

#[test]
fn test_resolve_plans_directory_with_setting() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let plans_sub = project.join("my-plans");
    std::fs::create_dir_all(&plans_sub).unwrap();

    let config = PathBuf::from("/home/user/.coco");
    let result = resolve_plans_directory(&config, Some(project), Some("my-plans"));
    assert!(result.ends_with("my-plans"));
}

#[test]
fn resolve_plans_directory_rejects_parent_escape() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let config = PathBuf::from("/home/user/.coco");
    let result = resolve_plans_directory(&config, Some(&project), Some("../outside"));

    assert_eq!(result, PathBuf::from("/home/user/.coco/plans"));
}

#[test]
fn resolve_plans_directory_rejects_absolute_outside_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let outside = dir.path().join("outside-plans");
    std::fs::create_dir_all(&project).unwrap();

    let config = PathBuf::from("/home/user/.coco");
    let result = resolve_plans_directory(
        &config,
        Some(&project),
        Some(outside.to_str().expect("utf-8 temp path")),
    );

    assert_eq!(result, PathBuf::from("/home/user/.coco/plans"));
}

#[test]
fn resolve_plans_directory_allows_nonexistent_normalized_inside_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let config = PathBuf::from("/home/user/.coco");
    let result = resolve_plans_directory(&config, Some(&project), Some("nested/../plans"));

    assert_eq!(result, project.canonicalize().unwrap().join("plans"));
}

#[test]
fn test_plan_crud() {
    let dir = tempfile::tempdir().unwrap();
    let plans_dir = dir.path().join("plans");
    let sid = "crud-test";

    // Initially no plan
    assert!(!plan_exists(sid, &plans_dir, None));
    assert!(get_plan(sid, &plans_dir, None).is_none());

    // Write
    write_plan(sid, &plans_dir, "# My Plan\n\n1. Do stuff", None).unwrap();
    assert!(plan_exists(sid, &plans_dir, None));

    // Read
    let content = get_plan(sid, &plans_dir, None).unwrap();
    assert_eq!(content, "# My Plan\n\n1. Do stuff");

    // Update
    write_plan(sid, &plans_dir, "# Updated Plan", None).unwrap();
    let content = get_plan(sid, &plans_dir, None).unwrap();
    assert_eq!(content, "# Updated Plan");

    // Delete
    delete_plan(sid, &plans_dir, None).unwrap();
    assert!(!plan_exists(sid, &plans_dir, None));

    clear_plan_slug(sid);
}

#[test]
fn test_subagent_plan_path() {
    let dir = tempfile::tempdir().unwrap();
    let plans_dir = dir.path();
    let sid = "agent-test";
    set_plan_slug(sid, "bright-dancing-fox");

    let main_path = get_plan_file_path(sid, plans_dir, None);
    assert!(main_path.ends_with("bright-dancing-fox.md"));

    let agent_path = get_plan_file_path(sid, plans_dir, Some("agent-42"));
    assert!(agent_path.ends_with("bright-dancing-fox-agent-agent-42.md"));

    clear_plan_slug(sid);
}

#[test]
fn test_recover_plan_from_exit_tool_input() {
    let dir = tempfile::tempdir().unwrap();
    let plans_dir = dir.path().join("plans");
    let sid = "recover-test";

    let entries = vec![json!({
        "role": "assistant",
        "content": [{
            "type": "tool_use",
            "name": "ExitPlanMode",
            "input": { "plan": "# Recovered Plan\n\n- Step 1\n- Step 2" }
        }]
    })];

    let result = recover_plan_for_resume(sid, &plans_dir, "test-slug", &entries);
    assert!(result);

    let content = std::fs::read_to_string(plans_dir.join("test-slug.md")).unwrap();
    assert_eq!(content, "# Recovered Plan\n\n- Step 1\n- Step 2");

    clear_plan_slug(sid);
}

#[test]
fn test_recover_plan_prefers_file_snapshot_over_newer_tool_input() {
    // File snapshot check runs before message recovery, so the most recent
    // plan snapshot globally wins over tool inputs.
    let dir = tempfile::tempdir().unwrap();
    let plans_dir = dir.path().join("plans");
    let sid = "recover-snapshot-priority";

    let entries = vec![
        json!({
            "type": "system",
            "subtype": "file_snapshot",
            "snapshotFiles": [{
                "key": "plan",
                "path": "/tmp/plan.md",
                "content": "# Snapshot Plan"
            }]
        }),
        json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "ExitPlanMode",
                    "input": { "plan": "# Tool Input Plan" }
                }]
            }
        }),
    ];

    let result = recover_plan_for_resume(sid, &plans_dir, "snapshot-slug", &entries);
    assert!(result);

    let content = std::fs::read_to_string(plans_dir.join("snapshot-slug.md")).unwrap();
    assert_eq!(content, "# Snapshot Plan");

    clear_plan_slug(sid);
}

#[test]
fn test_recover_plan_from_ts_assistant_message_shape() {
    let dir = tempfile::tempdir().unwrap();
    let plans_dir = dir.path().join("plans");
    let sid = "recover-ts-assistant";

    let entries = vec![json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "name": "ExitPlanMode",
                "input": { "plan": "# TS Assistant Plan" }
            }]
        }
    })];

    let result = recover_plan_for_resume(sid, &plans_dir, "ts-assistant-slug", &entries);
    assert!(result);

    let content = std::fs::read_to_string(plans_dir.join("ts-assistant-slug.md")).unwrap();
    assert_eq!(content, "# TS Assistant Plan");

    clear_plan_slug(sid);
}

#[test]
fn test_recover_plan_from_user_plan_content() {
    let dir = tempfile::tempdir().unwrap();
    let plans_dir = dir.path().join("plans");
    let sid = "recover-user-test";

    let entries = vec![json!({
        "role": "user",
        "planContent": "# User Plan Content"
    })];

    let result = recover_plan_for_resume(sid, &plans_dir, "user-slug", &entries);
    assert!(result);

    let content = std::fs::read_to_string(plans_dir.join("user-slug.md")).unwrap();
    assert_eq!(content, "# User Plan Content");

    clear_plan_slug(sid);
}

#[test]
fn test_recover_plan_file_already_exists() {
    let dir = tempfile::tempdir().unwrap();
    let plans_dir = dir.path().join("plans");
    std::fs::create_dir_all(&plans_dir).unwrap();
    std::fs::write(plans_dir.join("existing-slug.md"), "existing").unwrap();
    let sid = "exists-test";

    let result = recover_plan_for_resume(sid, &plans_dir, "existing-slug", &[]);
    assert!(result);

    clear_plan_slug(sid);
}

#[test]
fn test_copy_plan_for_fork() {
    let dir = tempfile::tempdir().unwrap();
    let plans_dir = dir.path().join("plans");
    let src_sid = "fork-src";
    let dst_sid = "fork-dst";

    write_plan(src_sid, &plans_dir, "# Source Plan", None).unwrap();

    let result = copy_plan_for_fork(src_sid, dst_sid, &plans_dir);
    assert!(result);

    let dst_content = get_plan(dst_sid, &plans_dir, None).unwrap();
    assert_eq!(dst_content, "# Source Plan");

    clear_plan_slug(src_sid);
    clear_plan_slug(dst_sid);
}

// ── Reminder rendering ──

fn att(rt: ReminderType, is_sub: bool, path: &str, exists: bool) -> PlanModeAttachment {
    PlanModeAttachment {
        reminder_type: rt,
        workflow: PlanWorkflow::default(),
        phase4_variant: Phase4Variant::default(),
        explore_agent_count: 3,
        plan_agent_count: 1,
        explore_plan_agents_available: true,
        is_sub_agent: is_sub,
        plan_file_path: path.into(),
        plan_exists: exists,
        write_tool: coco_types::ToolName::Write,
        edit_tool: coco_types::ToolName::Edit,
        deferred_tools: Vec::new(),
    }
}

#[test]
fn reminder_full_main_agent_includes_workflow_and_plan_file() {
    let att = PlanModeAttachment {
        reminder_type: ReminderType::Full,
        workflow: PlanWorkflow::default(),
        phase4_variant: Phase4Variant::default(),
        explore_agent_count: 3,
        plan_agent_count: 1,
        explore_plan_agents_available: true,
        is_sub_agent: false,
        plan_file_path: "/tmp/plans/foo.md".into(),
        plan_exists: true,
        write_tool: coco_types::ToolName::Write,
        edit_tool: coco_types::ToolName::Edit,
        deferred_tools: Vec::new(),
    };
    let out = render_plan_mode_reminder(&att);
    assert!(out.contains("Plan mode is active"));
    assert!(out.contains("/tmp/plans/foo.md"));
    assert!(out.contains("A plan file already exists"));
    assert!(out.contains("## Plan Workflow"));
    // Must not claim no plan file when one exists.
    assert!(!out.contains("No plan file exists yet"));
}

#[test]
fn reminder_names_model_specific_file_tool() {
    // Regression for the gpt-5 plan-file trigger: the reminder must name the
    // model's actual file tool (`apply_patch`), never a hardcoded `Write`/
    // `Edit` the gpt-5 family doesn't have. The builder resolves these from
    // the model's tool set; here we simulate the gpt-5 resolution.
    let mk = |exists: bool| PlanModeAttachment {
        reminder_type: ReminderType::Full,
        workflow: PlanWorkflow::default(),
        phase4_variant: Phase4Variant::default(),
        explore_agent_count: 3,
        plan_agent_count: 1,
        explore_plan_agents_available: true,
        is_sub_agent: false,
        plan_file_path: "/tmp/plans/foo.md".into(),
        plan_exists: exists,
        write_tool: coco_types::ToolName::ApplyPatch,
        edit_tool: coco_types::ToolName::ApplyPatch,
        deferred_tools: Vec::new(),
    };

    let new_plan = render_plan_mode_reminder(&mk(false));
    assert!(new_plan.contains("using the apply_patch tool"));
    assert!(!new_plan.contains("using the Write tool"));

    let existing_plan = render_plan_mode_reminder(&mk(true));
    assert!(existing_plan.contains("using the apply_patch tool"));
    assert!(!existing_plan.contains("using the Edit tool"));
}

#[test]
fn reminder_mentions_tool_search_only_when_exit_plan_mode_is_deferred() {
    let mut loaded = att(ReminderType::Sparse, false, "/tmp/plans/foo.md", false);
    loaded.deferred_tools = vec!["OtherTool".to_string()];
    let loaded_text = render_plan_mode_reminder(&loaded);
    assert!(
        !loaded_text.contains("select:ExitPlanMode"),
        "loaded ExitPlanMode should not get ToolSearch guidance: {loaded_text}"
    );

    let mut deferred = att(ReminderType::Sparse, false, "/tmp/plans/foo.md", false);
    deferred.deferred_tools = vec![coco_types::ToolName::ExitPlanMode.as_str().to_string()];
    let deferred_text = render_plan_mode_reminder(&deferred);
    assert!(
        deferred_text.contains("select:ExitPlanMode"),
        "deferred ExitPlanMode should get ToolSearch guidance: {deferred_text}"
    );
}

#[test]
fn reminder_full_missing_plan_file_switches_branch() {
    let att = att(ReminderType::Full, false, "/tmp/plans/new.md", false);
    let out = render_plan_mode_reminder(&att);
    assert!(out.contains("No plan file exists yet"));
    assert!(!out.contains("A plan file already exists"));
}

#[test]
fn reminder_sparse_is_short_and_references_plan_file() {
    let att = att(ReminderType::Sparse, false, "/tmp/plans/foo.md", true);
    let out = render_plan_mode_reminder(&att);
    assert!(out.contains("Plan mode still active"));
    assert!(out.contains("/tmp/plans/foo.md"));
    // Sparse variant should NOT include the workflow block.
    assert!(!out.contains("## Plan Workflow"));
}

#[test]
fn reminder_reentry_has_reentry_heading_and_plan_state() {
    let att = att(ReminderType::Reentry, false, "/tmp/plans/foo.md", true);
    let out = render_plan_mode_reminder(&att);
    assert!(out.contains("## Re-entering Plan Mode"));
    assert!(out.contains("A plan file exists at /tmp/plans/foo.md"));
    assert!(out.contains("you should always edit the plan file"));
    // Distinct from Full + Sparse.
    assert!(!out.contains("## Plan Workflow"));
    assert!(!out.contains("Plan mode still active"));
}

#[test]
fn reminder_full_sub_agent_skips_workflow_and_plan_approval_text() {
    let att = att(
        ReminderType::Full,
        true,
        "/tmp/plans/foo-agent-a1.md",
        false,
    );
    let out = render_plan_mode_reminder(&att);
    assert!(out.contains("Plan mode is active"));
    assert!(out.contains("/tmp/plans/foo-agent-a1.md"));
    assert!(!out.contains("## Plan Workflow"));
}

// ── Phase-4 variant rendering ──

#[test]
fn phase4_standard_includes_context_section() {
    let mut a = att(ReminderType::Full, false, "/p.md", false);
    a.phase4_variant = Phase4Variant::Standard;
    let out = render_plan_mode_reminder(&a);
    assert!(out.contains("Begin with a **Context** section"));
    assert!(!out.contains("Hard limit: 40 lines"));
}

#[test]
fn phase4_trim_one_line_context() {
    let mut a = att(ReminderType::Full, false, "/p.md", false);
    a.phase4_variant = Phase4Variant::Trim;
    let out = render_plan_mode_reminder(&a);
    assert!(out.contains("One-line **Context**"));
}

#[test]
fn phase4_cut_forbids_context_section() {
    let mut a = att(ReminderType::Full, false, "/p.md", false);
    a.phase4_variant = Phase4Variant::Cut;
    let out = render_plan_mode_reminder(&a);
    assert!(out.contains("Do NOT write a Context or Background section"));
    assert!(out.contains("under 40 lines"));
}

#[test]
fn phase4_cap_enforces_hard_limit() {
    let mut a = att(ReminderType::Full, false, "/p.md", false);
    a.phase4_variant = Phase4Variant::Cap;
    let out = render_plan_mode_reminder(&a);
    assert!(out.contains("Hard limit: 40 lines"));
    assert!(out.contains("Do NOT restate the user's request"));
}

// ── Interview workflow ──

#[test]
fn interview_workflow_uses_loop_format() {
    let mut a = att(ReminderType::Full, false, "/p.md", false);
    a.workflow = PlanWorkflow::Interview;
    let out = render_plan_mode_reminder(&a);
    assert!(out.contains("Iterative Planning Workflow"));
    assert!(out.contains("pair-planning with the user"));
    // Must NOT render the 5-phase content.
    assert!(!out.contains("## Plan Workflow\n"));
    assert!(!out.contains("### Phase 1:"));
}

#[test]
fn interview_sparse_shares_five_phase_sparse_text() {
    // Sparse is workflow-independent.
    let mut a = att(ReminderType::Sparse, false, "/p.md", true);
    a.workflow = PlanWorkflow::Interview;
    let out = render_plan_mode_reminder(&a);
    assert!(out.contains("Plan mode still active"));
}

// ── Agent count threading ──

#[test]
fn five_phase_substitutes_configured_agent_counts() {
    let mut a = att(ReminderType::Full, false, "/p.md", false);
    a.explore_agent_count = 7;
    a.plan_agent_count = 5;
    let out = render_plan_mode_reminder(&a);
    assert!(out.contains("up to 7 Explore agents"));
    assert!(out.contains("up to 5 agent(s) in parallel"));
    assert!(out.contains("only use the Explore subagent type"));
    assert!(out.contains("Launch Plan agent(s)"));
    assert!(!out.contains("only use the explore subagent type"));
    // The "multiple agents" block should appear when plan_agent_count > 1.
    assert!(out.contains("Multiple agents"));
}

#[test]
fn five_phase_single_plan_agent_omits_multiple_block() {
    let a = att(ReminderType::Full, false, "/p.md", false);
    // default plan_agent_count = 1
    let out = render_plan_mode_reminder(&a);
    assert!(!out.contains("Multiple agents"));
}

#[test]
fn five_phase_without_agents_falls_back_to_interview_workflow() {
    let mut a = att(ReminderType::Full, false, "/p.md", false);
    a.explore_plan_agents_available = false;
    let out = render_plan_mode_reminder(&a);
    assert!(out.contains("Iterative Planning Workflow"));
    assert!(out.contains("Use Read, Glob, Grep, LSP"));
    assert!(!out.contains("Plan Workflow"));
    assert!(!out.contains("Launch Plan agent"));
    assert!(!out.contains("agent type to parallelize"));
}

#[test]
fn interview_omits_explore_agent_sentence_when_unavailable() {
    let mut a = att(ReminderType::Full, false, "/p.md", false);
    a.workflow = PlanWorkflow::Interview;
    a.explore_plan_agents_available = false;
    let out = render_plan_mode_reminder(&a);
    assert!(out.contains("Iterative Planning Workflow"));
    assert!(!out.contains("agent type to parallelize"));
}

// ── verify_plan_was_edited ──

#[test]
fn verify_missing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("nope.md");
    let entry_ms = 1_700_000_000_000; // arbitrary past
    assert_eq!(
        verify_plan_was_edited(&path, entry_ms),
        Some(PlanVerificationOutcome::Missing)
    );
}

#[test]
fn verify_edited_after_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("plan.md");
    std::fs::write(&path, "old content").unwrap();
    // Entry time before the write → file mtime is later → "edited".
    let entry_ms = 1_000; // epoch+1s, definitely older than any mtime
    assert_eq!(
        verify_plan_was_edited(&path, entry_ms),
        Some(PlanVerificationOutcome::Edited)
    );
}

#[test]
fn verify_not_edited_when_entry_later_than_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("plan.md");
    std::fs::write(&path, "old content").unwrap();
    // Entry time far in the future → file was never touched after entry.
    let entry_ms = i64::MAX;
    assert_eq!(
        verify_plan_was_edited(&path, entry_ms),
        Some(PlanVerificationOutcome::NotEdited)
    );
}

#[test]
fn verify_returns_none_when_no_entry_ms() {
    // Without an entry timestamp there's no baseline, so the function returns
    // `None` (the caller treats "skipped" as absence of an outcome).
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("plan.md");
    std::fs::write(&path, "content").unwrap();
    assert_eq!(verify_plan_was_edited(&path, 0), None);
    assert_eq!(verify_plan_was_edited(&path, -1), None);
}

#[test]
fn exit_reminder_with_existing_plan_includes_reference() {
    let att = PlanModeExitAttachment {
        plan_file_path: "/tmp/plans/foo.md".into(),
        plan_exists: true,
        outcome: Some(coco_types::ExitPlanModeOutcome::ImplementationPlan),
    };
    let out = render_plan_mode_exit_reminder(&att);
    assert!(out.contains("## Exited Plan Mode"));
    assert!(out.contains("can now make edits"));
    assert!(out.contains("/tmp/plans/foo.md"));
}

#[test]
fn exit_reminder_without_plan_omits_reference() {
    let att = PlanModeExitAttachment {
        plan_file_path: "/tmp/plans/foo.md".into(),
        plan_exists: false,
        outcome: None,
    };
    let out = render_plan_mode_exit_reminder(&att);
    assert!(out.contains("## Exited Plan Mode"));
    assert!(!out.contains("/tmp/plans/foo.md"));
}

#[test]
fn exit_reminder_no_implementation_plan_omits_stale_reference() {
    let att = PlanModeExitAttachment {
        plan_file_path: "/tmp/plans/old.md".into(),
        plan_exists: true,
        outcome: Some(coco_types::ExitPlanModeOutcome::NoImplementationPlan),
    };
    let out = render_plan_mode_exit_reminder(&att);
    assert!(out.contains("without an implementation plan"));
    assert!(!out.contains("/tmp/plans/old.md"));
    assert!(!out.contains("can now make edits"));
}

// ── Full-text snapshot pinning (Phase-4 + workflow matrix) ──
//
// Partial `contains()` assertions above catch *semantic* drift (the
// key load-bearing phrases). These snapshots pin the FULL rendered
// output per (workflow, phase4_variant, is_sub_agent) combination so
// whitespace / ordering / adjacent-text drift surfaces on review via
// `cargo insta pending-snapshots -p coco-context`.
//
// Deliberately narrow: one snapshot per Full main-agent Phase-4
// variant (4), one for Interview, one for Sub-agent. Sparse / Reentry
// are short enough that the `contains()` tests cover them.

fn snapshot_attachment(phase4: Phase4Variant, workflow: PlanWorkflow) -> PlanModeAttachment {
    PlanModeAttachment {
        reminder_type: ReminderType::Full,
        workflow,
        phase4_variant: phase4,
        explore_agent_count: 3,
        plan_agent_count: 1,
        explore_plan_agents_available: true,
        is_sub_agent: false,
        plan_file_path: "/tmp/plans/SNAP.md".into(),
        plan_exists: false,
        write_tool: coco_types::ToolName::Write,
        edit_tool: coco_types::ToolName::Edit,
        deferred_tools: Vec::new(),
    }
}

#[test]
fn snapshot_full_five_phase_standard() {
    let att = snapshot_attachment(Phase4Variant::Standard, PlanWorkflow::FivePhase);
    insta::assert_snapshot!("full_five_phase_standard", render_plan_mode_reminder(&att));
}

#[test]
fn snapshot_full_five_phase_trim() {
    let att = snapshot_attachment(Phase4Variant::Trim, PlanWorkflow::FivePhase);
    insta::assert_snapshot!("full_five_phase_trim", render_plan_mode_reminder(&att));
}

#[test]
fn snapshot_full_five_phase_cut() {
    let att = snapshot_attachment(Phase4Variant::Cut, PlanWorkflow::FivePhase);
    insta::assert_snapshot!("full_five_phase_cut", render_plan_mode_reminder(&att));
}

#[test]
fn snapshot_full_five_phase_cap() {
    let att = snapshot_attachment(Phase4Variant::Cap, PlanWorkflow::FivePhase);
    insta::assert_snapshot!("full_five_phase_cap", render_plan_mode_reminder(&att));
}

#[test]
fn snapshot_full_interview() {
    // Interview ignores phase4_variant, but fix it to Standard for determinism.
    let att = snapshot_attachment(Phase4Variant::Standard, PlanWorkflow::Interview);
    insta::assert_snapshot!("full_interview", render_plan_mode_reminder(&att));
}

#[test]
fn snapshot_full_sub_agent() {
    let mut att = snapshot_attachment(Phase4Variant::Standard, PlanWorkflow::FivePhase);
    att.is_sub_agent = true;
    att.plan_file_path = "/tmp/plans/SNAP-agent-a1.md".into();
    insta::assert_snapshot!("full_sub_agent", render_plan_mode_reminder(&att));
}
