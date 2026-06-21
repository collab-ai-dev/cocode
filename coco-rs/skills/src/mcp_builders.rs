//! Write-once registry for MCP-sourced skill construction.
//!
//! The registry is the leaf module that both the registrar and the consumer
//! (MCP client + capability discovery) can reference without forming a
//! dependency cycle.
//!
//! Layer rule reminder: `coco-mcp` (L3) does not depend on `coco-skills`
//! (L4). The wiring code in higher layers (e.g. `app/cli::SessionRuntime`)
//! registers a builder here at startup and bridges MCP resource discovery
//! callbacks into [`crate::SkillManager::register_mcp_skill`]. This module
//! itself has zero external dependencies beyond [`coco_frontmatter`].
//!
//! ## Default builder
//!
//! [`DefaultMcpSkillBuilder`] parses the MCP resource body as YAML
//! frontmatter + markdown, mirroring the same logic as the on-disk skill
//! loader ([`crate::parse_skill_markdown`]) but with the skill name and
//! source supplied by the MCP server rather than derived from a file path.
//!
//! ## Registration
//!
//! Call [`register_mcp_skill_builder`] exactly once at application startup
//! (before any MCP server connects). A second registration is a no-op so
//! tests can swap in a custom builder without panicking.

use std::sync::Arc;
use std::sync::OnceLock;

use crate::SkillContext;
use crate::SkillDefinition;
use crate::SkillSource;
use crate::SkillsError;
use coco_types::ModelRole;

/// Wire-shape input emitted by an MCP server for one skill.
///
/// Mirrors what `services/mcp/client.ts::fetchMcpSkillsForClient` reads
/// off an MCP resource: server identity, the resource URI, optional MCP
/// metadata (name / description), and the raw markdown body (with
/// optional frontmatter). The builder consumes this and returns a fully
/// typed [`SkillDefinition`] with [`SkillSource::Mcp`].
#[derive(Debug, Clone)]
pub struct McpSkillSpec {
    /// Server name as known to `McpConnectionManager`. Tagged onto the
    /// resulting skill via [`SkillSource::Mcp { server_name }`].
    pub server_name: String,
    /// MCP resource URI (e.g. `skill://my-server/lint-fix`).
    pub uri: String,
    /// Skill name. Typically the basename of [`Self::uri`] or the MCP
    /// resource's `name` field. Used as the [`SkillDefinition::name`]
    /// lookup key.
    pub name: String,
    /// Optional human-readable description from MCP metadata. Used as a
    /// fallback when the markdown body has no frontmatter `description`.
    pub description: Option<String>,
    /// Raw markdown body. May contain a leading `---` YAML frontmatter
    /// block followed by the prompt text.
    pub content: String,
}

/// Strategy for converting an [`McpSkillSpec`] into a [`SkillDefinition`].
///
/// In Rust we collapse both the creation and frontmatter-parsing steps into a
/// single trait method since the consumer only needs the end-to-end build path.
pub trait McpSkillBuilder: Send + Sync {
    /// Build a typed [`SkillDefinition`] from the MCP wire shape.
    fn build(&self, spec: &McpSkillSpec) -> Result<SkillDefinition, SkillsError>;
}

/// Default builder — parses [`McpSkillSpec::content`] for YAML
/// frontmatter and falls back to the MCP metadata when the frontmatter is
/// silent. Mirrors the field set parsed by [`crate::parse_skill_markdown`].
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultMcpSkillBuilder;

impl McpSkillBuilder for DefaultMcpSkillBuilder {
    fn build(&self, spec: &McpSkillSpec) -> Result<SkillDefinition, SkillsError> {
        build_mcp_skill_default(spec)
    }
}

