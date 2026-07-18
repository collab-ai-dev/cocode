use coco_types::ToolId;
use coco_types::ToolName;
use coco_types::WireToolName;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::RwLock;

use crate::context::ToolUseContext;
use crate::traits::DynTool;

/// Run the schema-time filter pipeline against one tool.
///
/// 1. `Tool::is_enabled(ctx)`     — Feature gate / OS / hard deps
/// 2. `ToolOverrides::permits`    — what the active model accepts
/// 3. `ToolFilter::allows`        — agent allow/deny lists
///
/// **No `PermissionMode` layer.** Plan mode does NOT narrow
/// the model's tool schema — the full tool set is exposed in every mode.
/// Plan-mode read-only is enforced at *call time* by `coco_permissions`
/// (read-only / session-plan-file write → allow; any other write → ask,
/// deny when non-interactive) plus the `<system-reminder>` plan
/// attachment. Filtering the schema here would strip the very tools the
/// model needs to make progress in plan mode (ExitPlanMode,
/// AskUserQuestion, writing the plan file) and double-enforce a policy
/// the permission layer already owns. See `core/permissions/src/evaluate.rs`
/// and `docs/internal/feature-gates-and-tool-filtering.md` §7/§9.
///
/// MCP server reachability is likewise not checked here; MCP tools whose
/// backing server disconnects are removed from the registry via
/// `ToolRegistry::deregister_by_server`, so they never reach this pipeline.
fn passes_filter_pipeline(tool: &dyn DynTool, ctx: &ToolUseContext) -> bool {
    let id = tool.id();
    tool.is_enabled(ctx)
        && (!ctx.is_coordinator_lead() || coco_subagent::coordinator_allows_tool_name(tool.name()))
        && ctx.tool_overrides.permits(&id)
        && ctx.tool_filter.allows(&id)
}

/// Inner state protected by a single RwLock.
///
/// Both maps are always mutated together (every `register` touches
/// `tools` and may also touch `aliases`; every `deregister_by_server`
/// touches both). A single lock ensures the two maps are always
/// consistent — no window where `tools` has a new entry but `aliases`
/// does not, or vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToolRegistrationId(i64);

#[derive(Clone)]
struct RegisteredTool {
    registration_id: ToolRegistrationId,
    tool: Arc<dyn DynTool>,
}

impl RegisteredTool {
    fn new(registration_id: ToolRegistrationId, tool: Arc<dyn DynTool>) -> Self {
        Self {
            registration_id,
            tool,
        }
    }
}

#[derive(Clone, Default)]
struct RegistryInner {
    /// Primary lookup: canonical name → tool.
    tools: HashMap<String, RegisteredTool>,
    /// Alias lookup: alias → canonical name.
    aliases: HashMap<String, String>,
    /// Provider-facing wire name → canonical name. Registration owns this
    /// index and rejects collisions before publishing a tool.
    wire_names: HashMap<WireToolName, String>,
    next_registration_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolRegistrationError {
    #[error("MCP {component} component must not be empty")]
    EmptyMcpIdentityComponent { component: &'static str },
    #[error("MCP metadata identifies {expected}, but the tool id is {tool_id}")]
    InvalidMcpIdentity { tool_id: ToolId, expected: ToolId },
    #[error(
        "server replacement for {expected_server} contains tool {tool_id} owned by {actual_server:?}"
    )]
    ServerOwnershipMismatch {
        expected_server: String,
        actual_server: Option<String>,
        tool_id: ToolId,
    },
    #[error("tool canonical name already registered: {canonical_name}")]
    CanonicalCollision { canonical_name: String },
    #[error("tool wire name {wire_name} collides between {existing} and {incoming}")]
    WireNameCollision {
        wire_name: WireToolName,
        existing: String,
        incoming: String,
    },
    #[error("tool alias {alias} collides between {existing} and {incoming}")]
    AliasCollision {
        alias: String,
        existing: String,
        incoming: String,
    },
}

impl RegistryInner {
    fn next_registration_id(&mut self) -> ToolRegistrationId {
        self.next_registration_id += 1;
        ToolRegistrationId(self.next_registration_id)
    }

