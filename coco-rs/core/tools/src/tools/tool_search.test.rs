//! Tests for the ToolSearch tool.
//!
//! Three test groups:
//!   1. `parse_select_query` — the select-mode prefix parser.
//!   2. `render_for_model` — envelope rendering.
//!   3. `execute` — end-to-end coverage of select + keyword modes,
//!      weighted scoring, `+keyword` required terms, and the
//!      `app_state_patch` promotion mechanism.

use super::parse_select_query;

// ---------------------------------------------------------------------------
// B3.2: ToolSearch select: syntax
// ---------------------------------------------------------------------------

#[test]
fn test_parse_select_query_basic() {
    assert_eq!(
        parse_select_query("select:Read,Grep"),
        Some(vec!["Read".into(), "Grep".into()])
    );
}

#[test]
fn test_parse_select_query_whitespace_tolerant() {
    assert_eq!(
        parse_select_query("select: Read , Grep , Glob "),
        Some(vec!["Read".into(), "Grep".into(), "Glob".into()])
    );
}

#[test]
fn test_parse_select_query_single_tool() {
    assert_eq!(parse_select_query("select:Bash"), Some(vec!["Bash".into()]));
}

#[test]
fn test_parse_select_query_drops_empty_entries() {
    assert_eq!(
        parse_select_query("select:Read,,Grep, "),
        Some(vec!["Read".into(), "Grep".into()])
    );
}

#[test]
fn test_parse_select_query_not_select_prefix() {
    assert_eq!(parse_select_query("rust async"), None);
    assert_eq!(parse_select_query("selectable"), None);
    assert_eq!(parse_select_query(""), None);
}

#[test]
fn test_parse_select_query_empty_after_prefix() {
    // `select:` with nothing after is still "select mode" but with no
    // tools — the execute path will reject it. 7 chars exactly.
    assert_eq!(parse_select_query("select:"), Some(vec![]));
}

/// The prefix match is case-insensitive: `/^select:(.+)$/i`.
/// `Select:`, `SELECT:`, `SeLeCt:` all trigger select mode.
#[test]
fn test_parse_select_query_case_insensitive_prefix() {
    assert_eq!(parse_select_query("Select:Read"), Some(vec!["Read".into()]));
    assert_eq!(
        parse_select_query("SELECT:Read,Grep"),
        Some(vec!["Read".into(), "Grep".into()])
    );
    assert_eq!(parse_select_query("SeLeCt:Bash"), Some(vec!["Bash".into()]));
}

/// The tool NAMES after the prefix are NOT lowercased — only the prefix
/// itself is case-insensitive. Tool lookup uses case-insensitive matching.
#[test]
fn test_parse_select_query_preserves_tool_name_case() {
    assert_eq!(
        parse_select_query("SELECT:MyCustomTool"),
        Some(vec!["MyCustomTool".into()])
    );
}

// ── render_for_model — ToolSearch envelopes ─────────────────────────────

mod render_tests {
    use super::super::ToolSearchTool;
    use coco_tool_runtime::DynTool;

    use coco_tool_runtime::ToolResultContentPart;
    use serde_json::json;

