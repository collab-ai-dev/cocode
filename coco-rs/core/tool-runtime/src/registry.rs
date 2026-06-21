use coco_types::MCP_TOOL_PREFIX;
use coco_types::ToolId;
use std::collections::HashMap;
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
/// and `docs/coco-rs/feature-gates-and-tool-filtering.md` §7/§9.
///
/// MCP server reachability is likewise not checked here; MCP tools whose
/// backing server disconnects are removed from the registry via
/// `ToolRegistry::deregister_by_server`, so they never reach this pipeline.
fn passes_filter_pipeline(tool: &dyn DynTool, ctx: &ToolUseContext) -> bool {
    let id = tool.id();
    tool.is_enabled(ctx) && ctx.tool_overrides.permits(&id) && ctx.tool_filter.allows(&id)
}

/// Inner state protected by a single RwLock.
///
/// Both maps are always mutated together (every `register` touches
/// `tools` and may also touch `aliases`; every `deregister_by_server`
/// touches both). A single lock ensures the two maps are always
/// consistent — no window where `tools` has a new entry but `aliases`
/// does not, or vice versa.
#[derive(Default)]
struct RegistryInner {
    /// Primary lookup: canonical name → tool.
    tools: HashMap<String, Arc<dyn DynTool>>,
    /// Alias lookup: alias → canonical name.
    aliases: HashMap<String, String>,
}

impl RegistryInner {
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
        self.tools.insert(canonical, tool);
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

/// Context-aware tool lookup for committed model tool calls.
pub enum ToolLookup {
    /// Tool is visible in the current turn's loaded tool set.
    Loaded(Arc<dyn DynTool>),
    /// Tool exists and is enabled, but is deferred until ToolSearch discovers it.
    Deferred { name: String },
    /// Tool is unknown or filtered out by feature/override/agent policy.
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
        let Some(tool) = self.get(id) else {
            return ToolLookup::Unavailable;
        };
        if !passes_filter_pipeline(tool.as_ref(), ctx) {
            return ToolLookup::Unavailable;
        }
        let tool_search_active = ctx.tool_search_active();
        if tool_search_active
            && tool.should_defer()
            && !tool.always_load()
            && !ctx.discovered_tool_names.contains(tool.name())
        {
            return ToolLookup::Deferred {
                name: tool.name().to_string(),
            };
        }
        ToolLookup::Loaded(tool)
    }

    /// Look up a tool by name or alias.
    pub fn get_by_name(&self, name: &str) -> Option<Arc<dyn DynTool>> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.tools.get(name).cloned().or_else(|| {
            inner
                .aliases
                .get(name)
                .and_then(|canonical| inner.tools.get(canonical))
                .cloned()
        })
    }

    /// Get all registered tools (clones the Arc handles).
    pub fn all(&self) -> Vec<Arc<dyn DynTool>> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.tools.values().cloned().collect()
    }

    /// Get enabled tools after running the schema-time filter pipeline
    /// (`is_enabled` × `ToolOverrides` × `ToolFilter`). Plan-mode
    /// read-only is a call-time permission concern, not a filter here.
    /// See `docs/coco-rs/feature-gates-and-tool-filtering.md` §7.
    pub fn enabled(&self, ctx: &ToolUseContext) -> Vec<Arc<dyn DynTool>> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .tools
            .values()
            .filter(|t| passes_filter_pipeline(t.as_ref(), ctx))
            .cloned()
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
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tool_search_active = ctx.tool_search_active();
        inner
            .tools
            .values()
            .filter(|t| {
                passes_filter_pipeline(t.as_ref(), ctx)
                    && (!tool_search_active
                        || !t.should_defer()
                        || t.always_load()
                        || ctx.discovered_tool_names.contains(t.name()))
            })
            .cloned()
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
        if !ctx.tool_search_active() {
            return Vec::new();
        }
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .tools
            .values()
            .filter(|t| {
                passes_filter_pipeline(t.as_ref(), ctx)
                    && t.should_defer()
                    && !t.always_load()
                    && !ctx.discovered_tool_names.contains(t.name())
            })
            .cloned()
            .collect()
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
                passes_filter_pipeline(t.as_ref(), ctx) && t.should_defer() && !t.always_load()
            })
            .cloned()
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
            .filter(|(_, tool)| {
                tool.mcp_info()
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
            .filter(|(_, t)| t.mcp_info().is_some_and(|i| i.server_name == server_name))
            .map(|(name, t)| (name.clone(), t.id()))
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