    /// Insert `tool` under its canonical name + aliases, replicating the
    /// **MCP-namespace promotion** (a hostile MCP `Read` is stored as
    /// `mcp__srv__Read`, never shadowing the built-in). Shared by
    /// [`ToolRegistry::register`] and [`ToolRegistry::replace_server_tools`]
    /// so the namespacing is identical on both paths.
    fn register_with_aliases(
        &mut self,
        tool: Arc<dyn DynTool>,
    ) -> Result<ToolRegistrationId, ToolRegistrationError> {
        let tool_id = tool.id();
        if let Some(info) = tool.mcp_info() {
            if info.server_name.is_empty() {
                return Err(ToolRegistrationError::EmptyMcpIdentityComponent {
                    component: "server",
                });
            }
            if info.tool_name.is_empty() {
                return Err(ToolRegistrationError::EmptyMcpIdentityComponent { component: "tool" });
            }
            let expected = ToolId::Mcp {
                server: info.server_name.clone(),
                tool: info.tool_name.clone(),
            };
            if tool_id != expected {
                return Err(ToolRegistrationError::InvalidMcpIdentity { tool_id, expected });
            }
        }
        let canonical = tool_id.to_string();
        if self.tools.contains_key(&canonical) {
            return Err(ToolRegistrationError::CanonicalCollision {
                canonical_name: canonical,
            });
        }
        let wire_name = WireToolName::for_tool_id(&tool_id);
        if let Some(existing) = self.wire_names.get(&wire_name)
            && existing != &canonical
        {
            return Err(ToolRegistrationError::WireNameCollision {
                wire_name,
                existing: existing.clone(),
                incoming: canonical,
            });
        }
        for alias in tool.aliases() {
            let alias = *alias;
            let existing = self
                .tools
                .contains_key(alias)
                .then(|| alias.to_string())
                .or_else(|| self.aliases.get(alias).cloned());
            if let Some(existing) = existing.filter(|existing| existing != &canonical) {
                return Err(ToolRegistrationError::AliasCollision {
                    alias: alias.to_string(),
                    existing,
                    incoming: canonical,
                });
            }
        }
        for alias in tool.aliases() {
            self.aliases.insert(alias.to_string(), canonical.clone());
        }
        let registration_id = self.next_registration_id();
        self.wire_names.insert(wire_name, canonical.clone());
        self.tools
            .insert(canonical, RegisteredTool::new(registration_id, tool));
        Ok(registration_id)
    }

    /// Remove a tool by `ToolId` (canonical name is `id.to_string()`). Does
    /// NOT touch aliases — callers wipe aliases separately.
    fn remove_tool_by_id(&mut self, id: &ToolId) {
        let canonical = id.to_string();
        self.tools.remove(&canonical);
        self.wire_names.retain(|_, value| value != &canonical);
    }
}

/// Registry of available tools. Populated at startup by coco-cli.
///
/// Supports lookup by name, alias, and ToolId.
/// Feature-gated tools are registered but may return is_enabled() == false.
///
/// Interior mutability via a single `RwLock` allows `register` and
/// `deregister_by_server` to take `&self`, so the registry can be
/// mutated after it is wrapped in `Arc` (required for runtime MCP
/// tool registration after servers connect).
pub struct ToolRegistry {
    inner: RwLock<RegistryInner>,
}

/// Where a materialized tool sits in the model-facing surface for one request.
///
/// Assigned exactly once during [`ToolRegistry::materialize`], making
/// contradictory membership (loaded *and* deferred) unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolPlacement {
    /// Schema is in the model's direct tool list.
    Loaded,
    /// Registered and searchable, but withheld until ToolSearch discovers it.
    Deferred,
    /// Discoverable by ToolSearch and callable only through `use_tool`.
    UseTool,
}

#[derive(Clone)]
pub struct MaterializedTool {
    pub tool_id: ToolId,
    pub canonical_name: String,
    /// Provider-facing name the model sees and calls. Derived from `tool_id`;
    /// equals `canonical_name` for built-ins and in-budget MCP names.
    pub wire_name: WireToolName,
    pub registration_id: ToolRegistrationId,
    pub placement: ToolPlacement,
    /// Eligible for ToolSearch in this request. This remains true after a
    /// deferred tool has been promoted to `Loaded`, making re-selection an
    /// idempotent operation without consulting the live registry.
    pub discoverable: bool,
    pub tool: Arc<dyn DynTool>,
}