    #[test]
    fn matches_emits_text_list() {
        let data = json!({
            "matches": ["Read", "Grep"],
            "query": "file",
            "total_deferred_tools": 12,
        });
        let parts = <ToolSearchTool as DynTool>::render_for_model(&ToolSearchTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(
            text.starts_with("Matched tools (schemas will be available next turn):"),
            "got: {text}"
        );
        assert!(text.contains("Read"), "got: {text}");
        assert!(text.contains("Grep"), "got: {text}");
    }

    #[test]
    fn empty_matches_without_pending_uses_bare_message() {
        // Returns `'No matching deferred tools found'` (no trailing period).
        let data = json!({
            "matches": [],
            "query": "missing",
            "total_deferred_tools": 0,
        });
        let parts = <ToolSearchTool as DynTool>::render_for_model(&ToolSearchTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(text, "No matching deferred tools found");
    }

    #[test]
    fn empty_matches_with_pending_appends_retry_hint() {
        // Appends a `. Some MCP servers ...` suffix when servers are still
        // in handshake. The list is joined with `, ` and the suffix ends
        // with a period.
        let data = json!({
            "matches": [],
            "query": "missing",
            "total_deferred_tools": 0,
            "pending_mcp_servers": ["server-a", "server-b"],
        });
        let parts = <ToolSearchTool as DynTool>::render_for_model(&ToolSearchTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(
            text.starts_with("No matching deferred tools found. Some MCP servers are still connecting: server-a, server-b."),
            "got: {text}"
        );
        assert!(text.ends_with("try searching again."), "got: {text}");
    }

    #[test]
    fn matches_with_tool_reference_flag_emits_custom_parts() {
        let data = json!({
            "matches": ["WebFetch", "WebSearch"],
            "query": "fetch",
            "total_deferred_tools": 12,
            "render_as_tool_reference": true,
        });
        let parts = <ToolSearchTool as DynTool>::render_for_model(&ToolSearchTool, &data);
        assert_eq!(parts.len(), 2);

        for (idx, expected_name) in ["WebFetch", "WebSearch"].iter().enumerate() {
            let ToolResultContentPart::Custom { provider_options } = &parts[idx] else {
                panic!("expected Custom part at index {idx}, got {:?}", parts[idx]);
            };
            let po = provider_options.as_ref().expect("provider_options present");
            let anthropic = po.0.get("anthropic").expect("anthropic ns");
            assert_eq!(
                anthropic.get("type").and_then(|v| v.as_str()),
                Some("tool-reference"),
            );
            assert_eq!(
                anthropic.get("toolName").and_then(|v| v.as_str()),
                Some(*expected_name),
            );
        }
    }

    #[test]
    fn empty_matches_with_tool_reference_flag_still_uses_text_branch() {
        // No matches → no `tool_reference` blocks even on capable
        // models; the empty-result message must still render so the
        // model knows the search failed (and that an MCP server may
        // be mid-handshake).
        let data = json!({
            "matches": [],
            "query": "missing",
            "total_deferred_tools": 0,
            "render_as_tool_reference": true,
        });
        let parts = <ToolSearchTool as DynTool>::render_for_model(&ToolSearchTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part for empty match, got {:?}", parts[0]);
        };
        assert_eq!(text, "No matching deferred tools found");
    }

    #[test]
    fn empty_matches_with_empty_pending_array_omits_suffix() {
        let data = json!({
            "matches": [],
            "query": "missing",
            "total_deferred_tools": 0,
            "pending_mcp_servers": [],
        });
        let parts = <ToolSearchTool as DynTool>::render_for_model(&ToolSearchTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(text, "No matching deferred tools found");
    }
}

// ── execute — select + keyword + scoring + promotion ──────────────────

mod execute_tests {
    use super::super::ToolSearchTool;
    use async_trait::async_trait;
    use coco_messages::ToolResult;
    use coco_tool_runtime::DescriptionOptions;
    use coco_tool_runtime::DynTool;
    use coco_tool_runtime::Tool;
    use coco_tool_runtime::ToolError;
    use coco_tool_runtime::ToolRegistry;
    use coco_tool_runtime::ToolUseContext;
    use coco_types::ToolId;
    use serde_json::Value;
    use serde_json::json;
    use std::sync::Arc;

    /// Lightweight deferrable tool stub. `deferred` toggles the
    /// trait default; `hint` and `desc` drive the scoring path.
    struct StubTool {
        name: String,
        desc: String,
        hint: Option<&'static str>,
        deferred: bool,
    }

    #[async_trait]
    impl Tool for StubTool {
        fn runtime_validation_schema(&self) -> &coco_tool_runtime::ToolInputSchema {
            static S: std::sync::OnceLock<coco_tool_runtime::ToolInputSchema> =
                std::sync::OnceLock::new();
            S.get_or_init(|| {
                coco_tool_runtime::ToolInputSchema::from_value(serde_json::json!({"type":"object"}))
                    .expect("schema")
            })
        }
        // Migration scaffold: assoc types pinned to `Value`.
        type Input = serde_json::Value;
        type Output = serde_json::Value;

        fn id(&self) -> ToolId {
            ToolId::Custom(self.name.clone())
        }
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
            self.desc.clone()
        }
        async fn prompt(&self, _: &coco_tool_runtime::PromptOptions) -> String {
            self.desc.clone()
        }
        fn search_hint(&self) -> Option<&str> {
            self.hint
        }
        fn should_defer(&self) -> bool {
            self.deferred
        }
        async fn execute(
            &self,
            _input: Value,
            _ctx: &ToolUseContext,
        ) -> Result<ToolResult<Value>, ToolError> {
            Ok(ToolResult {
                data: Value::Null,
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            })
        }
    }

    fn deferred(name: &str, desc: &str, hint: Option<&'static str>) -> Arc<StubTool> {
        Arc::new(StubTool {
            name: name.to_string(),
            desc: desc.to_string(),
            hint,
            deferred: true,
        })
    }

    fn eager(name: &str, desc: &str) -> Arc<StubTool> {
        Arc::new(StubTool {
            name: name.to_string(),
            desc: desc.to_string(),
            hint: None,
            deferred: false,
        })
    }

    /// Build a context whose registry holds the given tools. The
    /// `ToolSearch` tool itself is not registered — `execute` only
    /// consults `ctx.tools.all()`, not `ctx.tools.get_by_name(...)`.
    fn ctx_with_tools(tools: Vec<Arc<dyn DynTool>>) -> ToolUseContext {
        let registry = ToolRegistry::new();
        for t in tools {
            registry.register(t);
        }
        let mut ctx = ToolUseContext::test_default();
        ctx.tools = Arc::new(registry);
        ctx
    }

    #[tokio::test]
    async fn select_mode_returns_matched_names_and_emits_patch() {
        let ctx = ctx_with_tools(vec![
            deferred("WebFetch", "Fetch URL", Some("fetch a URL")),
            deferred("WebSearch", "Search the web", Some("search the web")),
            eager("Read", "Read a file"),
        ]);
        let result = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "select:WebFetch,WebSearch"}),
            &ctx,
        )
        .await
        .expect("select executes");
        // matches: exact resolved names from the deferred pool.
        let matches: Vec<&str> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(matches, vec!["WebFetch", "WebSearch"]);
        assert_eq!(result.data["query"], json!("select:WebFetch,WebSearch"));
        assert_eq!(result.data["total_deferred_tools"], json!(2));
        // The patch carries the promotion side-effect — apply it and
        // assert the discovery set picked up both names.
        let patch = result.app_state_patch.expect("non-empty match emits patch");
        let mut state = coco_types::ToolAppState::default();
        patch(&mut state);
        assert!(state.discovered_tool_names.contains("WebFetch"));
        assert!(state.discovered_tool_names.contains("WebSearch"));
    }

