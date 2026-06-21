use coco_messages::ToolResult;
use coco_messages::ToolResultContentPart;
use coco_types::ToolCheckResult;
use coco_types::ToolId;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::context::ToolUseContext;
use crate::error::ToolError;
use crate::validation::ValidationResult;

/// Session context for [`Tool::tool_spec`]. Carries
/// the per-session knobs that drive dynamic schema omits
/// (e.g. the `isBackgroundTasksDisabled || isForkSubagentEnabled()` gate).
///
/// Constructed at the model-facing schema seam
/// (`engine_prompt::build_language_model_tools`) once per turn from
/// runtime config + features; tools read the fields they care about.
#[derive(Debug, Clone, Default)]
pub struct SchemaContext {
    /// True when `COCO_BACKGROUND_TASKS_DISABLE` env truthy.
    /// AgentTool drops `run_in_background` from its schema when this is set.
    pub background_tasks_disabled: bool,
    /// True when fork-subagent mode is active for this session.
    /// AgentTool drops `run_in_background` when set — fork spawns
    /// always go through the bg path.
    pub fork_mode_active: bool,
    /// Snapshot of parent session features; tools that schema-gate
    /// on capability flags consult this. `None` when the seam can't
    /// resolve features (test / minimal SDK embedding).
    pub features: Option<std::sync::Arc<coco_types::Features>>,
    /// The active model's `apply_patch` tool shape, from
    /// `ModelInfo.apply_patch_tool_type` (`None` → the default `Freeform`). Read
    /// by `ApplyPatchTool::tool_spec`. `Freeform` is the only variant today, so
    /// this currently always yields the grammar tool — it is retained as the
    /// **extension point**: adding a future apply-type variant forces a new
    /// `tool_spec` match arm emitting a different wire shape (the exhaustive
    /// match guarantees it can't be silently defaulted).
    pub apply_patch_tool_type: Option<coco_types::ApplyPatchToolType>,
}

/// Info about whether a tool use is a search or read operation for UI collapse.
#[derive(Debug, Clone, Copy, Default)]
pub struct SearchReadInfo {
    /// True for search operations (grep, find, glob patterns).
    pub is_search: bool,
    /// True for read operations (cat, head, tail, file read).
    pub is_read: bool,
    /// True for directory-listing operations (ls, tree, du).
    pub is_list: bool,
}

/// How a tool behaves when the user interrupts (sends new input mid-execution).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterruptBehavior {
    /// Cancel the tool execution immediately.
    Cancel,
    /// Block: wait for tool to finish before processing new input.
    #[default]
    Block,
}

/// Options for generating tool descriptions.
#[derive(Debug, Clone, Default)]
pub struct DescriptionOptions {
    /// Whether this is a non-interactive (SDK/headless) session.
    pub is_non_interactive: bool,
    /// Names of all available tools (for cross-referencing in descriptions).
    pub tool_names: Vec<String>,
    /// Permission context for tailoring descriptions to the current mode.
    /// Tools may describe themselves differently based on what permissions are available.
    pub permission_context: Option<coco_types::ToolPermissionContext>,
}

/// Options for generating tool prompt text.
#[derive(Debug, Clone, Default)]
pub struct PromptOptions {
    /// Whether this is a non-interactive session.
    pub is_non_interactive: bool,
    /// Names of all available tools.
    pub tool_names: Vec<String>,
    /// Allowed agent types (if restricted).
    pub allowed_agent_types: Option<Vec<String>>,
    /// Names of model-invocable skills available this turn. Sorted
    /// for deterministic prompt text so tests and cache keys are
    /// stable. `SkillTool::prompt` consumes this to render the
    /// dynamic skill listing — `coco-rs` deliberately injects the
    /// list into the tool description rather than relying on system
    /// reminders, so every turn guarantees model visibility even if
    /// the reminder cadence skipped.
    pub skill_names: Vec<String>,
    /// Permission context for tailoring prompt to current mode.
    pub permission_context: Option<coco_types::ToolPermissionContext>,
    /// Full agent catalog snapshot. `AgentTool::prompt` consumes this
    /// to render the per-agent listing (`- {type}: {whenToUse} (Tools:
    /// ...)`) so the model sees the available subagent types and their
    /// tool surfaces. `None` ⇒ static fallback description (the
    /// pre-Round-7 behaviour).
    pub agent_catalog: Option<std::sync::Arc<coco_subagent::AgentCatalogSnapshot>>,
    /// Names of MCP servers ready (connected) this turn. The dynamic
    /// AgentTool prompt uses this to filter out agent definitions
    /// whose `required_mcp_servers` aren't all available — the model
    /// then never sees an agent it can't actually call.
    ///
    /// `None` ⇒ no MCP layer wired; the renderer's behaviour is to
    /// hide MCP-required agents (fail-closed). `Some(list)` filters
    /// against the named connected servers.
    pub ready_mcp_servers: Option<Vec<String>>,
    /// Coordinator-mode flag — when true, `AgentTool::prompt` renders
    /// the slim coordinator description (no usage notes, no parallel-
    /// spawn examples).
    pub coordinator_mode: bool,
    /// Fork-mode flag — when true, `AgentTool::prompt` adds the fork
    /// guidance section.
    pub fork_enabled: bool,
    /// Plan-mode interview-phase flag. When true, `EnterPlanModeTool::prompt`
    /// omits the `## What Happens in Plan Mode` section because the
    /// detailed iterative workflow already arrives via the plan-mode
    /// attachment. Source is `settings.plan_mode.workflow == Interview`
    /// only (no Growthbook / no env var; see `core/context/CLAUDE.md`).
    pub is_plan_interview_phase: bool,
    /// Host build embeds search tools (`bfs` / `ugrep`) inside the Bash
    /// tool. `AgentTool::prompt` swaps the "When NOT to use" section's
    /// FileRead/Glob/Grep hints for `find` / `grep` via Bash.
    pub has_embedded_search_tools: bool,
    /// Agent-team tools are available in this session. Task prompts use
    /// this to include teammate/owner coordination guidance only when
    /// `Feature::AgentTeams` is enabled.
    pub agent_teams_available: bool,
    /// Parent session is itself an in-process teammate. Drops the
    /// run_in_background / name / team_name / mode bullets and adds the
    /// "only synchronous subagents" notice in the AgentTool prompt.
    pub is_in_process_teammate: bool,
    /// Parent session is a (non in-process) teammate. Drops the name /
    /// team_name / mode bullets in the AgentTool prompt.
    pub is_teammate: bool,
    /// Inject the agent listing into a system-reminder attachment
    /// instead of inline in the tool description. Stabilises the
    /// tools-block prompt cache against MCP / plugin / permission
    /// changes (env `COCO_AGENT_LIST_IN_MESSAGES`).
    pub agent_list_via_attachment: bool,
    /// Pro subscriptions skip the inline "Launch multiple agents
    /// concurrently" usage-notes bullet because the same guidance is
    /// shown by the agent_listing_delta attachment for them.
    pub is_pro_subscription: bool,
    /// Host disabled background tasks via
    /// `COCO_BACKGROUND_TASKS_DISABLE`. Suppresses the run_in_background
    /// paragraphs in AgentTool's prompt.
    pub background_tasks_disabled: bool,
    /// Internal-build flag enabling the `isolation: "remote"` bullet.
    /// 3p builds keep this off because coco-rs ships only the local
    /// `worktree` isolation runtime. Only enabled on internal builds.
    pub ant_build: bool,
}

