use coco_types::MCP_TOOL_PREFIX;
use coco_types::ToolId;
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

#[derive(Default)]
struct RegistryInner {
    /// Primary lookup: canonical name → tool.
    tools: HashMap<String, RegisteredTool>,
    /// Alias lookup: alias → canonical name.
    aliases: HashMap<String, String>,
    next_registration_id: i64,
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
    fn register_with_aliases(&mut self, tool: Arc<dyn DynTool>) {
        let native_name = tool.name().to_string();
        let canonical = if let Some(info) = tool.mcp_info() {
            let qualified = info.qualified_name();
            if native_name == qualified || native_name.starts_with(MCP_TOOL_PREFIX) {
                native_name
            } else {
                self.aliases.insert(native_name, qualified.clone());
                qualified
            }
        } else {
            native_name
        };
        for alias in tool.aliases() {
            self.aliases.insert(alias.to_string(), canonical.clone());
        }
        let registration_id = self.next_registration_id();
        self.tools
            .insert(canonical, RegisteredTool::new(registration_id, tool));
    }

    /// Remove a tool by `ToolId` (canonical name is `id.to_string()`). Does
    /// NOT touch aliases — callers wipe aliases separately.
    fn remove_tool_by_id(&mut self, id: &ToolId) {
        self.tools.remove(&id.to_string());
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

#[derive(Clone)]
pub struct MaterializedTool {
    pub tool_id: ToolId,
    pub canonical_name: String,
    pub registration_id: ToolRegistrationId,
    pub tool: Arc<dyn DynTool>,
}

impl MaterializedTool {
    fn from_registered(canonical_name: String, registered: &RegisteredTool) -> Self {
        Self {
            tool_id: registered.tool.id(),
            canonical_name,
            registration_id: registered.registration_id,
            tool: registered.tool.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ToolMaterialization {
    loaded: Vec<MaterializedTool>,
    deferred: Vec<MaterializedTool>,
    aliases: HashMap<String, String>,
}

impl ToolMaterialization {
    pub fn loaded_tools(&self) -> Vec<Arc<dyn DynTool>> {
        self.loaded.iter().map(|t| t.tool.clone()).collect()
    }

    pub fn deferred_tools(&self) -> Vec<Arc<dyn DynTool>> {
        self.deferred.iter().map(|t| t.tool.clone()).collect()
    }

    pub fn loaded_materialized(&self) -> &[MaterializedTool] {
        &self.loaded
    }

    pub fn deferred_materialized(&self) -> &[MaterializedTool] {
        &self.deferred
    }

    pub fn lookup(&self, registry: &ToolRegistry, id: &ToolId) -> MaterializedToolLookup {
        let requested_name = id.to_string();
        let direct = self.lookup_canonical(registry, &requested_name);
        if !matches!(direct, MaterializedToolLookup::Unavailable) {
            return direct;
        }
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
        if let Some(tool) = self
            .loaded
            .iter()
            .find(|tool| tool.canonical_name == canonical_name)
        {
            return match registry.current_registration_id(&tool.canonical_name) {
                Some(current) if current == tool.registration_id => {
                    MaterializedToolLookup::Loaded(tool.clone())
                }
                Some(_) | None => MaterializedToolLookup::Stale {
                    name: tool.canonical_name.clone(),
                },
            };
        }
        if let Some(tool) = self
            .deferred
            .iter()
            .find(|tool| tool.canonical_name == canonical_name)
        {
            return MaterializedToolLookup::Deferred {
                name: tool.canonical_name.clone(),
                tool: tool.tool.clone(),
            };
        }
        MaterializedToolLookup::Unavailable
    }
}

fn visible_aliases_for(
    inner: &RegistryInner,
    loaded: &[MaterializedTool],
    deferred: &[MaterializedTool],
) -> HashMap<String, String> {
    let visible_canonicals: HashSet<&str> = loaded
        .iter()
        .chain(deferred.iter())
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
    /// **MCP naming convention** (B3.3): tools that report `mcp_info()`
    /// are normalized to their `qualified_name()` form
    /// `mcp__<server>__<tool>` if their primary name doesn't already
    /// follow that convention. This prevents hostile MCP servers
    ///   from shadowing built-in tools (e.g. an MCP server advertising a
    ///   tool named "Read" is registered as "mcp__foo__Read" rather than
    ///   overwriting the real Read tool).
    pub fn register(&self, tool: Arc<dyn DynTool>) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.register_with_aliases(tool);
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

    fn current_registration_id(&self, canonical_name: &str) -> Option<ToolRegistrationId> {
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
        let tool_search_active = ctx.tool_search_active();
        let mut loaded = Vec::new();
        let mut deferred = Vec::new();
        for (canonical, registered) in &inner.tools {
            if !passes_filter_pipeline(registered.tool.as_ref(), ctx) {
                continue;
            }
            let should_defer = tool_search_active
                && registered.tool.should_defer()
                && !registered.tool.always_load()
                && !ctx.discovered_tool_names.contains(registered.tool.name());
            let tool = MaterializedTool::from_registered(canonical.clone(), registered);
            if should_defer {
                deferred.push(tool);
            } else {
                loaded.push(tool);
            }
        }
        let aliases = visible_aliases_for(&inner, &loaded, &deferred);
        ToolMaterialization {
            loaded,
            deferred,
            aliases,
        }
    }

    /// Materialize every enabled tool while separately marking the tools that
    /// would normally be deferred. This is used by providers that support a
    /// native deferred-tool-reference flag in the tool definition itself.
    pub fn materialize_with_deferred_references(
        &self,
        ctx: &ToolUseContext,
    ) -> (ToolMaterialization, HashSet<String>) {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut loaded = Vec::new();
        let mut deferred_marker = HashSet::new();
        for (canonical, registered) in &inner.tools {
            if !passes_filter_pipeline(registered.tool.as_ref(), ctx) {
                continue;
            }
            if ctx.tool_search_active()
                && registered.tool.should_defer()
                && !registered.tool.always_load()
                && !ctx.discovered_tool_names.contains(registered.tool.name())
            {
                deferred_marker.insert(registered.tool.name().to_string());
            }
            loaded.push(MaterializedTool::from_registered(
                canonical.clone(),
                registered,
            ));
        }
        (
            ToolMaterialization {
                aliases: visible_aliases_for(&inner, &loaded, &[]),
                loaded,
                deferred: Vec::new(),
            },
            deferred_marker,
        )
    }

    pub fn deferred_tool_names(&self, ctx: &ToolUseContext) -> HashSet<String> {
        self.materialize(ctx)
            .deferred_materialized()
            .iter()
            .map(|t| t.tool.name().to_string())
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
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .tools
            .values()
            .filter(|t| {
                passes_filter_pipeline(t.tool.as_ref(), ctx)
                    && t.tool.should_defer()
                    && !t.tool.always_load()
            })
            .map(|t| t.tool.clone())
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
    ) -> Vec<ToolId> {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // 1. Snapshot the server's current canonical names + ToolIds.
        let owned: Vec<(String, ToolId)> = inner
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
        let tombstones: Vec<ToolId> = old_ids.difference(&new_ids).cloned().collect();

        // 2. Wipe ALL server-owned aliases (full membership, not just tombstones).
        inner
            .aliases
            .retain(|_, canonical| !owned_names.contains(canonical.as_str()));

        // 3. Drop tombstoned tools (their aliases already gone via step 2).
        for id in &tombstones {
            inner.remove_tool_by_id(id);
        }

        // 4. Re-register the new batch — re-establishes aliases fresh and
        //    overwrites retained tools with their new (reconnect) instance.
        for tool in new_tools {
            inner.register_with_aliases(tool);
        }

        tombstones
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