impl MaterializedTool {
    fn from_registered(
        canonical_name: String,
        registered: &RegisteredTool,
        placement: ToolPlacement,
        discoverable: bool,
    ) -> Self {
        let tool_id = registered.tool.id();
        let wire_name = WireToolName::for_tool_id(&tool_id);
        Self {
            tool_id,
            canonical_name,
            wire_name,
            registration_id: registered.registration_id,
            placement,
            discoverable,
            tool: registered.tool.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ToolMaterialization {
    /// One entry per tool that survived the filter pipeline, each tagged with
    /// its [`ToolPlacement`]. Single source of truth for the request surface.
    tools: Vec<MaterializedTool>,
    aliases: HashMap<String, String>,
    by_wire_name: HashMap<WireToolName, usize>,
    by_tool_id: HashMap<ToolId, usize>,
    tool_search_strategy: crate::ToolSearchStrategy,
}

impl ToolMaterialization {
    /// Discovery transport captured with this request snapshot.
    pub fn tool_search_strategy(&self) -> crate::ToolSearchStrategy {
        self.tool_search_strategy
    }

    /// Tools whose schema is in the model's direct tool list.
    pub fn loaded(&self) -> impl Iterator<Item = &MaterializedTool> {
        self.tools
            .iter()
            .filter(|t| t.placement == ToolPlacement::Loaded)
    }

    /// Tools withheld until ToolSearch discovers them.
    pub fn deferred(&self) -> impl Iterator<Item = &MaterializedTool> {
        self.tools
            .iter()
            .filter(|t| t.placement == ToolPlacement::Deferred)
    }

    /// Tools reachable only through `use_tool`.
    pub fn use_tool_targets(&self) -> impl Iterator<Item = &MaterializedTool> {
        self.tools
            .iter()
            .filter(|t| t.placement == ToolPlacement::UseTool)
    }

    /// Every materialized tool, regardless of placement.
    pub fn all_materialized(&self) -> &[MaterializedTool] {
        &self.tools
    }

    /// Request-snapshot-scoped ToolSearch corpus. Transport carriers are never
    /// included.
    pub fn searchable(&self) -> impl Iterator<Item = &MaterializedTool> {
        self.tools.iter().filter(|tool| tool.discoverable)
    }

    /// Resolve a provider-supplied wire name to its materialized tool. The
    /// `use_tool` preparer uses this to unwrap a `use_tool { name }`
    /// call — the registry owns the `WireToolName <-> ToolId` mapping, so no
    /// execution path parses a wire name back into a `ToolId`.
    pub fn lookup_by_wire_name(&self, wire_name: &str) -> Option<&MaterializedTool> {
        self.by_wire_name
            .get(wire_name)
            .and_then(|index| self.tools.get(*index))
    }

    /// Classify a provider call against this request's wire-name index.
    pub fn lookup_wire(&self, registry: &ToolRegistry, wire_name: &str) -> MaterializedToolLookup {
        self.lookup_by_wire_name(wire_name)
            .map_or(MaterializedToolLookup::Unavailable, |tool| {
                self.classify_lookup(registry, tool)
            })
    }

    /// The `Arc`s whose schema is in the model's direct tool list.
    pub fn loaded_tools(&self) -> Vec<Arc<dyn DynTool>> {
        self.loaded().map(|t| t.tool.clone()).collect()
    }

    /// The deferred `Arc`s (see [`Self::deferred`]).
    pub fn deferred_tools(&self) -> Vec<Arc<dyn DynTool>> {
        self.deferred().map(|t| t.tool.clone()).collect()
    }

    pub fn lookup(&self, registry: &ToolRegistry, id: &ToolId) -> MaterializedToolLookup {
        let direct = self
            .by_tool_id
            .get(id)
            .and_then(|index| self.tools.get(*index))
            .map_or(MaterializedToolLookup::Unavailable, |tool| {
                self.classify_lookup(registry, tool)
            });
        if !matches!(direct, MaterializedToolLookup::Unavailable) {
            return direct;
        }
        let requested_name = id.to_string();
        let Some(canonical_name) = self.aliases.get(&requested_name) else {
            return MaterializedToolLookup::Unavailable;
        };
        self.lookup_canonical(registry, canonical_name)
    }

    fn lookup_canonical(
        &self,
        registry: &ToolRegistry,
        canonical_name: &str,
    ) -> MaterializedToolLookup {
        self.tools
            .iter()
            .find(|tool| tool.canonical_name == canonical_name)
            .map_or(MaterializedToolLookup::Unavailable, |tool| {
                self.classify_lookup(registry, tool)
            })
    }

    fn classify_lookup(
        &self,
        registry: &ToolRegistry,
        tool: &MaterializedTool,
    ) -> MaterializedToolLookup {
        match tool.placement {
            ToolPlacement::Loaded => match registry.current_registration_id(&tool.canonical_name) {
                Some(current) if current == tool.registration_id => {
                    MaterializedToolLookup::Loaded(tool.clone())
                }
                Some(_) | None => MaterializedToolLookup::Stale {
                    name: tool.canonical_name.clone(),
                },
            },
            ToolPlacement::Deferred if self.tool_search_strategy.uses_server_side_expansion() => {
                match registry.current_registration_id(&tool.canonical_name) {
                    Some(current) if current == tool.registration_id => {
                        // Native discovery providers authorize the eventual
                        // direct call server-side. The client intentionally
                        // does not promote these tools into `Loaded`, because
                        // doing so would invalidate the stable tools-array
                        // prefix after every search.
                        MaterializedToolLookup::Loaded(tool.clone())
                    }
                    Some(_) | None => MaterializedToolLookup::Stale {
                        name: tool.canonical_name.clone(),
                    },
                }
            }
            ToolPlacement::Deferred => MaterializedToolLookup::Deferred {
                name: tool.canonical_name.clone(),
                tool: tool.tool.clone(),
            },
            ToolPlacement::UseTool => MaterializedToolLookup::Unavailable,
        }
    }
}

fn build_materialization(
    tools: Vec<MaterializedTool>,
    aliases: HashMap<String, String>,
    tool_search_strategy: crate::ToolSearchStrategy,
) -> ToolMaterialization {
    let by_wire_name = tools
        .iter()
        .enumerate()
        .map(|(index, tool)| (tool.wire_name.clone(), index))
        .collect();
    let by_tool_id = tools
        .iter()
        .enumerate()
        .map(|(index, tool)| (tool.tool_id.clone(), index))
        .collect();
    ToolMaterialization {
        tools,
        aliases,
        by_wire_name,
        by_tool_id,
        tool_search_strategy,
    }
}

fn is_transport_carrier(id: &ToolId) -> bool {
    matches!(
        id,
        ToolId::Builtin(ToolName::ToolSearch | ToolName::UseTool)
    )
}

fn visible_aliases_for(
    inner: &RegistryInner,
    tools: &[MaterializedTool],
) -> HashMap<String, String> {
    let visible_canonicals: HashSet<&str> = tools
        .iter()
        .map(|tool| tool.canonical_name.as_str())
        .collect();
    inner
        .aliases
        .iter()
        .filter(|(_, canonical)| visible_canonicals.contains(canonical.as_str()))
        .map(|(alias, canonical)| (alias.clone(), canonical.clone()))
        .collect()
}

/// Context-aware tool lookup for committed model tool calls.
pub enum ToolLookup {
    /// Tool is visible in the current turn's loaded tool set.
    Loaded(Arc<dyn DynTool>),
    /// Tool exists and is enabled, but is deferred until ToolSearch discovers it.
    Deferred { name: String },
    /// Tool is unknown or filtered out by feature/override/agent policy.
    Unavailable,
}

/// Context-aware lookup against one provider-turn tool materialization.
pub enum MaterializedToolLookup {
    /// Tool was loaded in the provider-turn snapshot and still has the same
    /// registry identity at settlement.
    Loaded(MaterializedTool),
    /// Tool was known to this snapshot, but deferred until ToolSearch discovers it.
    Deferred {
        name: String,
        tool: Arc<dyn DynTool>,
    },
    /// Tool was loaded in this snapshot, but its registry entry was removed or replaced.
    Stale { name: String },
    /// Tool was not part of the provider-turn call surface.
    Unavailable,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            inner: RwLock::new(RegistryInner::default()),
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Also registers all its aliases.
    ///
    /// Static/trusted registration. Canonical identity always comes from
    /// `Tool::id()`; MCP metadata must match that structured `ToolId` exactly,
    /// so an MCP tool can neither shadow a built-in nor smuggle a second
    /// server/tool identity through its display name.
    pub fn register(&self, tool: Arc<dyn DynTool>) {
        if let Err(error) = self.try_register(tool) {
            panic!("invalid static tool registration: {error}");
        }
    }

    /// Fallible registration for dynamic tools. Collision checks complete
    /// before the tool becomes visible.
    pub fn try_register(
        &self,
        tool: Arc<dyn DynTool>,
    ) -> Result<ToolRegistrationId, ToolRegistrationError> {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.register_with_aliases(tool)
    }

    /// Look up a tool by ToolId.
    pub fn get(&self, id: &ToolId) -> Option<Arc<dyn DynTool>> {
        self.get_by_name(&id.to_string())
    }

    /// Look up a tool exactly as the model is allowed to call it this turn.
    ///
    /// Raw registry lookup intentionally sees every registered tool, including
    /// disabled and deferred entries. Execution preparation needs the stricter
    /// model-facing view: feature/override/filter gates applied, and deferred
    /// tools rejected until ToolSearch has promoted them into the loaded set.
    pub fn lookup_loaded(&self, id: &ToolId, ctx: &ToolUseContext) -> ToolLookup {
        match self.materialize(ctx).lookup(self, id) {
            MaterializedToolLookup::Loaded(tool) => ToolLookup::Loaded(tool.tool),
            MaterializedToolLookup::Deferred { name, .. } => ToolLookup::Deferred { name },
            MaterializedToolLookup::Stale { .. } | MaterializedToolLookup::Unavailable => {
                ToolLookup::Unavailable
            }
        }
    }

    /// Look up a tool by name or alias.
    pub fn get_by_name(&self, name: &str) -> Option<Arc<dyn DynTool>> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.tools.get(name).map(|t| t.tool.clone()).or_else(|| {
            inner
                .aliases
                .get(name)
                .and_then(|canonical| inner.tools.get(canonical).map(|t| t.tool.clone()))
        })
    }

    /// Current registration id for a canonical name, or `None` if the tool was
    /// deregistered. Used to detect a stale materialization entry (the `use_tool`
    /// resolver and `MaterializedToolLookup` both fail closed on a mismatch).
    pub fn current_registration_id(&self, canonical_name: &str) -> Option<ToolRegistrationId> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.tools.get(canonical_name).map(|t| t.registration_id)
    }

    /// Get all registered tools (clones the Arc handles).
    pub fn all(&self) -> Vec<Arc<dyn DynTool>> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.tools.values().map(|t| t.tool.clone()).collect()
    }