impl PromptOptions {
    /// Convert to DescriptionOptions for default prompt() implementation.
    pub fn as_description_options(&self) -> DescriptionOptions {
        DescriptionOptions {
            is_non_interactive: self.is_non_interactive,
            tool_names: self.tool_names.clone(),
            permission_context: self.permission_context.clone(),
        }
    }
}

/// MCP server tool metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub server_name: String,
    pub tool_name: String,
}

impl McpToolInfo {
    /// MCP-qualified tool name: `mcp__<server>__<tool>`.
    ///
    /// Canonical name registered in the `ToolRegistry` so that MCP tools
    /// cannot accidentally shadow built-in tools — a hostile or buggy
    /// MCP server advertising a tool named `Read` or `Bash` gets
    /// namespaced as `mcp__foo__Read` instead of overwriting the real
    /// one.
    ///
    /// Server and tool names are passed through unchanged. Sanitization
    /// (replacing `-`/`.`/` ` with `_`) is the caller's responsibility
    /// if the upstream MCP server uses characters that would break the
    /// delimiter; most servers already use snake_case.
    pub fn qualified_name(&self) -> String {
        use coco_types::MCP_TOOL_PREFIX;
        use coco_types::MCP_TOOL_SEPARATOR;
        format!(
            "{MCP_TOOL_PREFIX}{server}{MCP_TOOL_SEPARATOR}{tool}",
            server = self.server_name,
            tool = self.tool_name,
        )
    }
}

/// Progress update from a tool during execution.
///
/// Sent via `ctx.progress_tx` channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolProgress {
    pub tool_use_id: String,
    /// Optional parent tool use ID (for nested progress).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
    /// Progress payload (tool-specific data).
    pub data: Value,
}

/// Sender for tool progress updates.
pub type ProgressSender = tokio::sync::mpsc::UnboundedSender<ToolProgress>;

/// Receiver for tool progress updates.
pub type ProgressReceiver = tokio::sync::mpsc::UnboundedReceiver<ToolProgress>;

// =========================================================================
// `DynTool` — dyn-safe erased view used by registry / executor / hooks.
// =========================================================================
//
// Every implementation of [`Tool`] (the typed contract) automatically
// gets `DynTool` via the blanket `impl<T: Tool> DynTool for T` below,
// so tools don't write this trait by hand. The blanket handles the
// `serde_json::Value` ↔ `T::Input` / `T::Output` conversion at the
// boundary; tool bodies see only typed structs.
//
// Tools whose schema is dynamic (e.g. `McpTool` — the schema comes
// from the wire at runtime) use `type Input = Value; type Output =
// Value;` on the typed `Tool` trait and supply `runtime_validation_schema`
// from the wire. The blanket then degrades to a no-op round-trip at the
// boundary.
//
// ## Why two traits
//
// Rust can't have `dyn DynTool` with associated types, so we split
// the surface in two: typed (what tools implement) and erased
// (what the registry stores).

