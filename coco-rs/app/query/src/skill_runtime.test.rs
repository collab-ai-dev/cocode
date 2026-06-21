//! Tests for [`QuerySkillRuntime`].
//!
//! Uses in-memory `SkillManager` fixtures + a stub
//! `AgentQueryEngine` to exercise both inline and fork routing
//! without touching the filesystem.

use std::sync::Arc;

use async_trait::async_trait;
use coco_skills::SkillContext;
use coco_skills::SkillDefinition;
use coco_skills::SkillManager;
use coco_skills::SkillSource;
use coco_tool_runtime::AgentQueryConfig;
use coco_tool_runtime::AgentQueryEngine;
use coco_tool_runtime::AgentQueryResult;
use coco_tool_runtime::SkillHandle;
use coco_tool_runtime::SkillInvocationError;
use coco_tool_runtime::SkillInvocationResult;
use coco_tool_runtime::SubagentInheritance;
use pretty_assertions::assert_eq;

use super::*;

fn sample_skill(
    name: &str,
    prompt: &str,
    context: SkillContext,
    disabled: bool,
    disable_model: bool,
) -> SkillDefinition {
    SkillDefinition {
        name: name.into(),
        display_name: None,
        description: format!("{name} description"),
        prompt: prompt.into(),
        source: SkillSource::Bundled,
        aliases: Vec::new(),
        allowed_tools: None,
        model: None,
        model_role: None,
        when_to_use: None,
        argument_names: Vec::new(),
        paths: Vec::new(),
        effort: None,
        context,
        agent: None,
        version: None,
        disabled,
        hooks: None,
        argument_hint: None,
        user_invocable: true,
        disable_model_invocation: disable_model,
        shell: None,
        content_length: prompt.len() as i64,
        has_user_specified_description: true,
        progress_message: Some("running".to_string()),
        is_hidden: false,
        gated_by: None,
        files: std::collections::HashMap::new(),
        skill_root: None,
    }
}

fn runtime_with(skills: Vec<SkillDefinition>) -> QuerySkillRuntime {
    let mgr = SkillManager::new();
    for s in skills {
        mgr.register(s);
    }
    QuerySkillRuntime::new(Arc::new(mgr))
}

#[tokio::test]
async fn test_not_found_returns_not_found_error() {
    let rt = runtime_with(vec![]);
    let err = rt
        .invoke_skill(
            "nope",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SkillInvocationError::NotFound { .. }));
}

#[tokio::test]
async fn test_disabled_skill_returns_disabled_error() {
    let skill = sample_skill("foo", "body", SkillContext::Inline, true, false);
    let rt = runtime_with(vec![skill]);
    let err = rt
        .invoke_skill(
            "foo",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SkillInvocationError::Disabled { .. }));
}

#[tokio::test]
async fn test_disable_model_invocation_returns_hidden_error() {
    let skill = sample_skill("foo", "body", SkillContext::Inline, false, true);
    let rt = runtime_with(vec![skill]);
    let err = rt
        .invoke_skill(
            "foo",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SkillInvocationError::HiddenFromModel { .. }));
}