/// Default body of [`DefaultMcpSkillBuilder::build`], extracted so other
/// builders can compose it (e.g. wrap with extra validation).
pub fn build_mcp_skill_default(spec: &McpSkillSpec) -> Result<SkillDefinition, SkillsError> {
    use coco_frontmatter::FrontmatterValue;

    let frontmatter = coco_frontmatter::parse(&spec.content);
    let data = &frontmatter.data;

    let lookup = |aliases: &[&str]| -> Option<&FrontmatterValue> {
        aliases.iter().find_map(|k| data.get(*k))
    };
    let lookup_str = |aliases: &[&str]| -> Option<String> {
        lookup(aliases)
            .and_then(FrontmatterValue::as_str)
            .map(str::to_owned)
    };
    let lookup_bool =
        |aliases: &[&str]| -> Option<bool> { lookup(aliases).and_then(FrontmatterValue::as_bool) };

    // Description resolution: frontmatter > MCP metadata > markdown-body fallback.
    let fm_description = lookup_str(&["description"]).filter(|s| !s.trim().is_empty());
    let spec_description = spec.description.as_ref().and_then(|s| {
        let trimmed = s.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    });
    let has_user_specified_description = fm_description.is_some() || spec_description.is_some();
    let description = fm_description
        .or(spec_description)
        .unwrap_or_else(|| crate::extract_description_from_markdown(&frontmatter.content, "Skill"));

    let display_name = lookup(&["name"]).and_then(scalar_to_string_local);

    let allowed_tools = lookup(&["allowed-tools", "allowed_tools"]).map(value_to_csv_list_local);
    let model = lookup_str(&["model"]);
    let model_role = lookup_str(&["model-role", "model_role", "modelRole"])
        .and_then(|raw| raw.parse::<ModelRole>().ok());
    let when_to_use = lookup_str(&["when-to-use", "when_to_use"]);

    let argument_names = lookup(&["arguments", "argument-names", "argument_names"])
        .map(|v| match v {
            FrontmatterValue::Sequence(_) => v
                .as_string_list()
                .map(|items| items.into_iter().map(str::to_owned).collect())
                .unwrap_or_default(),
            FrontmatterValue::String(s) => s
                .split_whitespace()
                .map(str::to_string)
                .filter(|s| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit()))
                .collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default();

    let aliases = lookup(&["aliases"])
        .map(value_to_csv_list_local)
        .unwrap_or_default();

    let effort = lookup_str(&["effort"]).and_then(|s| s.trim().parse().ok());
    let context = match lookup_str(&["context"]).as_deref() {
        Some("fork") => SkillContext::Fork,
        _ => SkillContext::Inline,
    };
    let agent = lookup_str(&["agent"]);
    let version = lookup(&["version"]).and_then(scalar_to_string_local);
    let disabled = lookup_bool(&["disabled"]).unwrap_or(false);
    let argument_hint = lookup_str(&["argument-hint", "argument_hint"]);
    let user_invocable = lookup_bool(&["user-invocable", "user_invocable"]).unwrap_or(true);
    let disable_model_invocation =
        lookup_bool(&["disable-model-invocation", "disable_model_invocation"]).unwrap_or(false);

    let hooks = lookup(&["hooks"]).map(FrontmatterValue::to_json);
    let shell = lookup(&["shell"]).map(FrontmatterValue::to_json);

    let prompt = frontmatter.content.trim().to_string();
    let content_length = prompt.len() as i64;
    let is_hidden = !user_invocable;

    Ok(SkillDefinition {
        name: spec.name.clone(),
        display_name,
        description,
        prompt,
        progress_message: Some("running".to_string()),
        has_user_specified_description,
        source: SkillSource::Mcp {
            server_name: spec.server_name.clone(),
        },
        aliases,
        allowed_tools,
        model,
        model_role,
        when_to_use,
        argument_names,
        // MCP-sourced skills have no on-disk `paths` semantics (the path
        // glob conditional-activation feature only makes sense for skills
        // loaded from the filesystem). Always empty.
        paths: Vec::new(),
        effort,
        context,
        agent,
        version,
        disabled,
        hooks,
        argument_hint,
        user_invocable,
        disable_model_invocation,
        shell,
        content_length,
        is_hidden,
        gated_by: None,
        files: std::collections::HashMap::new(),
        skill_root: None,
    })
}

// Local copies of the small helpers used by the disk-loader path. Kept
// private so the public API surface stays minimal; if we end up needing
// these in a third place we can promote to `pub(crate)` in `lib.rs`.

fn scalar_to_string_local(v: &coco_frontmatter::FrontmatterValue) -> Option<String> {
    use coco_frontmatter::FrontmatterValue;
    match v {
        FrontmatterValue::String(s) => Some(s.clone()),
        FrontmatterValue::Int(n) => Some(n.to_string()),
        FrontmatterValue::Float(f) => Some(f.to_string()),
        FrontmatterValue::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn value_to_csv_list_local(v: &coco_frontmatter::FrontmatterValue) -> Vec<String> {
    use coco_frontmatter::FrontmatterValue;
    match v {
        FrontmatterValue::Sequence(_) => v
            .as_string_list()
            .map(|items| items.into_iter().map(str::to_owned).collect())
            .unwrap_or_default(),
        FrontmatterValue::String(s) => s
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

// ─── Write-once registry ────────────────────────────────────────────────

static MCP_SKILL_BUILDER: OnceLock<Arc<dyn McpSkillBuilder>> = OnceLock::new();

/// Register the process-wide MCP skill builder.
///
/// Returns `true` if registration succeeded, `false` if a builder was
/// already registered (no-op). Callers SHOULD NOT panic on a false
/// return — test runners legitimately register multiple times across
/// integration tests.
pub fn register_mcp_skill_builder(builder: Arc<dyn McpSkillBuilder>) -> bool {
    MCP_SKILL_BUILDER.set(builder).is_ok()
}

/// Get the registered MCP skill builder, falling back to
/// [`DefaultMcpSkillBuilder`] if none was registered yet. Cheap to call
/// — the fallback is a zero-sized type.
///
/// Never panics on uninitialized state — the fallback keeps the codepath
/// unconditional.
pub fn mcp_skill_builder() -> Arc<dyn McpSkillBuilder> {
    MCP_SKILL_BUILDER
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(DefaultMcpSkillBuilder) as Arc<dyn McpSkillBuilder>)
}

#[cfg(test)]
#[path = "mcp_builders.test.rs"]
mod tests;