/// The dyn-safe erased view of [`Tool`]. Stored in `ToolRegistry` as
/// `Arc<dyn DynTool>` and consumed by every executor / hook / schema
/// path that needs heterogeneous tool dispatch.
///
/// **Do not implement this trait directly** — implement [`Tool`]
/// instead. The blanket `impl<T: Tool> DynTool for T` produces the
/// erased view automatically.
#[async_trait::async_trait]
pub trait DynTool: Send + Sync + 'static {
    // -- Identity --

    fn id(&self) -> ToolId;
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&str];
    fn search_hint(&self) -> Option<&str>;
    fn user_facing_name(&self) -> &str;

    // -- Schema --

    fn runtime_validation_schema(&self) -> &crate::schema::ToolInputSchema;
    async fn tool_spec(&self, schema_ctx: &SchemaContext, prompt_opts: &PromptOptions) -> ToolSpec;
    fn strict(&self) -> bool;

    // -- Description --

    fn description(&self, input: &Value, options: &DescriptionOptions) -> String;
    async fn prompt(&self, options: &PromptOptions) -> String;

    // -- Capability flags --

    fn is_enabled(&self, ctx: &ToolUseContext) -> bool;
    fn is_read_only(&self, input: &Value) -> bool;
    fn is_always_read_only(&self) -> bool;
    fn is_concurrency_safe(&self, input: &Value) -> bool;
    fn is_destructive(&self, input: &Value) -> bool;
    fn should_defer(&self) -> bool;
    fn always_load(&self) -> bool;
    fn is_lsp(&self) -> bool;
    fn interrupt_behavior(&self) -> InterruptBehavior;
    fn max_result_size_bound(&self) -> crate::tool_result_storage::ResultSizeBound;
    fn mcp_info(&self) -> Option<&McpToolInfo>;
    fn requires_user_interaction(&self) -> bool;
    fn is_open_world(&self, input: &Value) -> bool;
    fn is_mcp(&self) -> bool;
    fn is_search_or_read_command(&self, input: &Value) -> Option<SearchReadInfo>;
    fn get_tool_use_summary(&self, input: &Value) -> Option<String>;
    fn get_activity_description(&self, input: &Value) -> Option<String>;
    fn is_transparent_wrapper(&self) -> bool;
    fn extract_search_text(&self, output: &Value) -> Option<String>;
    fn is_result_truncated(&self, output: &Value) -> bool;

    // -- Validation --

    fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult;
    fn coerce_raw_string_input(&self, raw: &str) -> Option<Value>;
    fn inputs_equivalent(&self, a: &Value, b: &Value) -> bool;
    fn backfill_observable_input(&self, input: &mut Value);

    // -- Permissions --

    async fn check_permissions(&self, input: &Value, ctx: &ToolUseContext) -> ToolCheckResult;
    fn prepare_permission_matcher(&self, input: &Value) -> String;
    fn to_auto_classifier_input(&self, input: &Value) -> Option<String>;

    // -- Execution --

    async fn execute(
        &self,
        input: Value,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError>;
    fn get_path(&self, input: &Value) -> Option<String>;

    // -- Rendering --

    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart>;
}

// =========================================================================
// `ToolSpec` — the model-facing wire shape (single source of truth).
// =========================================================================

/// How a tool is presented to the model in the request tool list. The single
/// source of truth, returned by [`Tool::tool_spec`] and converted to the
/// provider wire format once in `app/query::engine_prompt`. Mirrors codex-rs
/// `ToolSpec`. Deliberately *exhaustive*: the wire-conversion boundary MUST
/// handle every shape, so adding a future variant (provider built-ins, MCP)
/// is a compile error there until serialized — never a silent fallback.
#[derive(Debug, Clone)]
pub enum ToolSpec {
    /// A JSON function tool (`{name, description, parameters}`). The default
    /// for every tool — the model calls it with a JSON object argument.
    Function(FunctionToolSpec),
    /// A freeform/grammar tool (OpenAI Responses custom tool): the model emits
    /// raw text constrained by `format`, not a JSON object. Realized as a
    /// provider-defined custom tool at the wire edge. OpenAI-Responses-only.
    Freeform(FreeformToolSpec),
}

/// A JSON function tool definition.
#[derive(Debug, Clone)]
pub struct FunctionToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub strict: bool,
}

/// A freeform/grammar tool definition.
#[derive(Debug, Clone)]
pub struct FreeformToolSpec {
    pub name: String,
    pub description: String,
    pub format: GrammarFormat,
}

/// The grammar constraint carried by a [`FreeformToolSpec`].
#[derive(Debug, Clone)]
pub struct GrammarFormat {
    /// Grammar syntax, e.g. `"lark"`.
    pub syntax: String,
    /// The grammar definition text.
    pub definition: String,
}

impl ToolSpec {
    /// The model-facing description, regardless of variant.
    pub fn description(&self) -> &str {
        match self {
            ToolSpec::Function(f) => &f.description,
            ToolSpec::Freeform(f) => &f.description,
        }
    }
}