#[tokio::test]
async fn test_inline_skill_expands_prompt_into_new_messages() {
    let skill = sample_skill(
        "greet",
        "Hello from a skill!",
        SkillContext::Inline,
        false,
        false,
    );
    let rt = runtime_with(vec![skill]);
    let result = rt
        .invoke_skill(
            "greet",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .expect("ok");
    match result {
        SkillInvocationResult::Inline {
            summary,
            new_messages,
            permission_updates: _,
        } => {
            assert!(summary.contains("greet"));
            assert_eq!(new_messages.len(), 1);
            // Serialized UserMessage should carry the expanded prompt.
            let json = &new_messages[0];
            let text = serde_json::to_string(json).unwrap();
            assert!(
                text.contains("Hello from a skill"),
                "expanded prompt must appear in new_messages; got: {text}"
            );
        }
        other => panic!("expected Inline, got {other:?}"),
    }
}

#[tokio::test]
async fn test_inline_skill_substitutes_skill_dir_and_session_id() {
    let mut skill = sample_skill(
        "paths",
        "Schema at ${CLAUDE_SKILL_DIR}/schema.json for session ${CLAUDE_SESSION_ID}",
        SkillContext::Inline,
        false,
        false,
    );
    skill.skill_root = Some(std::path::PathBuf::from("/skills/paths"));

    let mgr = SkillManager::new();
    mgr.register(skill);
    let rt = QuerySkillRuntime::new(Arc::new(mgr)).with_session_id("sess-123");

    let result = rt
        .invoke_skill(
            "paths",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .expect("ok");
    let json = match result {
        SkillInvocationResult::Inline { new_messages, .. } => new_messages[0].clone(),
        _ => panic!("expected Inline"),
    };
    let text = serde_json::to_string(&json).unwrap();
    assert!(
        text.contains("/skills/paths/schema.json"),
        "${{CLAUDE_SKILL_DIR}} must be substituted; got: {text}"
    );
    assert!(
        text.contains("session sess-123"),
        "${{CLAUDE_SESSION_ID}} must be substituted; got: {text}"
    );
    assert!(
        text.contains("Base directory for this skill: /skills/paths"),
        "base-dir header must be prepended; got: {text}"
    );
    // The literal tokens must NOT survive.
    assert!(!text.contains("CLAUDE_SKILL_DIR"));
    assert!(!text.contains("CLAUDE_SESSION_ID"));
}

#[tokio::test]
async fn test_inline_skill_expands_arguments() {
    // `expand_skill_prompt` substitutes $ARGUMENTS with the
    // raw args string. Prove the substitution happens.
    let skill = sample_skill(
        "echo",
        "You said: $ARGUMENTS",
        SkillContext::Inline,
        false,
        false,
    );
    let rt = runtime_with(vec![skill]);
    let result = rt
        .invoke_skill(
            "echo",
            "hello world",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .expect("ok");
    let json = match result {
        SkillInvocationResult::Inline { new_messages, .. } => new_messages[0].clone(),
        _ => panic!("expected Inline"),
    };
    let text = serde_json::to_string(&json).unwrap();
    assert!(
        text.contains("hello world"),
        "args should be substituted; got: {text}"
    );
}

#[tokio::test]
async fn test_fork_skill_without_engine_fails_forked() {
    let skill = sample_skill("run", "Run something", SkillContext::Fork, false, false);
    let rt = runtime_with(vec![skill]);
    let err = rt
        .invoke_skill(
            "run",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .unwrap_err();
    match err {
        SkillInvocationError::Forked { reason } => {
            assert!(
                reason.contains("no AgentQueryEngine"),
                "must explain why; got: {reason}"
            );
        }
        other => panic!("expected Forked, got {other:?}"),
    }
}

struct StubEngine;

#[async_trait]
impl AgentQueryEngine for StubEngine {
    async fn execute_query(
        &self,
        prompt: &str,
        _config: AgentQueryConfig,
    ) -> Result<AgentQueryResult, coco_error::BoxedError> {
        Ok(AgentQueryResult {
            usage: Default::default(),
            cost_usd: 0.0,
            response_text: Some(format!("child got: {prompt}")),
            messages: Vec::new(),
            turns: 1,
            input_tokens: 10,
            output_tokens: 5,
            tool_use_count: 0,
            cancelled: false,
        })
    }
}

#[tokio::test]
async fn test_fork_skill_with_engine_routes_through_agent_query() {
    let skill = sample_skill(
        "analyze",
        "Analyze the input: $ARGUMENTS",
        SkillContext::Fork,
        false,
        false,
    );
    let rt = runtime_with(vec![skill]).with_agent_engine(Arc::new(StubEngine));

    let result = rt
        .invoke_skill(
            "analyze",
            "a signal",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .expect("ok");
    match result {
        SkillInvocationResult::Forked { agent_id, output } => {
            assert!(agent_id.starts_with("skill-analyze-"));
            assert!(
                output.contains("a signal"),
                "child prompt must reflect args; got: {output}"
            );
        }
        other => panic!("expected Forked, got {other:?}"),
    }
}

#[tokio::test]
async fn test_name_normalization_strips_leading_slash() {
    // Users may invoke `/greet` in slash-command style; the
    // runtime should strip the slash.
    let skill = sample_skill("greet", "Hi!", SkillContext::Inline, false, false);
    let rt = runtime_with(vec![skill]);
    let ok = rt
        .invoke_skill(
            "/greet",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await;
    assert!(ok.is_ok(), "leading slash should normalize away");
}

#[tokio::test]
async fn test_inline_skill_without_allowed_tools_has_no_updates() {
    // Baseline: an inline skill with no `allowed-tools` frontmatter
    // produces an empty `permission_updates` vec. Nothing flows into
    // the executor's permission-rule handle on that path.
    let skill = sample_skill("plain", "Hi!", SkillContext::Inline, false, false);
    let rt = runtime_with(vec![skill]);
    let result = rt
        .invoke_skill(
            "plain",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .expect("ok");
    match result {
        SkillInvocationResult::Inline {
            permission_updates, ..
        } => {
            assert!(
                permission_updates.is_empty(),
                "no `allowed-tools` ⇒ no updates, got: {permission_updates:?}"
            );
        }
        other => panic!("expected Inline, got {other:?}"),
    }
}

#[tokio::test]
async fn test_inline_skill_with_allowed_tools_emits_command_rules() {
    // A skill frontmatter `allowed-tools: Read, Edit(*.md)` becomes a
    // single `PermissionUpdate::AddRules { destination: Command }` carrying
    // two `PermissionRule { source: Command, behavior: Allow, ... }` entries.
    // The destination is `Command` (NOT `Session`) so the rules live in the
    // `alwaysAllowRules.command` slot — observable for audit + cleanly
    // separated from user-clicked Always-Allow rules.
    let mut skill = sample_skill("editor", "Edit stuff", SkillContext::Inline, false, false);
    skill.allowed_tools = Some(vec!["Read".to_string(), "Edit(*.md)".to_string()]);
    let rt = runtime_with(vec![skill]);
    let result = rt
        .invoke_skill(
            "editor",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .expect("ok");
    let permission_updates = match result {
        SkillInvocationResult::Inline {
            permission_updates, ..
        } => permission_updates,
        other => panic!("expected Inline, got {other:?}"),
    };
    assert_eq!(
        permission_updates.len(),
        1,
        "one AddRules update per skill invocation"
    );
    match &permission_updates[0] {
        coco_types::PermissionUpdate::AddRules { rules, destination } => {
            assert_eq!(
                *destination,
                coco_types::PermissionUpdateDestination::Command,
                "destination must be Command, not Session"
            );
            assert_eq!(rules.len(), 2);
            assert_eq!(rules[0].source, coco_types::PermissionRuleSource::Command);
            assert_eq!(rules[0].behavior, coco_types::PermissionBehavior::Allow);
            assert_eq!(rules[0].value.tool_pattern, "Read");
            assert!(rules[0].value.rule_content.is_none());
            // `Edit(*.md)` parses to `tool_pattern: Edit` + content `*.md`.
            assert_eq!(rules[1].value.tool_pattern, "Edit");
            assert_eq!(rules[1].value.rule_content.as_deref(), Some("*.md"));
        }
        other => panic!("expected AddRules, got {other:?}"),
    }
}

#[tokio::test]
async fn test_fork_skill_with_allowed_tools_does_not_narrow_registry() {
    // Fork-mode skills push `allowed-tools` into `extra_permission_rules`
    // (auto-allow), NOT `allowed_tools` (registry filter). The forked
    // subagent therefore sees the full inherited tool registry; the listed
    // entries are simply auto-allowed.
    //
    // We assert this on the config the runtime hands to the engine
    // by recording the value through a capturing `AgentQueryEngine`.
    use std::sync::Mutex;

    struct CapturingEngine {
        captured: Arc<Mutex<Option<AgentQueryConfig>>>,
    }

    #[async_trait]
    impl AgentQueryEngine for CapturingEngine {
        async fn execute_query(
            &self,
            _prompt: &str,
            config: AgentQueryConfig,
        ) -> Result<AgentQueryResult, coco_error::BoxedError> {
            *self.captured.lock().unwrap() = Some(config);
            Ok(AgentQueryResult {
                usage: Default::default(),
                cost_usd: 0.0,
                response_text: Some("ok".into()),
                messages: Vec::new(),
                turns: 1,
                input_tokens: 0,
                output_tokens: 0,
                tool_use_count: 0,
                cancelled: false,
            })
        }
    }

    let mut skill = sample_skill("scan", "Scan!", SkillContext::Fork, false, false);
    skill.allowed_tools = Some(vec!["Read".to_string(), "Grep".to_string()]);
    let captured = Arc::new(Mutex::new(None));
    let engine = Arc::new(CapturingEngine {
        captured: captured.clone(),
    });
    let rt = runtime_with(vec![skill]).with_agent_engine(engine);
    rt.invoke_skill(
        "scan",
        "",
        SubagentInheritance::default(),
        coco_tool_runtime::SkillGateContext::default(),
    )
    .await
    .expect("ok");

    let config = captured.lock().unwrap().take().expect("captured config");
    // Registry filter MUST be empty — fork-skills do not narrow tools[].
    assert!(
        config.allowed_tools.is_empty(),
        "fork-skill must NOT set registry filter; got: {:?}",
        config.allowed_tools
    );
    // Auto-allow set MUST contain the two Command-source rules.
    assert_eq!(config.extra_permission_rules.len(), 2);
    assert!(
        config
            .extra_permission_rules
            .iter()
            .all(|r| r.source == coco_types::PermissionRuleSource::Command
                && r.behavior == coco_types::PermissionBehavior::Allow),
        "extra_permission_rules must all be Command + Allow; got: {:?}",
        config.extra_permission_rules
    );
}

/// Stub `BashToolHandle` for in-prompt shell-expansion tests: returns a
/// fixed string per marker, or fails (denied / command error) when `fail`.
struct StubBash {
    output: String,
    fail: bool,
}

#[async_trait]
impl coco_skills::shell_exec::BashToolHandle for StubBash {
    async fn execute_with_permissions(
        &self,
        _command: &str,
        _allowed_tools: &[String],
    ) -> Result<String, String> {
        if self.fail {
            Err("permission denied".into())
        } else {
            Ok(self.output.clone())
        }
    }
}

fn bash_cell(handle: StubBash) -> SkillBashHandleCell {
    Arc::new(std::sync::RwLock::new(Some(Arc::new(handle) as _)))
}

#[tokio::test]
async fn test_inline_skill_runs_shell_markers_when_bash_handle_present() {
    // With a Bash handle installed, the model-invoked inline path expands
    // `` !`cmd` `` markers (parity with the user-typed slash path / TS
    // getPromptForCommand). The stub replaces the marker with "HELLO".
    let skill = sample_skill(
        "diff",
        "diff: !`echo hi`",
        SkillContext::Inline,
        false,
        false,
    );
    let rt = runtime_with(vec![skill]).with_bash_handle_cell(bash_cell(StubBash {
        output: "HELLO".into(),
        fail: false,
    }));
    let result = rt
        .invoke_skill(
            "diff",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .expect("ok");
    let text = match result {
        SkillInvocationResult::Inline { new_messages, .. } => {
            serde_json::to_string(&new_messages[0]).unwrap()
        }
        other => panic!("expected Inline, got {other:?}"),
    };
    assert!(
        text.contains("HELLO"),
        "shell marker must expand; got: {text}"
    );
    assert!(
        !text.contains("echo hi"),
        "raw marker must not survive; got: {text}"
    );
}

#[tokio::test]
async fn test_inline_skill_leaves_markers_when_no_bash_handle() {
    // Without a Bash handle (tests / pre-bootstrap) the marker is left
    // untouched — no unguarded shell ever runs.
    let skill = sample_skill(
        "diff",
        "diff: !`echo hi`",
        SkillContext::Inline,
        false,
        false,
    );
    let rt = runtime_with(vec![skill]);
    let result = rt
        .invoke_skill(
            "diff",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .expect("ok");
    let text = match result {
        SkillInvocationResult::Inline { new_messages, .. } => {
            serde_json::to_string(&new_messages[0]).unwrap()
        }
        other => panic!("expected Inline, got {other:?}"),
    };
    assert!(
        text.contains("echo hi"),
        "marker must survive verbatim when no handle; got: {text}"
    );
}

#[tokio::test]
async fn test_mcp_skill_skips_shell_expansion() {
    // MCP-sourced skills are remote/untrusted — their in-prompt shell must
    // NEVER run, even with a Bash handle installed.
    let mut skill = sample_skill(
        "remote",
        "x: !`echo hi`",
        SkillContext::Inline,
        false,
        false,
    );
    skill.source = SkillSource::Mcp {
        server_name: "srv".into(),
    };
    let rt = runtime_with(vec![skill]).with_bash_handle_cell(bash_cell(StubBash {
        output: "LEAK".into(),
        fail: false,
    }));
    let result = rt
        .invoke_skill(
            "remote",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .expect("ok");
    let text = match result {
        SkillInvocationResult::Inline { new_messages, .. } => {
            serde_json::to_string(&new_messages[0]).unwrap()
        }
        other => panic!("expected Inline, got {other:?}"),
    };
    assert!(
        !text.contains("LEAK"),
        "MCP skill must not run shell; got: {text}"
    );
    assert!(text.contains("echo hi"), "marker must survive; got: {text}");
}

#[tokio::test]
async fn test_shell_expansion_failure_aborts_invocation() {
    // A denied / failed marker aborts the whole invocation with an
    // Expansion error (mirrors TS MalformedCommandError — no partial
    // substitution reaches the model).
    let skill = sample_skill(
        "diff",
        "diff: !`rm -rf /`",
        SkillContext::Inline,
        false,
        false,
    );
    let rt = runtime_with(vec![skill]).with_bash_handle_cell(bash_cell(StubBash {
        output: String::new(),
        fail: true,
    }));
    let err = rt
        .invoke_skill(
            "diff",
            "",
            SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .unwrap_err();
    match err {
        SkillInvocationError::Expansion { name, reason } => {
            assert_eq!(name, "diff");
            assert!(reason.contains("permission denied"), "got: {reason}");
        }
        other => panic!("expected Expansion, got {other:?}"),
    }
}