    #[tokio::test]
    async fn select_mode_drops_unknown_names_silently() {
        let ctx = ctx_with_tools(vec![deferred("WebFetch", "Fetch URL", None)]);
        let result = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "select:WebFetch,NonExistent"}),
            &ctx,
        )
        .await
        .expect("select executes");
        let matches: Vec<&str> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(matches, vec!["WebFetch"]);
    }

    #[tokio::test]
    async fn select_mode_falls_back_to_full_pool_when_already_loaded() {
        let ctx = ctx_with_tools(vec![eager("Read", "Read a file")]);
        let result = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "select:Read"}),
            &ctx,
        )
        .await
        .expect("select executes");
        let matches: Vec<&str> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(matches, vec!["Read"]);
    }

    /// A deferred tool that fails the filter pipeline (here: excluded via
    /// `ToolOverrides`) must NOT be matched — neither by `select:` nor by
    /// keyword. Matching an unsurfaceable tool is inert (it can't enter
    /// `loaded_tools`) and makes the model loop. Regression guard for the
    /// pre-fix pool that ignored `passes_filter_pipeline`.
    #[tokio::test]
    async fn search_pool_excludes_filtered_out_deferred_tools() {
        let mut ctx = ctx_with_tools(vec![
            deferred("WebFetch", "Fetch a URL", Some("fetch a URL")),
            deferred("WebSearch", "Search the web", Some("search the web")),
        ]);
        // Model-level exclusion of WebFetch (Layer 2 of the pipeline).
        ctx.tool_overrides = Arc::new(
            coco_types::ToolOverrides::default().with_excluded(ToolId::Custom("WebFetch".into())),
        );

        // select: drops the excluded tool silently; the pool count reflects
        // only surfaceable tools.
        let sel = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "select:WebFetch,WebSearch"}),
            &ctx,
        )
        .await
        .expect("select executes");
        let sel_matches: Vec<&str> = sel.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            sel_matches,
            vec!["WebSearch"],
            "excluded WebFetch must not match"
        );
        assert_eq!(
            sel.data["total_deferred_tools"],
            json!(1),
            "pool counts only pipeline-passing deferred tools"
        );

        // keyword exact-name must also drop it (fallback corpus is the
        // enabled set, which excludes WebFetch).
        let kw = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "WebFetch"}),
            &ctx,
        )
        .await
        .expect("keyword executes");
        let kw_matches = kw.data["matches"].as_array().unwrap();
        assert!(
            kw_matches.is_empty(),
            "excluded WebFetch must not match by exact name: {kw_matches:?}"
        );
    }

    #[tokio::test]
    async fn select_mode_rejects_empty_name_list() {
        let ctx = ctx_with_tools(vec![]);
        let err = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "select:"}),
            &ctx,
        )
        .await
        .expect_err("empty select must error");
        assert!(matches!(err, ToolError::InvalidInput { .. }));
    }

    #[tokio::test]
    async fn keyword_exact_name_fast_path() {
        // A bare tool name (no `select:` prefix) returns that tool directly.
        // Useful for subagents that emit a name without the prefix.
        let ctx = ctx_with_tools(vec![
            deferred("WebFetch", "Fetch a URL", None),
            deferred("WebSearch", "Search the web", None),
        ]);
        let result = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "WebFetch"}),
            &ctx,
        )
        .await
        .expect("keyword executes");
        let matches: Vec<&str> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(matches, vec!["WebFetch"]);
    }

    #[tokio::test]
    async fn keyword_mcp_prefix_fast_path() {
        // `mcp__server` prefix returns all matching MCP tools.
        let ctx = ctx_with_tools(vec![
            deferred("mcp__slack__send_message", "Slack send", None),
            deferred("mcp__slack__list_channels", "Slack list", None),
            deferred("mcp__github__create_issue", "GH issue", None),
        ]);
        let result = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "mcp__slack"}),
            &ctx,
        )
        .await
        .expect("keyword executes");
        let matches: Vec<String> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().all(|m| m.starts_with("mcp__slack__")));
    }

    #[tokio::test]
    async fn keyword_scoring_ranks_part_match_over_description_match() {
        // Two tools — one matches the name part `notebook`, the other
        // mentions `notebook` only in the description. The first
        // should rank higher.
        let ctx = ctx_with_tools(vec![
            deferred("NotebookEdit", "Edit a cell", None),
            deferred("EditFile", "Edit a notebook file", None),
        ]);
        let result = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "notebook"}),
            &ctx,
        )
        .await
        .expect("keyword executes");
        let matches: Vec<&str> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            matches[0], "NotebookEdit",
            "name part hit ranks first: {matches:?}"
        );
    }

    #[tokio::test]
    async fn keyword_required_term_filters_candidates() {
        // `+slack` requires the term `slack` in the name / description /
        // hint. `send` is an optional ranking term that does not
        // require all candidates to mention it.
        let ctx = ctx_with_tools(vec![
            deferred("mcp__slack__send_message", "Send a message", None),
            deferred("mcp__github__create_issue", "Create an issue", None),
            deferred("mcp__slack__list_channels", "List channels", None),
        ]);
        let result = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "+slack send"}),
            &ctx,
        )
        .await
        .expect("keyword executes");
        let matches: Vec<String> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        // The GH tool is filtered out (no `slack`); the two slack
        // tools survive. `send_message` ranks higher because it
        // matches `send` too.
        assert!(
            matches.iter().all(|m| m.contains("slack")),
            "+slack should filter out github: {matches:?}",
        );
        assert_eq!(
            matches.first().map(String::as_str),
            Some("mcp__slack__send_message"),
            "send_message ranks first: {matches:?}",
        );
    }

    #[tokio::test]
    async fn keyword_scoring_excludes_eager_tools() {
        // Eager tools (`should_defer() == false`) are NOT in the
        // scoring pool — the model already has their schema. Only
        // the exact-name fast path falls back to the full pool (harmless
        // no-op — see `keyword_exact_name_fast_path`).
        //
        // Pick a non-exact-name query to exercise the scoring path
        // so eager tools never appear.
        let ctx = ctx_with_tools(vec![
            eager("ReadFile", "Read content from a file"),
            deferred("WebFetch", "Fetch a URL", None),
        ]);
        let result =
            <ToolSearchTool as DynTool>::execute(&ToolSearchTool, json!({"query": "file"}), &ctx)
                .await
                .expect("keyword executes");
        let matches: Vec<&str> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            !matches.contains(&"ReadFile"),
            "ReadFile is eager, scoring pool must exclude it: {matches:?}",
        );
    }

    #[tokio::test]
    async fn keyword_max_results_caps_returned_list() {
        let ctx = ctx_with_tools(vec![
            deferred("TaskCreate", "create task", None),
            deferred("TaskGet", "get task", None),
            deferred("TaskList", "list tasks", None),
            deferred("TaskUpdate", "update task", None),
        ]);
        let result = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "task", "max_results": 2}),
            &ctx,
        )
        .await
        .expect("keyword executes");
        let matches = result.data["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[tokio::test]
    async fn keyword_ties_sort_by_canonical_tool_name() {
        let ctx = ctx_with_tools(vec![
            deferred("ZetaAlpha", "alpha helper", None),
            deferred("AlphaTool", "alpha helper", None),
        ]);
        let result =
            <ToolSearchTool as DynTool>::execute(&ToolSearchTool, json!({"query": "alpha"}), &ctx)
                .await
                .expect("keyword executes");
        let matches: Vec<&str> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(matches, vec!["AlphaTool", "ZetaAlpha"]);
    }

    #[tokio::test]
    async fn empty_query_is_rejected() {
        let ctx = ctx_with_tools(vec![]);
        let err = <ToolSearchTool as DynTool>::execute(&ToolSearchTool, json!({"query": ""}), &ctx)
            .await
            .expect_err("empty query must error");
        assert!(matches!(err, ToolError::InvalidInput { .. }));
    }

    #[tokio::test]
    async fn keyword_match_emits_promotion_patch() {
        let ctx = ctx_with_tools(vec![deferred("WebFetch", "Fetch a URL", None)]);
        let result =
            <ToolSearchTool as DynTool>::execute(&ToolSearchTool, json!({"query": "fetch"}), &ctx)
                .await
                .expect("keyword executes");
        let patch = result.app_state_patch.expect("non-empty match emits patch");
        let mut state = coco_types::ToolAppState::default();
        patch(&mut state);
        assert!(state.discovered_tool_names.contains("WebFetch"));
    }

    #[tokio::test]
    async fn keyword_no_match_emits_no_patch() {
        let ctx = ctx_with_tools(vec![deferred("WebFetch", "Fetch a URL", None)]);
        let result = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "totally-unrelated-query"}),
            &ctx,
        )
        .await
        .expect("keyword executes");
        assert!(result.app_state_patch.is_none());
        let matches = result.data["matches"].as_array().unwrap();
        assert!(matches.is_empty());
    }

    // ── ServerSideToolReference capability — emission path ─

    /// Server-side capable ctx (Anthropic Sonnet 4.5+/Opus 4+).
    /// `model_supports_client_side_tool_search` is also true because
    /// every server-side-capable model can run the client-side
    /// fallback if the beta header ever fails to negotiate.
    fn ctx_with_tools_capable(tools: Vec<Arc<dyn DynTool>>) -> ToolUseContext {
        let mut ctx = ctx_with_tools(tools);
        ctx.model_supports_tool_reference = true;
        ctx.model_supports_client_side_tool_search = true;
        ctx.tool_search_has_candidates = true;
        ctx
    }

    /// Client-side-only capable ctx (GPT-5, Gemini, DeepSeek, Haiku).
    /// Used to verify the universal promotion path remains active
    /// when the model only declares `ClientSideToolSearch`.
    fn ctx_with_tools_client_capable(tools: Vec<Arc<dyn DynTool>>) -> ToolUseContext {
        let mut ctx = ctx_with_tools(tools);
        ctx.model_supports_client_side_tool_search = true;
        ctx.tool_search_has_candidates = true;
        ctx
    }

    /// When the model supports `tool_reference` expansion, the
    /// envelope is tagged `render_as_tool_reference: true` and the
    /// promotion patch is **suppressed** — discovery state lives in
    /// the messages array (`tool_reference` blocks) rather than the
    /// `ToolAppState`.
    #[tokio::test]
    async fn capable_model_select_skips_patch_and_tags_envelope() {
        let ctx =
            ctx_with_tools_capable(vec![deferred("WebFetch", "Fetch URL", Some("fetch a URL"))]);
        let result = <ToolSearchTool as DynTool>::execute(
            &ToolSearchTool,
            json!({"query": "select:WebFetch"}),
            &ctx,
        )
        .await
        .expect("select executes");
        assert_eq!(result.data["matches"], json!(["WebFetch"]));
        assert_eq!(result.data["render_as_tool_reference"], json!(true));
        assert!(
            result.app_state_patch.is_none(),
            "patch must be suppressed on the tool_reference path"
        );
    }

    /// Keyword search on a capable model: same suppression rule.
    #[tokio::test]
    async fn capable_model_keyword_skips_patch_and_tags_envelope() {
        let ctx = ctx_with_tools_capable(vec![deferred("WebFetch", "Fetch a URL", None)]);
        let result =
            <ToolSearchTool as DynTool>::execute(&ToolSearchTool, json!({"query": "fetch"}), &ctx)
                .await
                .expect("keyword executes");
        let matches: Vec<&str> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(matches, vec!["WebFetch"]);
        assert_eq!(result.data["render_as_tool_reference"], json!(true));
        assert!(result.app_state_patch.is_none());
    }

    /// `Feature::ToolSearch` is the user-facing on/off switch. When the feature is
    /// disabled the tool must hide itself from the model — symmetric
    /// with `ToolRegistry::loaded_tools` short-circuiting the
    /// deferral filter so every tool's schema lands in turn 1.
    ///
    /// Sets a client-side capability so the feature flag is the only
    /// differentiator under test; a model with zero capabilities is
    /// covered by `tool_search_tool_hidden_when_no_capability` below.
    #[tokio::test]
    async fn tool_search_tool_hidden_when_feature_off() {
        let mut ctx =
            ctx_with_tools_client_capable(vec![deferred("WebFetch", "Fetch a URL", None)]);
        assert!(
            <ToolSearchTool as DynTool>::is_enabled(&ToolSearchTool, &ctx),
            "feature on + client-side cap → ToolSearch exposed"
        );

        let mut disabled = coco_types::Features::with_defaults();
        disabled.disable(coco_types::Feature::ToolSearch);
        ctx.features = Arc::new(disabled);
        assert!(
            !<ToolSearchTool as DynTool>::is_enabled(&ToolSearchTool, &ctx),
            "feature off → ToolSearch hidden even with client-side cap"
        );
    }

    /// Three-state predicate: feature on, but the model declares
    /// neither capability. The tool must hide (safe degradation
    /// path) and the registry must surface every tool eagerly.
    /// Catches the regression of "user enabled ToolSearch globally
    /// but my custom local model breaks under it".
    #[tokio::test]
    async fn tool_search_tool_hidden_when_no_capability() {
        let ctx = ctx_with_tools(vec![]);
        // `ctx_with_tools` → defaults: feature on, no capability.
        assert!(ctx.features.enabled(coco_types::Feature::ToolSearch));
        assert!(!ctx.model_supports_tool_reference);
        assert!(!ctx.model_supports_client_side_tool_search);
        assert!(
            !<ToolSearchTool as DynTool>::is_enabled(&ToolSearchTool, &ctx),
            "no capability → ToolSearch must hide regardless of feature flag"
        );
        assert!(!ctx.tool_search_active());
    }

    /// Client-side-only capable model: text envelope + promotion patch.
    /// Pinned to make sure capability gating doesn't regress the
    /// client-side path other providers rely on (GPT-5, Gemini,
    /// DeepSeek, Haiku — every model that declares only
    /// `ClientSideToolSearch`).
    #[tokio::test]
    async fn client_side_only_model_keeps_patch_and_omits_tag() {
        let ctx = ctx_with_tools_client_capable(vec![deferred("WebFetch", "Fetch a URL", None)]);
        let result =
            <ToolSearchTool as DynTool>::execute(&ToolSearchTool, json!({"query": "fetch"}), &ctx)
                .await
                .expect("keyword executes");
        assert!(result.data.get("render_as_tool_reference").is_none());
        let patch = result
            .app_state_patch
            .expect("client-side path must keep the discovery patch");
        let mut state = coco_types::ToolAppState::default();
        patch(&mut state);
        assert!(state.discovered_tool_names.contains("WebFetch"));
    }

    #[tokio::test]
    async fn client_side_model_returns_stable_functions_schema_text() {
        let ctx = ctx_with_tools_client_capable(vec![
            deferred("BetaTool", "alpha helper", None),
            deferred("AlphaTool", "alpha helper", None),
        ]);
        let first =
            <ToolSearchTool as DynTool>::execute(&ToolSearchTool, json!({"query": "alpha"}), &ctx)
                .await
                .expect("first search executes");
        let second =
            <ToolSearchTool as DynTool>::execute(&ToolSearchTool, json!({"query": "alpha"}), &ctx)
                .await
                .expect("second search executes");

        let first_schema = first.data["rendered_functions"]
            .as_str()
            .expect("client-side rendered functions");
        let second_schema = second.data["rendered_functions"]
            .as_str()
            .expect("client-side rendered functions");
        assert_eq!(first_schema, second_schema);
        assert!(first_schema.starts_with("<functions>\n"));
        assert!(first_schema.ends_with("\n</functions>"));
        assert!(
            first_schema
                .contains("<function>{\"description\":\"alpha helper\",\"name\":\"AlphaTool\""),
            "AlphaTool schema must be wrapped in <function>: {first_schema}"
        );
        assert!(
            first_schema.contains(
                "</function>\n<function>{\"description\":\"alpha helper\",\"name\":\"BetaTool\""
            ),
            "each schema should render as a separate <function> line: {first_schema}"
        );
        let alpha_index = first_schema
            .find("<function>{\"description\":\"alpha helper\",\"name\":\"AlphaTool\"")
            .expect("AlphaTool rendered");
        let beta_index = first_schema
            .find("<function>{\"description\":\"alpha helper\",\"name\":\"BetaTool\"")
            .expect("BetaTool rendered");
        assert!(
            alpha_index < beta_index,
            "schemas should render in canonical name order: {first_schema}"
        );

        let parts = <ToolSearchTool as DynTool>::render_for_model(&ToolSearchTool, &first.data);
        let coco_tool_runtime::ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected rendered functions text");
        };
        assert_eq!(text, first_schema);
    }
}

// ── parse_tool_name — decomposition ────────────────────────

mod parse_name_tests {
    use super::super::parse_tool_name;

    #[test]
    fn camel_case_split() {
        let p = parse_tool_name("NotebookEdit");
        assert_eq!(p.parts, vec!["notebook", "edit"]);
        assert_eq!(p.full, "notebook edit");
        assert!(!p.is_mcp);
    }

    #[test]
    fn snake_case_split() {
        let p = parse_tool_name("read_file");
        assert_eq!(p.parts, vec!["read", "file"]);
        assert_eq!(p.full, "read file");
        assert!(!p.is_mcp);
    }

    #[test]
    fn mcp_double_underscore_split() {
        let p = parse_tool_name("mcp__slack__send_message");
        assert!(p.is_mcp);
        assert_eq!(p.parts, vec!["slack", "send", "message"]);
        assert_eq!(p.full, "slack send message");
    }

    #[test]
    fn mcp_no_inner_underscore() {
        let p = parse_tool_name("mcp__github__list");
        assert!(p.is_mcp);
        assert_eq!(p.parts, vec!["github", "list"]);
        assert_eq!(p.full, "github list");
    }
}