// =========================================================================
// `Tool` — the typed contract every built-in implements.
// =========================================================================
//
// Associated types `Input` / `Output` replace structural generics.
// Method bodies see typed structs instead of `serde_json::Value`;
// field renames are caught at `cargo check`, the schema is auto-derived
// from `Self::Input` via the `JsonSchema` impl, and
// `render_for_model(&Self::Output)` stops digging fields out of a `Value`.
//
// Adding a new tool:
//
// ```ignore
// #[derive(Deserialize, JsonSchema)]
// pub struct MyInput {
//     /// Doc comments become the schema's `description` for the field.
//     pub pattern: String,
//     #[serde(default)]
//     pub limit: Option<i32>,
// }
//
// #[derive(Serialize, Deserialize)]
// pub struct MyOutput { ... }
//
// #[async_trait]
// impl Tool for MyTool {
//     type Input  = MyInput;
//     type Output = MyOutput;
//     fn id(&self) -> ToolId { ... }
//     fn name(&self) -> &str { ... }
//     async fn execute(&self, input: MyInput, ctx: &ToolUseContext)
//         -> Result<ToolResult<MyOutput>, ToolError> { ... }
//     fn render_for_model(&self, out: &MyOutput) -> Vec<ToolResultContentPart> { ... }
// }
// ```
//
// `DynTool` comes free via the blanket impl below.
#[async_trait::async_trait]
pub trait Tool: Send + Sync + 'static {
    /// Typed input — deserialised once at the executor boundary.
    /// Renaming a field is a compile-error; the model-visible schema
    /// (derived from `JsonSchema`) is the same artifact as the
    /// parser (driven by `Deserialize`), eliminating drift.
    ///
    /// Tools whose schema is dynamic (e.g. MCP) set this to `Value`
    /// and override [`Tool::runtime_validation_schema`] manually.
    type Input: for<'de> Deserialize<'de> + JsonSchema + Send + Sync + 'static;
    /// Typed output — `render_for_model(&Self::Output)` reads fields
    /// directly.
    ///
    /// Tools without a structured output (free-form text) set this to
    /// `String`; tools with rich shapes use `#[serde(tag = "...")]`
    /// tagged enums (the `AgentSpawnRenderResult` pattern).
    type Output: Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static;

    // -- Identity --

    /// Tool identity (ToolId::Builtin or Mcp or Custom).
    fn id(&self) -> ToolId;

    /// User-facing name (e.g., "Read", "Bash", "WebFetch").
    fn name(&self) -> &str;

    /// Alternative names for tool search.
    fn aliases(&self) -> &[&str] {
        &[]
    }

    /// Short search hint for ToolSearch deferred discovery (3-10 words).
    fn search_hint(&self) -> Option<&str> {
        None
    }

    /// User-facing display name (defaults to name()).
    fn user_facing_name(&self) -> &str {
        self.name()
    }

    // -- Schema --

    /// **Runtime validation schema** (v4.2 owner: `coco-tool-runtime`). Static,
    /// owns the compiled validator; validated on every tool call (including
    /// hook-rewritten input at `tool_call_preparer.rs`). No default ⇒ E0046
    /// forces every tool to declare it (a `Value`-Input tool can't fall through
    /// to a derive). MUST be a superset of every `tool_spec(ctx)` parameters
    /// view; internally-built schemas carry `additionalProperties:false`.
    fn runtime_validation_schema(&self) -> &crate::schema::ToolInputSchema;

    /// **The model-facing wire shape** — the single source of truth for how
    /// this tool appears in the request tool list. `app/query::engine_prompt`
    /// calls only this (not `prompt()` / a separate schema hook) and converts
    /// the [`ToolSpec`] to the provider wire format. Mirrors codex-rs
    /// `ToolExecutor::spec`.
    ///
    /// Default: a JSON [`ToolSpec::Function`] built from `name()`, the
    /// model-facing `prompt()` description, the runtime validation schema, and
    /// `strict()`. Override to present a different shape:
    /// - a [`ToolSpec::Freeform`] grammar tool (apply_patch), or
    /// - a `Function` with runtime-only hook-injected fields omitted — AgentTool
    ///   `mcp_servers`, Bash `_simulatedSedEdit`, ExitPlanMode `plan` — via
    ///   [`crate::schema::schema_omit_properties`] on the runtime schema value.
    async fn tool_spec(
        &self,
        _schema_ctx: &SchemaContext,
        prompt_opts: &PromptOptions,
    ) -> ToolSpec {
        ToolSpec::Function(FunctionToolSpec {
            name: self.name().to_string(),
            description: self.prompt(prompt_opts).await,
            parameters: self.runtime_validation_schema().as_value().clone(),
            strict: self.strict(),
        })
    }

    /// Coerce a raw-string tool-call input into the typed JSON this tool's
    /// schema expects. Freeform/custom tool calls (e.g. apply_patch) deliver
    /// the model's output as a bare string, not a JSON object; the tool
    /// declares how to wrap it (apply_patch → `{"patch": raw}`) so schema
    /// validation and `Self::Input` deserialization succeed. Default: `None`
    /// (normal tools always receive a JSON object). Invoked before schema
    /// validation only when the raw input is a `Value::String`.
    fn coerce_raw_string_input(&self, _raw: &str) -> Option<Value> {
        None
    }

    /// Whether to enforce strict schema validation.
    fn strict(&self) -> bool {
        false
    }

    // -- Description --

    /// Dynamic description that may vary based on input and context.
    ///
    /// Called at tool-call render time when input is fully streamed.
    /// For schema-listing time (no input yet), use [`Tool::prompt`].
    fn description(&self, input: &Self::Input, options: &DescriptionOptions) -> String;

    /// Model-facing tool description — the LLM-visible text in the tool
    /// definition's `description` field. Assembled at schema-listing time
    /// (before any input exists), so it is input-independent and must stay
    /// session-stable for prompt caching.
    ///
    /// REQUIRED (no default, like [`Tool::runtime_validation_schema`]):
    /// `app/query::engine_prompt` sources the wire `description` from this
    /// method, NOT from [`Tool::description`] (the per-call UI label). An
    /// empty default would silently ship a tool the model knows nothing
    /// about, so the compiler (E0046) forces every tool to declare it.
    ///
    async fn prompt(&self, options: &PromptOptions) -> String;

    // -- Capability Flags --

    /// Whether this tool is enabled in the given context.
    ///
    /// Default returns `true` — most tools are always available. Override
    /// to gate the tool on `ctx.features.enabled(Feature::X)` (token-economy
    /// or experimental gates), an OS check, or a runtime resource probe.
    /// See `docs/coco-rs/feature-gates-and-tool-filtering.md` for the
    /// design and the multi-layer filter pipeline this hook is the
    /// first layer of.
    fn is_enabled(&self, _ctx: &crate::context::ToolUseContext) -> bool {
        true
    }

    /// Whether this tool only reads (no side effects).
    fn is_read_only(&self, _input: &Self::Input) -> bool {
        false
    }

    /// Whether this tool is **statically** read-only — known to be safe
    /// without inspecting input. A coarse, input-independent companion to
    /// [`Tool::is_read_only`].
    ///
    /// **Not a security boundary, and no longer a schema filter.** Plan
    /// mode does NOT narrow the model's tool schema; plan-mode
    /// read-only is enforced at *call time* by `coco_permissions` (see
    /// `core/permissions/src/evaluate.rs`), which keys off its own
    /// tool-name allow-list, not this method. Retained as a tool-level
    /// property for diagnostics / potential future reuse; it currently has
    /// no registry consumer.
    ///
    /// **Contract**: the answer must not depend on input.
    ///
    /// Default: synthesize an `Input` from `Value::Null` and delegate to
    /// [`Tool::is_read_only`]. Tools whose `Self::Input` is `Value` or has
    /// only optional fields inherit their constant `is_read_only`; tools
    /// whose typed `Self::Input` requires fields fall through to `false`.
    fn is_always_read_only(&self) -> bool {
        serde_json::from_value::<Self::Input>(Value::Null)
            .ok()
            .map(|input| Tool::is_read_only(self, &input))
            .unwrap_or(false)
    }

    /// Whether multiple instances can safely run concurrently.
    /// Critical for batch partitioning in ToolExecutor.
    ///
    /// **Invariant**: tools returning `true` MUST NOT mutate
    /// `ctx.app_state` during `execute`. Concurrent tools share a
    /// single `Arc<RwLock<ToolAppState>>`; live writes would race
    /// with sibling reads. Concurrent tools run against a shared
    /// `currentContext` snapshot and queue `setAppState` calls to
    /// apply after the batch. Rust relies on convention (concurrent
    /// tools are read-only — Read/Glob/Grep/LSP/etc.) instead of
    /// implementing the queue; this comment is the contract. Serial
    /// unsafe tools (`is_concurrency_safe == false`) are the only
    /// code path that writes `ctx.app_state`.
    fn is_concurrency_safe(&self, _input: &Self::Input) -> bool {
        false
    }

    /// Whether this tool performs destructive operations.
    fn is_destructive(&self, _input: &Self::Input) -> bool {
        false
    }

    /// Whether this tool should be deferred (lazy-loaded, discovered via ToolSearch).
    fn should_defer(&self) -> bool {
        false
    }

    /// Whether this tool should always be loaded (even when deferred).
    fn always_load(&self) -> bool {
        false
    }

    /// Whether this is an LSP tool.
    fn is_lsp(&self) -> bool {
        false
    }

    /// How this tool behaves when the user interrupts.
    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    /// Per-tool tool-result size cap (in characters). When a tool's
    /// result exceeds this size, the runtime persists it to disk
    /// (via `coco_tool_runtime::tool_result_storage::persist_to_disk`)
    /// and substitutes a `<persisted-output>` reference message into
    /// the conversation.
    ///
    /// `i64::MAX` opts the tool out of persistence — its results stay
    /// inline regardless of length. Tools whose
    /// output is canonical (e.g. `Read` on a tracked file the model
    /// will read again) opt out so persistence isn't circular.
    ///
    /// Default `100_000`, clamped by `DEFAULT_MAX_RESULT_SIZE_CHARS = 50_000`.
    /// Override per-tool to declare opt-out ([`ResultSizeBound::Unbounded`])
    /// or a tighter cap ([`ResultSizeBound::Chars`]).
    fn max_result_size_bound(&self) -> crate::tool_result_storage::ResultSizeBound {
        crate::tool_result_storage::DEFAULT_TOOL_MAX_RESULT_SIZE_BOUND
    }

    /// MCP server/tool info (for MCP-wrapped tools).
    fn mcp_info(&self) -> Option<&McpToolInfo> {
        None
    }

    /// Whether this tool requires user interaction to complete.
    ///
    /// When false, permission prompts are auto-denied for headless/background agents.
    /// Used by ExitPlanMode (returns false for teammates so they send approval
    /// via mailbox instead of requiring a local permission dialog).
    fn requires_user_interaction(&self) -> bool {
        true
    }

    /// Whether this tool exhibits "open-world" behavior — i.e. its effect
    /// depends on external state not under our control (environment,
    /// network, external services, arbitrary user input). Used as a
    /// metadata hint for UI rendering and telemetry; does NOT gate
    /// permissions or execution.
    ///
    /// Default is `false` — tools are closed-world unless they opt in.
    /// Dynamic MCP wrappers (`core/tools/src/tools/mcp_tools.rs`) can
    /// override this to forward the annotation from the MCP server.
    fn is_open_world(&self, _input: &Self::Input) -> bool {
        false
    }

    /// Whether this tool is sourced from an MCP (Model Context Protocol)
    /// server rather than being a native built-in.
    ///
    /// Default derives from `mcp_info()`: any tool that advertises
    /// `McpToolInfo` is an MCP tool.
    fn is_mcp(&self) -> bool {
        self.mcp_info().is_some()
    }

    /// Returns information about whether this tool use is a search or read
    /// operation that should be collapsed into a condensed display in the UI.
    ///
    fn is_search_or_read_command(&self, _input: &Self::Input) -> Option<SearchReadInfo> {
        None
    }

    /// Returns a short string summary of this tool use for compact views.
    ///
    fn get_tool_use_summary(&self, _input: &Self::Input) -> Option<String> {
        None
    }

    /// Returns a human-readable present-tense activity description for spinner
    /// display (e.g., "Reading src/foo.ts", "Running bun test").
    ///
    fn get_activity_description(&self, _input: &Self::Input) -> Option<String> {
        None
    }

    /// Whether this tool is a transparent wrapper that delegates all rendering
    /// to its progress handler. The wrapper itself shows nothing in the UI.
    ///
    fn is_transparent_wrapper(&self) -> bool {
        false
    }

    /// Returns flattened text of what the tool result shows, for transcript
    /// search indexing.
    ///
    fn extract_search_text(&self, _output: &Self::Output) -> Option<String> {
        None
    }

    /// Returns true when the non-verbose rendering of this output is truncated
    /// (i.e., expanding would reveal more content).
    ///
    fn is_result_truncated(&self, _output: &Self::Output) -> bool {
        false
    }

    // -- Validation --

    /// Validate input before execution. Called before check_permissions.
    ///
    /// Schema-level validation (required fields, types) already passed
    /// before this method runs — the `Self::Input` you receive is the
    /// successfully-deserialised value. Use this hook for **semantic**
    /// validation: cross-field constraints, stateful checks like
    /// read-before-write enforcement, runtime feature gating.
    ///
    fn validate_input(&self, _input: &Self::Input, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::Valid
    }

    /// Check if two inputs are equivalent (for idempotency detection).
    fn inputs_equivalent(&self, _a: &Self::Input, _b: &Self::Input) -> bool {
        false
    }

    /// Backfill observable input fields for hooks/logging.
    ///
    /// Normalizes input before hooks see it (e.g., adds default field
    /// values, expands aliases). Called on a shallow clone; the
    /// original input is unchanged.
    ///
    /// **Stays `Value`-typed deliberately** — it operates on the wire
    /// shape (adding legacy field aliases that the typed struct may
    /// not even know about). Tools that need typed access should
    /// `serde_json::from_value` inside this method.
    fn backfill_observable_input(&self, _input: &mut Value) {}

    // -- Permissions --

    /// Tool's own opinion at the central evaluator's step-1c slot.
    ///
    /// Tools that need content-specific safety checks (Read/Grep/Glob
    /// path safety, Bash subcommand parsing, Write path validation)
    /// override this to return `Deny`/`Ask` for unsafe inputs and
    /// `Passthrough` otherwise. The default `Passthrough` defers
    /// entirely to the rule pipeline; tools without a `check_permissions`
    /// override behave as `() => Passthrough`.
    ///
    /// The result is consumed by
    /// `coco_permissions::PermissionEvaluator::evaluate_with_tool_check`
    /// inside `app/query::tool_call_preparer::resolve_permission_decision`.
    /// Returning `Allow { updated_input }` here propagates the
    /// normalized input onto the resulting `PermissionDecision::Allow`.
    async fn check_permissions(
        &self,
        _input: &Self::Input,
        _ctx: &ToolUseContext,
    ) -> ToolCheckResult {
        ToolCheckResult::Passthrough
    }

    /// Prepare a permission matcher string for hook matching.
    fn prepare_permission_matcher(&self, _input: &Self::Input) -> String {
        self.name().to_string()
    }

    /// Project this tool's input down to the security-relevant fields the
    /// auto-mode classifier should see.
    ///
    /// `None` means "no classifier-relevant input" — the classifier then
    /// falls back to the raw input JSON. Security-relevant tools (anything
    /// that mutates the filesystem, executes code, or reaches the network)
    /// should override this with a curated `Some(...)` so the gate judges a
    /// minimal, well-formed projection rather than an over-broad blob.
    fn to_auto_classifier_input(&self, _input: &Self::Input) -> Option<String> {
        None
    }

    // -- Execution --

    /// Execute the tool.
    ///
    /// Progress reporting: send updates via `ctx.progress_tx` if available.
    /// ```ignore
    /// if let Some(tx) = &ctx.progress_tx {
    ///     let _ = tx.send(ToolProgress {
    ///         tool_use_id: ctx.tool_use_id.clone().unwrap_or_default(),
    ///         parent_tool_use_id: None,
    ///         data: serde_json::json!({"status": "running", "pct": 50}),
    ///     });
    /// }
    /// ```
    async fn execute(
        &self,
        input: Self::Input,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Self::Output>, ToolError>;

    // -- File Path --

    /// Get the file path associated with this tool call (for file-based tools).
    fn get_path(&self, _input: &Self::Input) -> Option<String> {
        None
    }

    // -- Result Mapping --

    /// Render the tool's structured output as a sequence of content
    /// parts (text + images + documents) that the model sees in the
    /// `tool_result` block.
    ///
    /// The Rust signature drops `tool_use_id` because the executor wraps the
    /// parts at message-creation time, not the tool.
    /// **The argument is `&Self::Output` (typed)** —
    /// no more `data.get("xxx").and_then(...)` field-mining at the call site.
    ///
    /// # Default behaviour
    ///
    /// The default impl emits a single [`ToolResultContentPart::Text`]
    /// with `serde_json::to_string(&data)` — the right thing for
    /// structured outputs (the model gets to see the JSON). Tools
    /// with free-form text output (`type Output = String`) override
    /// to emit the bare string without the surrounding quotes.
    ///
    /// # Path 1 only
    ///
    /// This method is **only** for tool results — i.e. content blocks
    /// that pair with a real `tool_use_id` from the assistant's prior
    /// `tool_use`. Synthesizing a `tool_result` without a paired
    /// `tool_use_id` is rejected by every major provider (Anthropic
    /// 400, OpenAI "Invalid parameter…", Gemini schema mismatch).
    /// Slash command output, system reminders, and attachments use
    /// their own user-message-text paths and do NOT go through here.
    ///
    /// # Provider degradation
    ///
    /// Multi-block parts (image / document) flow through to providers
    /// that support them (Anthropic, Gemini 3+). Text-only providers
    /// (OpenAI Chat, OpenAI-Compatible: DeepSeek/xAI/Groq) replace
    /// non-Text parts with a visible marker at the provider boundary
    /// — see `vercel-ai-openai/src/messages/...` for the conversion.
    ///
    /// # Purity
    ///
    /// Implementations must be pure (no IO, no async, no global
    /// state). If a tool needs async work to format its output, do
    /// the work in [`Tool::execute`] and stash the rendered form in
    /// `data`.
    fn render_for_model(&self, data: &Self::Output) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: serde_json::to_string(data).unwrap_or_default(),
            provider_options: None,
        }]
    }
}