    /// Get enabled tools after running the schema-time filter pipeline
    /// (`is_enabled` × `ToolOverrides` × `ToolFilter`). Plan-mode
    /// read-only is a call-time permission concern, not a filter here.
    /// See `docs/internal/feature-gates-and-tool-filtering.md` §7.
    pub fn enabled(&self, ctx: &ToolUseContext) -> Vec<Arc<dyn DynTool>> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .tools
            .values()
            .filter(|t| passes_filter_pipeline(t.tool.as_ref(), ctx))
            .map(|t| t.tool.clone())
            .collect()
    }

    /// Get non-deferred enabled tools (loaded immediately).
    ///
    /// A deferred tool whose wire-name appears in
    /// `ctx.discovered_tool_names` is treated as if it were not
    /// deferred — that is the mechanism through which the
    /// model "loads" a tool via `ToolSearch`. `always_load()` still
    /// short-circuits the deferral check independent of discovery.
    ///
    /// When [`coco_types::Feature::ToolSearch`] is **off**, the
    /// deferral check is bypassed entirely: every enabled tool's
    /// full schema lands in turn-1 requests.
    /// Keeps the per-Provider serialization path identical, just
    /// without the lazy-loading optimization.
    pub fn loaded_tools(&self, ctx: &ToolUseContext) -> Vec<Arc<dyn DynTool>> {
        self.materialize(ctx).loaded_tools()
    }

    pub fn materialize(&self, ctx: &ToolUseContext) -> ToolMaterialization {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let builtin_tool_search_active = ctx.tool_search_supported();
        let mut tools = Vec::new();
        for (canonical, registered) in &inner.tools {
            if is_transport_carrier(&registered.tool.id()) {
                continue;
            }
            if !passes_filter_pipeline(registered.tool.as_ref(), ctx) {
                continue;
            }
            // Discovery is keyed on canonical identity (`id.to_string()`, which
            // is the loop `canonical`), never bare `name()`: two servers with
            // the same bare tool name must not both count as discovered when
            // only one was selected.
            let discovered = ctx.discovered_tool_names.contains(canonical.as_str());
            let (placement, discoverable) = if let Some(info) = registered.tool.mcp_info() {
                match ctx.mcp_tool_exposure_for(&info.server_name) {
                    coco_types::McpToolExposure::Load => (ToolPlacement::Loaded, false),
                    // Explicit server policy wins over a tool's `alwaysLoad`
                    // hint: `use_tool` never injects target schemas directly.
                    coco_types::McpToolExposure::UseTool => (ToolPlacement::UseTool, true),
                    coco_types::McpToolExposure::Defer
                        if registered.tool.always_load() || !registered.tool.should_defer() =>
                    {
                        (ToolPlacement::Loaded, false)
                    }
                    coco_types::McpToolExposure::Defer
                        if ctx.tool_search_strategy.is_supported() && discovered =>
                    {
                        (ToolPlacement::Loaded, true)
                    }
                    coco_types::McpToolExposure::Defer
                        if ctx.tool_search_strategy.is_supported() =>
                    {
                        (ToolPlacement::Deferred, true)
                    }
                    // A model without a schema-promotion strategy cannot
                    // implement `defer`; retain lazy exposure through
                    // `use_tool` instead of eagerly loading every MCP schema.
                    coco_types::McpToolExposure::Defer => (ToolPlacement::UseTool, true),
                }
            } else {
                let should_defer = builtin_tool_search_active
                    && registered.tool.should_defer()
                    && !registered.tool.always_load()
                    && !discovered;
                let discoverable = builtin_tool_search_active
                    && registered.tool.should_defer()
                    && !registered.tool.always_load();
                let placement = if should_defer {
                    ToolPlacement::Deferred
                } else {
                    ToolPlacement::Loaded
                };
                (placement, discoverable)
            };
            tools.push(MaterializedTool::from_registered(
                canonical.clone(),
                registered,
                placement,
                discoverable,
            ));
        }
        // Transport closure: carriers are derived after ordinary target
        // filtering. Agent/model allowlists may narrow targets but cannot make
        // surviving deferred or `use_tool` targets unreachable.
        for carrier in [ToolName::ToolSearch, ToolName::UseTool] {
            let canonical = carrier.as_str();
            let Some(registered) = inner.tools.get(canonical) else {
                continue;
            };
            if registered.tool.is_enabled(ctx) {
                tools.push(MaterializedTool::from_registered(
                    canonical.to_string(),
                    registered,
                    ToolPlacement::Loaded,
                    false,
                ));
            }
        }
        let aliases = visible_aliases_for(&inner, &tools);
        build_materialization(tools, aliases, ctx.tool_search_strategy)
    }

    /// Materialize every enabled tool while separately marking the tools that
    /// would normally be deferred. This is used by providers that support a
    /// native deferred-tool-reference flag in the tool definition itself.
    pub fn materialize_with_deferred_references(
        &self,
        ctx: &ToolUseContext,
    ) -> (ToolMaterialization, HashSet<String>) {
        let materialization = self.materialize(ctx);
        let deferred_marker = materialization
            .deferred()
            .map(|tool| tool.canonical_name.clone())
            .collect();
        (materialization, deferred_marker)
    }

    pub fn deferred_tool_names(&self, ctx: &ToolUseContext) -> HashSet<String> {
        self.materialize(ctx)
            .deferred()
            .map(|t| t.canonical_name.clone())
            .collect()
    }

    /// Get deferred tools (discovered via ToolSearch).
    ///
    /// Symmetric to [`Self::loaded_tools`]: deferred tools that have
    /// been discovered are *excluded* — they have moved into the
    /// loaded set for this turn.
    ///
    /// Returns empty when [`coco_types::Feature::ToolSearch`] is
    /// off — there is no deferred pool to surface, every tool is
    /// already loaded via [`Self::loaded_tools`].
    pub fn deferred_tools(&self, ctx: &ToolUseContext) -> Vec<Arc<dyn DynTool>> {
        self.materialize(ctx).deferred_tools()
    }

    /// Deferred tools eligible to be surfaced by `ToolSearch`.
    ///
    /// Same pipeline gate as [`Self::loaded_tools`] / [`Self::deferred_tools`]
    /// (`is_enabled` × `ToolOverrides` × `ToolFilter`), so `ToolSearch` can
    /// never match a tool the registry would refuse to surface — a match
    /// that doesn't pass the pipeline is inert (it can't enter
    /// `loaded_tools`) and would make the model re-search forever. Unlike
    /// [`Self::deferred_tools`] this does NOT exclude already-discovered
    /// names: re-selecting a discovered tool is an idempotent no-op
    /// (`select:` semantics: re-selecting a discovered tool is idempotent).
    pub fn searchable_deferred(&self, ctx: &ToolUseContext) -> Vec<Arc<dyn DynTool>> {
        self.materialize(ctx)
            .searchable()
            .map(|tool| tool.tool.clone())
            .collect()
    }

    /// Deregister all tools from a specific MCP server.
    ///
    /// Called when an MCP server disconnects. Removes all tools whose
    /// `mcp_info().server_name` matches the given server name, plus
    /// their aliases. Full re-discovery happens on reconnect.
    pub fn deregister_by_server(&self, server_name: &str) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let to_remove: Vec<String> = inner
            .tools
            .iter()
            .filter(|(_, registered)| {
                registered
                    .tool
                    .mcp_info()
                    .is_some_and(|info| info.server_name == server_name)
            })
            .map(|(name, _)| name.clone())
            .collect();

        for name in &to_remove {
            inner.tools.remove(name);
        }
        inner
            .wire_names
            .retain(|_, canonical| !to_remove.contains(canonical));

        // Also remove aliases that point to removed tools
        inner
            .aliases
            .retain(|_, canonical| !to_remove.contains(canonical));
    }

    /// Atomically replace all tools belonging to `server_name` with
    /// `new_tools`, under a SINGLE write lock (no window where readers see a
    /// partial set — fixes the non-transactional `deregister`+loop-`register`
    /// reconnect path). Returns the tombstoned `ToolId`s: present in the
    /// previous batch but absent from `new_tools`.
    ///
    /// All server-owned aliases are wiped by **full membership** BEFORE
    /// re-registering, so a retained tool whose advertised alias set changed
    /// across reconnect leaves no stale alias (v4.2 finding 6).
    pub fn replace_server_tools(
        &self,
        server_name: &str,
        new_tools: Vec<Arc<dyn DynTool>>,
    ) -> Result<Vec<ToolId>, ToolRegistrationError> {
        for tool in &new_tools {
            let tool_id = tool.id();
            let actual_server = tool.mcp_info().map(|info| info.server_name.clone());
            if actual_server.as_deref() != Some(server_name) {
                return Err(ToolRegistrationError::ServerOwnershipMismatch {
                    expected_server: server_name.to_string(),
                    actual_server,
                    tool_id,
                });
            }
        }
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut next = inner.clone();

        // 1. Snapshot the server's current canonical names + ToolIds.
        let owned: Vec<(String, ToolId)> = next
            .tools
            .iter()
            .filter(|(_, t)| {
                t.tool
                    .mcp_info()
                    .is_some_and(|i| i.server_name == server_name)
            })
            .map(|(name, t)| (name.clone(), t.tool.id()))
            .collect();
        let owned_names: std::collections::HashSet<String> =
            owned.iter().map(|(n, _)| n.clone()).collect();
        let old_ids: std::collections::HashSet<ToolId> =
            owned.into_iter().map(|(_, id)| id).collect();
        let new_ids: std::collections::HashSet<ToolId> = new_tools.iter().map(|t| t.id()).collect();
        let mut tombstones: Vec<ToolId> = old_ids.difference(&new_ids).cloned().collect();
        tombstones.sort_by_key(std::string::ToString::to_string);

        // 2. Wipe ALL server-owned aliases (full membership, not just tombstones).
        next.aliases
            .retain(|_, canonical| !owned_names.contains(canonical.as_str()));

        // 3. Remove the entire previous server batch from the staging copy.
        // Retained tools are reinserted below with fresh registration ids.
        for id in &old_ids {
            next.remove_tool_by_id(id);
        }

        // 4. Re-register the new batch — re-establishes aliases fresh and
        //    overwrites retained tools with their new (reconnect) instance.
        for tool in new_tools {
            next.register_with_aliases(tool)?;
        }

        *inner = next;
        Ok(tombstones)
    }

    pub fn len(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tools
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tools
            .is_empty()
    }
}

#[cfg(test)]
#[path = "registry.test.rs"]
mod tests;
