//! Tests for ToolRegistry, focusing on the B3.3 MCP naming convention
//! enforcement (`mcp__<server>__<tool>`) and deregister cleanup.

use super::ToolRegistry;
use crate::traits::DescriptionOptions;
use crate::traits::DynTool;
use crate::traits::McpToolInfo;
use crate::traits::Tool;
use coco_messages::ToolResult;
use coco_types::ToolId;
use coco_types::ToolName;
use serde_json::Value;
use std::sync::Arc;

/// Minimal test tool with configurable name + optional MCP info. Used
/// to simulate MCP-backed tools, built-in tools, and edge cases without
/// pulling in real implementations.
struct StubTool {
    name: String,
    mcp: Option<McpToolInfo>,
    should_defer: bool,
    always_load: bool,
}

#[async_trait::async_trait]
impl Tool for StubTool {
    fn runtime_validation_schema(&self) -> &crate::schema::ToolInputSchema {
        crate::schema::test_runtime_schema()
    } // Migration scaffold: assoc types pinned to `Value`.
    type Input = serde_json::Value;
    type Output = serde_json::Value;

    fn id(&self) -> ToolId {
        ToolId::Custom(self.name.clone())
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "stub".into()
    }
    async fn prompt(&self, _options: &crate::traits::PromptOptions) -> String {
        "test tool".into()
    }
    fn mcp_info(&self) -> Option<&McpToolInfo> {
        self.mcp.as_ref()
    }
    fn should_defer(&self) -> bool {
        self.should_defer
    }
    fn always_load(&self) -> bool {
        self.always_load
    }
    async fn execute(
        &self,
        input: Value,
        _ctx: &crate::context::ToolUseContext,
    ) -> Result<ToolResult<Value>, crate::error::ToolError> {
        Ok(ToolResult {
            data: input,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

fn stub(name: &str) -> Arc<StubTool> {
    Arc::new(StubTool {
        name: name.into(),
        mcp: None,
        should_defer: false,
        always_load: false,
    })
}

fn mcp_stub(name: &str, server: &str, mcp_name: &str) -> Arc<StubTool> {
    Arc::new(StubTool {
        name: name.into(),
        mcp: Some(McpToolInfo {
            server_name: server.into(),
            tool_name: mcp_name.into(),
        }),
        should_defer: false,
        always_load: false,
    })
}

/// Build a deferred-by-default MCP-shaped stub for `loaded_tools`
/// partition tests.
fn deferred_mcp_stub(name: &str, server: &str, always_load: bool) -> Arc<StubTool> {
    Arc::new(StubTool {
        name: name.into(),
        mcp: Some(McpToolInfo {
            server_name: server.into(),
            tool_name: name.into(),
        }),
        should_defer: true,
        always_load,
    })
}

// ---------------------------------------------------------------------------
// B3.3: MCP naming convention
// ---------------------------------------------------------------------------

/// Built-in (non-MCP) tools register under their native name. No
/// namespace prefix is added.
#[test]
fn test_register_builtin_tool_keeps_native_name() {
    let reg = ToolRegistry::new();
    reg.register(stub("Read"));
    assert!(reg.get_by_name("Read").is_some());
    assert!(reg.get_by_name("mcp__foo__Read").is_none());
}

/// An MCP tool whose native name already follows the convention is
/// registered as-is (no double-prefixing).
#[test]
fn test_register_mcp_tool_already_qualified() {
    let reg = ToolRegistry::new();
    reg.register(mcp_stub(
        "mcp__slack__send_message",
        "slack",
        "send_message",
    ));
    assert!(reg.get_by_name("mcp__slack__send_message").is_some());
    // Should not get double-wrapped.
    assert!(
        reg.get_by_name("mcp__slack__mcp__slack__send_message")
            .is_none()
    );
}

/// An MCP tool with a native name that doesn't follow the convention
/// (e.g. a hostile server advertising "Read") is re-namespaced to
/// `mcp__<server>__<tool>` so it can't shadow built-in tools.
#[test]
fn test_register_mcp_tool_hostile_name_gets_namespaced() {
    let reg = ToolRegistry::new();
    // First register the real built-in Read so we can verify it's not
    // overwritten.
    reg.register(stub("Read"));

    // MCP tool tries to pretend to be Read. It should land at the
    // qualified name, not overwrite the built-in.
    reg.register(mcp_stub("Read", "evil_server", "Read"));

    // Built-in Read must still resolve correctly.
    let real_read = reg.get_by_name("Read").unwrap();
    assert!(real_read.mcp_info().is_none(), "built-in Read must win");

    // The MCP tool must be accessible under its qualified form.
    let mcp_read = reg.get_by_name("mcp__evil_server__Read").unwrap();
    assert!(mcp_read.mcp_info().is_some());
}

/// Registration order doesn't matter — if the MCP tool comes first and
/// the built-in comes second, the built-in still wins at its native
/// name and the MCP tool is preserved under its qualified form.
#[test]
fn test_register_mcp_then_builtin_both_accessible() {
    let reg = ToolRegistry::new();
    reg.register(mcp_stub("Read", "legit", "Read"));
    reg.register(stub("Read"));

    // Built-in Read claims the native slot.
    let native = reg.get_by_name("Read").unwrap();
    assert!(native.mcp_info().is_none());

    // MCP version is still reachable via qualified name.
    assert!(reg.get_by_name("mcp__legit__Read").is_some());
}

/// `qualified_name()` builds the expected string format.
#[test]
fn test_qualified_name_format() {
    let info = McpToolInfo {
        server_name: "slack".into(),
        tool_name: "send_message".into(),
    };
    assert_eq!(info.qualified_name(), "mcp__slack__send_message");
}

// ---------------------------------------------------------------------------
// Schema-time filter pipeline (docs/coco-rs/feature-gates-and-tool-filtering.md §7)
// ---------------------------------------------------------------------------

/// Stub variant that supports per-instance read-only and feature-gate
/// behavior so we can exercise each filter layer in isolation.
struct GatedTool {
    id: ToolId,
    name: String,
    read_only: bool,
    feature_gate: Option<coco_types::Feature>,
}

#[async_trait::async_trait]
impl Tool for GatedTool {
    fn runtime_validation_schema(&self) -> &crate::schema::ToolInputSchema {
        crate::schema::test_runtime_schema()
    } // Migration scaffold: assoc types pinned to `Value`.
    type Input = serde_json::Value;
    type Output = serde_json::Value;

    fn id(&self) -> ToolId {
        self.id.clone()
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "gated".into()
    }
    async fn prompt(&self, _options: &crate::traits::PromptOptions) -> String {
        "test tool".into()
    }
    fn is_enabled(&self, ctx: &crate::context::ToolUseContext) -> bool {
        match self.feature_gate {
            Some(f) => ctx.features.enabled(f),
            None => true,
        }
    }
    fn is_read_only(&self, _input: &Value) -> bool {
        self.read_only
    }
    async fn execute(
        &self,
        input: Value,
        _ctx: &crate::context::ToolUseContext,
    ) -> Result<ToolResult<Value>, crate::error::ToolError> {
        Ok(ToolResult {
            data: input,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

fn builtin(
    name: ToolName,
    read_only: bool,
    feature_gate: Option<coco_types::Feature>,
) -> Arc<GatedTool> {
    Arc::new(GatedTool {
        id: ToolId::Builtin(name),
        name: name.as_str().to_string(),
        read_only,
        feature_gate,
    })
}

/// Build a registry mirroring the design-doc §8 worked example. Every
/// tool name goes through `ToolName` so a rename surfaces at compile
/// time instead of as a silent test miss.
fn doc_example_registry() -> ToolRegistry {
    let reg = ToolRegistry::new();
    // read-only, no gate
    reg.register(builtin(ToolName::Read, true, None));
    // write tools, no gate
    reg.register(builtin(ToolName::Edit, false, None));
    reg.register(builtin(ToolName::Write, false, None));
    // is_read_only depends on input — `Bash` registers as not always-read-only
    reg.register(builtin(ToolName::Bash, false, None));
    // apply_patch — model-specific tool now in ToolName enum
    reg.register(builtin(ToolName::ApplyPatch, false, None));
    // Feature-gated read-only tools (Layer 1 candidates)
    reg.register(builtin(
        ToolName::WebSearch,
        true,
        Some(coco_types::Feature::WebSearch),
    ));
    reg.register(builtin(
        ToolName::WebFetch,
        true,
        Some(coco_types::Feature::WebFetch),
    ));
    reg
}

fn names(tools: &[Arc<dyn DynTool>]) -> std::collections::HashSet<String> {
    tools.iter().map(|t| t.name().to_string()).collect()
}

#[test]
fn pipeline_layer1_feature_gate_filters_web_tools() {
    let reg = doc_example_registry();
    let mut features = coco_types::Features::with_defaults();
    features.disable(coco_types::Feature::WebSearch);
    features.disable(coco_types::Feature::WebFetch);
    let ctx = crate::context::ToolUseContext::stub_for_filtering(
        Arc::new(features),
        Arc::new(coco_types::ToolOverrides::none()),
        coco_types::ToolFilter::unrestricted(),
        coco_types::PermissionMode::Default,
    );
    let visible = names(&reg.loaded_tools(&ctx));
    assert!(!visible.contains(ToolName::WebSearch.as_str()));
    assert!(!visible.contains(ToolName::WebFetch.as_str()));
    assert!(visible.contains(ToolName::Read.as_str()));
    assert!(visible.contains(ToolName::Edit.as_str()));
}

#[test]
fn pipeline_layer2_tool_overrides_drop_excluded() {
    let reg = doc_example_registry();
    // gpt-5-style diff: excludes Edit (uses apply_patch instead).
    // Other baseline tools stay visible without enumeration — diff
    // model means we only declare the delta, not the full universe.
    let overrides =
        coco_types::ToolOverrides::default().with_excluded(ToolId::Builtin(ToolName::Edit));
    let ctx = crate::context::ToolUseContext::stub_for_filtering(
        Arc::new(coco_types::Features::with_defaults()),
        Arc::new(overrides),
        coco_types::ToolFilter::unrestricted(),
        coco_types::PermissionMode::Default,
    );
    let visible = names(&reg.loaded_tools(&ctx));
    assert!(!visible.contains(ToolName::Edit.as_str()));
    // Baseline tools stay — the diff didn't have to mention them.
    assert!(visible.contains(ToolName::Read.as_str()));
    assert!(visible.contains(ToolName::Bash.as_str()));
}

/// Plan mode does NOT narrow the model's tool schema. Write
/// tools stay visible (the schema is identical to Default mode); plan-mode
/// read-only is enforced at call time by `coco_permissions`, not by
/// removing tools here. Regression guard for the old layer-3 strip that
/// hid ExitPlanMode / AskUserQuestion / Write and broke plan mode.
#[test]
fn plan_mode_keeps_full_tool_schema() {
    let reg = doc_example_registry();
    let ctx = crate::context::ToolUseContext::stub_for_filtering(
        Arc::new(coco_types::Features::with_defaults()),
        Arc::new(coco_types::ToolOverrides::none()),
        coco_types::ToolFilter::unrestricted(),
        coco_types::PermissionMode::Plan,
    );
    let visible = names(&reg.loaded_tools(&ctx));
    // Read-only tools present (as always).
    assert!(visible.contains(ToolName::Read.as_str()));
    assert!(visible.contains(ToolName::WebSearch.as_str()));
    assert!(visible.contains(ToolName::WebFetch.as_str()));
    // Write tools are NOT stripped in plan mode any more.
    assert!(visible.contains(ToolName::Edit.as_str()));
    assert!(visible.contains(ToolName::Write.as_str()));
    assert!(visible.contains(ToolName::ApplyPatch.as_str()));
    assert!(visible.contains(ToolName::Bash.as_str()));
}

#[test]
fn pipeline_agent_filter_narrows() {
    let reg = doc_example_registry();
    let filter = coco_types::ToolFilter::new(
        vec![
            ToolName::Read.as_str().to_string(),
            ToolName::Bash.as_str().to_string(),
        ],
        Vec::new(),
    );
    let ctx = crate::context::ToolUseContext::stub_for_filtering(
        Arc::new(coco_types::Features::with_defaults()),
        Arc::new(coco_types::ToolOverrides::none()),
        filter,
        coco_types::PermissionMode::Default,
    );
    let visible = names(&reg.loaded_tools(&ctx));
    let expected: std::collections::HashSet<String> =
        [ToolName::Read.as_str(), ToolName::Bash.as_str()]
            .iter()
            .map(ToString::to_string)
            .collect();
    assert_eq!(visible, expected);
}

/// End-to-end design-doc §8 trace: gpt-5 + Plan mode.
///
/// Plan mode no longer narrows the schema, so the visible set
/// is exactly the gpt-5 tool universe — `Edit` and `Write` excluded,
/// `apply_patch` added (it covers both edits and new files), `Bash`
/// present. Plan-mode read-only is a call-time permission concern, not a
/// schema filter.
#[test]
fn pipeline_design_doc_gpt5_plan_mode_trace() {
    let reg = doc_example_registry();

    // Layer 1 — features all default ON.
    let features = coco_types::Features::with_defaults();

    // Layer 2 — gpt-5 diff: extra apply_patch (now a typed ToolName
    // variant), excluded Edit + Write. All go in via `ToolId::Builtin`.
    let overrides = coco_types::ToolOverrides::default()
        .with_extra(ToolId::Builtin(ToolName::ApplyPatch))
        .with_excluded(ToolId::Builtin(ToolName::Edit))
        .with_excluded(ToolId::Builtin(ToolName::Write));

    // Plan mode — no schema narrowing.
    // Top-level session, no agent restriction.
    let ctx = crate::context::ToolUseContext::stub_for_filtering(
        Arc::new(features),
        Arc::new(overrides),
        coco_types::ToolFilter::unrestricted(),
        coco_types::PermissionMode::Plan,
    );

    let visible = names(&reg.loaded_tools(&ctx));
    let expected: std::collections::HashSet<String> = [
        ToolName::Read.as_str(),
        ToolName::Bash.as_str(),
        ToolName::ApplyPatch.as_str(),
        ToolName::WebSearch.as_str(),
        ToolName::WebFetch.as_str(),
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        visible, expected,
        "plan mode keeps the full gpt-5 schema (Edit + Write excluded, apply_patch present)"
    );
}

/// When `Feature::ToolSearch` is **off**, the deferral filter
/// is bypassed entirely: every deferred tool gets full schema on turn 1
/// and the deferred pool is empty. Mirrors the user-facing `settings.json`
/// toggle `features.tool_search = false` that lets sessions with few
/// deferrable tools skip the lazy-loading round-trip.
#[test]
fn tool_search_disabled_loads_every_deferred_tool_eagerly() {
    let reg = ToolRegistry::new();
    reg.register(deferred_mcp_stub(
        "mcp__notes__list",
        "notes",
        /*always_load=*/ false,
    ));
    reg.register(stub("Read")); // eager built-in

    let mut features = coco_types::Features::with_defaults();
    features.disable(coco_types::Feature::ToolSearch);
    let ctx = crate::context::ToolUseContext::stub_for_filtering(
        Arc::new(features),
        Arc::new(coco_types::ToolOverrides::none()),
        coco_types::ToolFilter::unrestricted(),
        coco_types::PermissionMode::Default,
    );

    let loaded = names(&reg.loaded_tools(&ctx));
    let deferred = names(&reg.deferred_tools(&ctx));

    assert!(
        loaded.contains("mcp__notes__list"),
        "feature off → deferred tool must surface eagerly"
    );
    assert!(loaded.contains("Read"));
    assert!(
        deferred.is_empty(),
        "feature off → deferred pool must be empty: {deferred:?}"
    );
}

/// Sanity: with the feature on AND the model declaring a
/// capability, the deferred filter still hides the would-be-deferred
/// tool. Catches regressions where the disabled-branch logic leaks
/// into the enabled path.
#[test]
fn tool_search_enabled_keeps_deferred_pool() {
    let reg = ToolRegistry::new();
    reg.register(deferred_mcp_stub(
        "mcp__notes__list",
        "notes",
        /*always_load=*/ false,
    ));

    let ctx = crate::context::ToolUseContext::stub_for_filtering(
        Arc::new(coco_types::Features::with_defaults()),
        Arc::new(coco_types::ToolOverrides::none()),
        coco_types::ToolFilter::unrestricted(),
        coco_types::PermissionMode::Default,
    )
    // Declare client-side capability so `tool_search_active()` is
    // true — feature alone isn't enough now that we three-state.
    .with_model_capabilities(false, true)
    .with_tool_search_candidates(true);

    let loaded = names(&reg.loaded_tools(&ctx));
    let deferred = names(&reg.deferred_tools(&ctx));
    assert!(!loaded.contains("mcp__notes__list"));
    assert!(deferred.contains("mcp__notes__list"));
}

/// Three-state coverage: feature ON but model lacks both
/// capabilities → deferral filter short-circuits like
/// `Feature::ToolSearch = false`. Surface every tool eagerly.
#[test]
fn tool_search_inactive_when_model_lacks_capability() {
    let reg = ToolRegistry::new();
    reg.register(deferred_mcp_stub(
        "mcp__notes__list",
        "notes",
        /*always_load=*/ false,
    ));

    let ctx = crate::context::ToolUseContext::stub_for_filtering(
        Arc::new(coco_types::Features::with_defaults()),
        Arc::new(coco_types::ToolOverrides::none()),
        coco_types::ToolFilter::unrestricted(),
        coco_types::PermissionMode::Default,
    );
    // No `with_model_capabilities` call → both flags default false.

    let loaded = names(&reg.loaded_tools(&ctx));
    let deferred = names(&reg.deferred_tools(&ctx));
    assert!(
        loaded.contains("mcp__notes__list"),
        "no capability → deferred tool must surface eagerly"
    );
    assert!(
        deferred.is_empty(),
        "no capability → deferred pool must be empty: {deferred:?}"
    );
}

/// `always_load` (`_meta["anthropic/alwaysLoad"]` opt-out) must
/// surface the tool in `loaded_tools` on turn 1 even when
/// `should_defer() == true`. Symmetric: `deferred_tools` must omit it.
#[test]
fn loaded_tools_includes_always_load_mcp_tool_on_turn_one() {
    let reg = ToolRegistry::new();
    reg.register(deferred_mcp_stub(
        "mcp__notes__pin",
        "notes",
        /*always_load=*/ true,
    ));
    reg.register(deferred_mcp_stub(
        "mcp__notes__list",
        "notes",
        /*always_load=*/ false,
    ));
    reg.register(stub("Read")); // eager built-in — always loaded

    let ctx = crate::context::ToolUseContext::stub_for_filtering(
        Arc::new(coco_types::Features::with_defaults()),
        Arc::new(coco_types::ToolOverrides::none()),
        coco_types::ToolFilter::unrestricted(),
        coco_types::PermissionMode::Default,
    )
    // Declare a capability so `tool_search_active()` is true and the
    // deferral filter actually runs (the always-load short-circuit
    // is what the test is asserting).
    .with_model_capabilities(false, true)
    .with_tool_search_candidates(true);

    let loaded = names(&reg.loaded_tools(&ctx));
    let deferred = names(&reg.deferred_tools(&ctx));

    // alwaysLoad MCP tool short-circuits the deferral filter.
    assert!(loaded.contains("mcp__notes__pin"));
    assert!(!deferred.contains("mcp__notes__pin"));
    // Regular MCP tool stays deferred until ToolSearch discovers it.
    assert!(!loaded.contains("mcp__notes__list"));
    assert!(deferred.contains("mcp__notes__list"));
    // Sanity: eager built-in is always loaded.
    assert!(loaded.contains("Read"));
}

/// Deregister-by-server must find tools by their MCP info, regardless
/// of whether they're registered under the qualified name.
#[test]
fn test_deregister_by_server_removes_namespaced_tools() {
    let reg = ToolRegistry::new();
    reg.register(mcp_stub("ls", "myserver", "ls"));
    reg.register(mcp_stub("mcp__other__read", "other", "read"));
    reg.register(stub("Read")); // built-in — must survive

    assert!(reg.get_by_name("mcp__myserver__ls").is_some());

    reg.deregister_by_server("myserver");

    // Only myserver's tool is gone.
    assert!(reg.get_by_name("mcp__myserver__ls").is_none());
    assert!(reg.get_by_name("mcp__other__read").is_some());
    assert!(reg.get_by_name("Read").is_some());
}