// =========================================================================
// Blanket: every `Tool` is automatically a `DynTool`.
// =========================================================================
//
// At the boundary between erased and typed:
// - input  Value → T::Input  via `serde_json::from_value`
// - output T::Output → Value via `serde_json::to_value`
//
// Tools whose Input/Output is already `Value` (e.g. McpTool) round-trip
// at zero structural cost (just clones).
#[async_trait::async_trait]
impl<T: Tool> DynTool for T {
    fn id(&self) -> ToolId {
        Tool::id(self)
    }
    fn name(&self) -> &str {
        Tool::name(self)
    }
    fn aliases(&self) -> &[&str] {
        Tool::aliases(self)
    }
    fn search_hint(&self) -> Option<&str> {
        Tool::search_hint(self)
    }
    fn user_facing_name(&self) -> &str {
        Tool::user_facing_name(self)
    }

    fn runtime_validation_schema(&self) -> &crate::schema::ToolInputSchema {
        Tool::runtime_validation_schema(self)
    }
    async fn tool_spec(&self, schema_ctx: &SchemaContext, prompt_opts: &PromptOptions) -> ToolSpec {
        Tool::tool_spec(self, schema_ctx, prompt_opts).await
    }
    fn strict(&self) -> bool {
        Tool::strict(self)
    }

    fn description(&self, input: &Value, options: &DescriptionOptions) -> String {
        // At schema-listing time the caller passes Value::Null and the
        // typed parse fails for any non-Default Input. That path is
        // expected to use `prompt()` instead — but to stay tolerant we
        // return an empty string rather than panicking.
        match serde_json::from_value::<T::Input>(input.clone()) {
            Ok(typed) => Tool::description(self, &typed, options),
            Err(_) => String::new(),
        }
    }
    async fn prompt(&self, options: &PromptOptions) -> String {
        Tool::prompt(self, options).await
    }

    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        Tool::is_enabled(self, ctx)
    }
    fn is_read_only(&self, input: &Value) -> bool {
        // When the typed `Self::Input` can't be synthesised from the
        // raw Value (typically because required fields are absent —
        // tests passing `Value::Null`, partial streamed input), fall
        // back to the static `is_always_read_only()` answer. This
        // preserves the pre-typed convention where tools whose
        // `is_read_only` ignored input returned a constant.
        //
        // For input-conditional tools (e.g. Bash), `is_always_read_only`
        // returns the default `false`, matching the conservative
        // "unknown → not safe" fallback.
        serde_json::from_value::<T::Input>(input.clone())
            .map(|t| Tool::is_read_only(self, &t))
            .unwrap_or_else(|_| Tool::is_always_read_only(self))
    }
    fn is_always_read_only(&self) -> bool {
        Tool::is_always_read_only(self)
    }
    fn is_concurrency_safe(&self, input: &Value) -> bool {
        serde_json::from_value::<T::Input>(input.clone())
            .map(|t| Tool::is_concurrency_safe(self, &t))
            .unwrap_or(false)
    }
    fn is_destructive(&self, input: &Value) -> bool {
        serde_json::from_value::<T::Input>(input.clone())
            .map(|t| Tool::is_destructive(self, &t))
            .unwrap_or(false)
    }
    fn should_defer(&self) -> bool {
        Tool::should_defer(self)
    }
    fn always_load(&self) -> bool {
        Tool::always_load(self)
    }
    fn is_lsp(&self) -> bool {
        Tool::is_lsp(self)
    }
    fn interrupt_behavior(&self) -> InterruptBehavior {
        Tool::interrupt_behavior(self)
    }
    fn max_result_size_bound(&self) -> crate::tool_result_storage::ResultSizeBound {
        Tool::max_result_size_bound(self)
    }
    fn mcp_info(&self) -> Option<&McpToolInfo> {
        Tool::mcp_info(self)
    }
    fn requires_user_interaction(&self) -> bool {
        Tool::requires_user_interaction(self)
    }
    fn is_open_world(&self, input: &Value) -> bool {
        serde_json::from_value::<T::Input>(input.clone())
            .map(|t| Tool::is_open_world(self, &t))
            .unwrap_or(false)
    }
    fn is_mcp(&self) -> bool {
        Tool::is_mcp(self)
    }
    fn is_search_or_read_command(&self, input: &Value) -> Option<SearchReadInfo> {
        serde_json::from_value::<T::Input>(input.clone())
            .ok()
            .and_then(|t| Tool::is_search_or_read_command(self, &t))
    }
    fn get_tool_use_summary(&self, input: &Value) -> Option<String> {
        serde_json::from_value::<T::Input>(input.clone())
            .ok()
            .and_then(|t| Tool::get_tool_use_summary(self, &t))
    }
    fn get_activity_description(&self, input: &Value) -> Option<String> {
        serde_json::from_value::<T::Input>(input.clone())
            .ok()
            .and_then(|t| Tool::get_activity_description(self, &t))
    }
    fn is_transparent_wrapper(&self) -> bool {
        Tool::is_transparent_wrapper(self)
    }
    fn extract_search_text(&self, output: &Value) -> Option<String> {
        serde_json::from_value::<T::Output>(output.clone())
            .ok()
            .and_then(|o| Tool::extract_search_text(self, &o))
    }
    fn is_result_truncated(&self, output: &Value) -> bool {
        serde_json::from_value::<T::Output>(output.clone())
            .map(|o| Tool::is_result_truncated(self, &o))
            .unwrap_or(false)
    }

    fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        match serde_json::from_value::<T::Input>(input.clone()) {
            Ok(typed) => Tool::validate_input(self, &typed, ctx),
            Err(e) => ValidationResult::invalid(format!("input does not match schema: {e}")),
        }
    }
    fn coerce_raw_string_input(&self, raw: &str) -> Option<Value> {
        Tool::coerce_raw_string_input(self, raw)
    }
    fn inputs_equivalent(&self, a: &Value, b: &Value) -> bool {
        match (
            serde_json::from_value::<T::Input>(a.clone()),
            serde_json::from_value::<T::Input>(b.clone()),
        ) {
            (Ok(ta), Ok(tb)) => Tool::inputs_equivalent(self, &ta, &tb),
            _ => false,
        }
    }
    fn backfill_observable_input(&self, input: &mut Value) {
        Tool::backfill_observable_input(self, input)
    }

    async fn check_permissions(&self, input: &Value, ctx: &ToolUseContext) -> ToolCheckResult {
        match serde_json::from_value::<T::Input>(input.clone()) {
            Ok(typed) => Tool::check_permissions(self, &typed, ctx).await,
            // Unreachable in the production pipeline: every caller holds a
            // `ValidatedInput`, so the typed deserialization must succeed.
            // Reaching this branch means an unvalidated input slipped past
            // the seam — fail CLOSED (Ask), never silently `Passthrough`:
            // a tool's path-based carve-outs (e.g. plan-file auto-allow) and
            // deny opinions are security-relevant and were skipped.
            Err(e) => {
                tracing::error!(
                    tool = self.name(),
                    error = %e,
                    "check_permissions: input failed typed deserialization after \
                     validation — failing closed to Ask"
                );
                ToolCheckResult::Ask {
                    message: format!(
                        "{} input could not be interpreted for permission checks; \
                         manual review required.",
                        self.name()
                    ),
                    suggestions: vec![],
                    choices: None,
                    detail: None,
                }
            }
        }
    }
    fn prepare_permission_matcher(&self, input: &Value) -> String {
        serde_json::from_value::<T::Input>(input.clone())
            .map(|t| Tool::prepare_permission_matcher(self, &t))
            .unwrap_or_else(|_| self.name().to_string())
    }
    fn to_auto_classifier_input(&self, input: &Value) -> Option<String> {
        match serde_json::from_value::<T::Input>(input.clone()) {
            Ok(typed) => Tool::to_auto_classifier_input(self, &typed),
            // Deserialization failure must NOT become `None`: `None` reads as
            // "no security relevance" downstream. A malformed input is exactly
            // what the gate must still inspect, so fall back to the raw JSON
            // (fail toward classification) — fall back to the raw JSON.
            Err(_) => Some(serde_json::to_string(input).unwrap_or_default()),
        }
    }

    async fn execute(
        &self,
        input: Value,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let typed: T::Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput {
                message: format!("invalid tool input: {e}"),
                error_code: None,
            })?;
        let r = Tool::execute(self, typed, ctx).await?;
        Ok(ToolResult {
            data: serde_json::to_value(&r.data).unwrap_or(Value::Null),
            new_messages: r.new_messages,
            app_state_patch: r.app_state_patch,
            permission_updates: r.permission_updates,
            // Preserve the typed tool's SUCCESS display data (apply-patch
            // preview, AskUserQuestion answers) so the styled transcript cell
            // gets it. Errors carry their own display data via `ToolError`.
            display_data: r.display_data,
        })
    }
    fn get_path(&self, input: &Value) -> Option<String> {
        serde_json::from_value::<T::Input>(input.clone())
            .ok()
            .and_then(|t| Tool::get_path(self, &t))
    }

    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        // `data` was produced by `DynTool::execute` (above) via
        // `to_value(&T::Output)`. Round-tripping it back should always
        // succeed; if it doesn't, something has rewritten the Value
        // shape (e.g. transcript replay across schema changes) — fall
        // back to a JSON dump rather than panicking.
        match serde_json::from_value::<T::Output>(data.clone()) {
            Ok(typed) => Tool::render_for_model(self, &typed),
            Err(_) => vec![ToolResultContentPart::Text {
                text: serde_json::to_string(data).unwrap_or_default(),
                provider_options: None,
            }],
        }
    }
}

/// Helper for the common `render_for_model` pattern: emit a single
/// [`ToolResultContentPart::Text`] containing either the bare string
/// payload (when `data` is `Value::String`) or the JSON-stringified
/// `data` for any other shape.
///
/// Tools whose `mapToolResultToToolResultBlockParam` returns plain text
/// use this so the model sees the underlying message without a `"…"`
/// JSON-quote wrapper.
///
/// Most typed-output tools won't need this; it stays available for
/// `Output = Value` cases (MCP, dynamic schema) and migrations in
/// progress.
pub fn render_text_or_json(data: &Value) -> Vec<ToolResultContentPart> {
    let text = data
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| serde_json::to_string(data).unwrap_or_default());
    vec![ToolResultContentPart::Text {
        text,
        provider_options: None,
    }]
}

#[cfg(test)]
#[path = "traits.test.rs"]
mod tests;
