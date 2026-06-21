//! Parse a markdown agent file into an `AgentDefinition`.
//!
//! Both snake_case (`max_turns`) and camelCase (`maxTurns`) keys are
//! accepted; output normalizes to the `AgentDefinition` field set.

use std::path::Path;

use coco_frontmatter::FrontmatterValue;
use coco_types::{
    AgentColorName, AgentDefinition, AgentIsolation, AgentMcpServerSpec, AgentSource, AgentTypeId,
    MemoryScope, ModelRole, ReasoningEffort,
};

use crate::validation::ValidationError;

/// Errors that prevent a markdown file from producing a usable definition.
#[derive(Debug, thiserror::Error)]
pub enum FrontmatterParseError {
    #[error("missing `name` frontmatter field")]
    MissingName,
    #[error("missing `description` frontmatter field")]
    MissingDescription,
    #[error("invalid frontmatter value for `{field}`: {message}")]
    InvalidValue {
        field: &'static str,
        message: String,
    },
}

impl coco_error::StackError for FrontmatterParseError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn coco_error::StackError> {
        None
    }
}

impl coco_error::ErrorExt for FrontmatterParseError {
    fn status_code(&self) -> coco_error::StatusCode {
        coco_error::StatusCode::InvalidArguments
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Parse the markdown body of an agent file along with its frontmatter map.
///
/// `body` is the post-frontmatter content (the system prompt). `path` is
/// recorded as `filename` and `base_dir` for diagnostics. Unknown fields
/// are silently ignored; invalid values for known fields are surfaced as
/// `ValidationError` entries in the returned `warnings` vector — the
/// definition still loads with the field defaulted.
pub fn parse_agent_markdown(
    path: &Path,
    body: &str,
    frontmatter: &std::collections::HashMap<String, FrontmatterValue>,
    source: AgentSource,
) -> Result<(AgentDefinition, Vec<ValidationError>), FrontmatterParseError> {
    // The falsy check rejects empty string — a file with `name: ""` is
    // treated the same as a missing key.
    let name = read_str(frontmatter, "name")
        .filter(|s| !s.trim().is_empty())
        .ok_or(FrontmatterParseError::MissingName)?;
    // YAML keeps the literal `\n` token; restore the real newline.
    let description = read_str_aliased(frontmatter, &["description", "whenToUse", "when_to_use"])
        .map(|s| s.replace("\\n", "\n"))
        .filter(|s| !s.trim().is_empty())
        .ok_or(FrontmatterParseError::MissingDescription)?;

    let body_trimmed = body.trim();
    let mut warnings = Vec::new();
    let mut def = AgentDefinition {
        agent_type: AgentTypeId::from_str_infallible(&name),
        name: name.clone(),
        when_to_use: Some(description.clone()),
        description: Some(description),
        source,
        filename: path.file_name().and_then(|s| s.to_str()).map(str::to_owned),
        base_dir: path.parent().and_then(|p| p.to_str()).map(str::to_owned),
        system_prompt: (!body_trimmed.is_empty()).then(|| body_trimmed.to_owned()),
        ..Default::default()
    };

    if let Some(model) = read_str(frontmatter, "model") {
        let normalized = if model.trim().eq_ignore_ascii_case("inherit") {
            "inherit".to_owned()
        } else {
            model.trim().to_owned()
        };
        def.model = Some(normalized);
    }
    if let Some(raw) = read_str_aliased(frontmatter, &["modelRole", "model_role"]) {
        match raw.parse::<ModelRole>() {
            Ok(role) => def.model_role = Some(role),
            Err(_) => warnings.push(ValidationError::InvalidModelRole { value: raw }),
        }
    }
    if let Some(effort) = read_str(frontmatter, "effort") {
        match effort.trim().parse::<ReasoningEffort>() {
            Ok(e) => def.effort = Some(e),
            Err(_) => warnings.push(ValidationError::InvalidFrontmatter {
                message: format!(
                    "effort: unrecognized value `{effort}` (expected one of \
                     off/auto/minimal/low/medium/high/xhigh, or alias `max`)"
                ),
            }),
        }
    } else if read_int(frontmatter, "effort").is_some() {
        // The downstream consumer (`session_runtime::thinking_level_for_effort_from`)
        // takes a `ReasoningEffort` enum directly — there is no consumer
        // for numeric input, so accepting it would silently drop
        // the operator's intent. Reject loudly and point at the
        // proper config surface.
        warnings.push(ValidationError::InvalidFrontmatter {
            message: "effort: numeric form is not accepted — `effort:` is a lookup key into \
                      the model's `supported_thinking_levels`, not a budget number. Use one \
                      of off/auto/minimal/low/medium/high/xhigh (or `max`). For a custom \
                      budget, configure `settings.models.<role>.thinking_level.budget_tokens`."
                .into(),
        });
    }
    if let Some(initial) = read_str_aliased(frontmatter, &["initialPrompt", "initial_prompt"]) {
        def.initial_prompt = Some(initial);
    }
    if let Some(reminder) = read_str_aliased(
        frontmatter,
        &[
            "criticalSystemReminder",
            "criticalSystemReminder_EXPERIMENTAL",
            "critical_system_reminder",
        ],
    ) {
        def.critical_system_reminder = Some(reminder);
    }
    if let Some(identity) = read_str(frontmatter, "identity") {
        def.identity = Some(identity);
    }

    if let Some(value) = read_str_aliased(frontmatter, &["permissionMode", "permission_mode"]) {
        if VALID_PERMISSION_MODES.contains(&value.as_str()) {
            def.permission_mode = Some(value);
        } else {
            warnings.push(ValidationError::InvalidPermissionMode { value });
        }
    }

    if let Some(raw) = read_str(frontmatter, "color")
        && let Some(color) = parse_color_value(&raw, &mut warnings)
    {
        def.color = Some(color);
    }

    if let Some(raw) = read_str(frontmatter, "isolation") {
        match parse_isolation_value(&raw) {
            Ok(value) => def.isolation = value,
            Err(err) => warnings.push(err),
        }
    }

    if let Some(raw) = read_str(frontmatter, "memory") {
        match parse_memory_value(&raw) {
            Ok(value) => def.memory_scope = Some(value),
            Err(err) => warnings.push(err),
        }
    }

    if let Some(turns) = read_int_aliased(frontmatter, &["maxTurns", "max_turns"]) {
        if let Ok(n) = i32::try_from(turns)
            && n > 0
        {
            def.max_turns = Some(n);
        } else {
            warnings.push(ValidationError::InvalidMaxTurns {
                value: turns.to_string(),
            });
        }
    }

    if let Some(bg) = read_bool(frontmatter, "background") {
        def.background = bg;
    }
    if let Some(omit) = read_bool_aliased(frontmatter, &["omitClaudeMd", "omit_claude_md"]) {
        def.omit_claude_md = omit;
    }
    if let Some(exact) = read_bool_aliased(frontmatter, &["useExactTools", "use_exact_tools"]) {
        def.use_exact_tools = exact;
    }

    def.allowed_tools = read_csv_or_list_aliased(frontmatter, &["tools", "allowed_tools"])
        .map(coco_types::ToolAllowList::from_frontmatter)
        .unwrap_or_default();
    def.disallowed_tools =
        read_csv_or_list_aliased(frontmatter, &["disallowedTools", "disallowed_tools"])
            .unwrap_or_default();
    def.skills = read_csv_or_list(frontmatter, "skills").unwrap_or_default();
    def.mcp_servers = parse_mcp_servers(frontmatter);
    // NOTE: `required_mcp_servers` is intentionally NOT read from
    // frontmatter — only `mcpServers` is read; `requiredMcpServers` is
    // set programmatically, never from a markdown field. The struct
    // field stays for programmatic use.
    // Hooks parsing: nested `hooks:` mapping flows through verbatim
    // as `serde_json::Value`. `coco_hooks::load_hooks_from_config`
    // consumes it at SubagentStart time. Validation is deferred until
    // the hook loader actually parses the value (errors surface as
    // tracing::warn).
    def.hooks = frontmatter
        .get("hooks")
        .map(FrontmatterValue::to_json)
        .unwrap_or(serde_json::Value::Null);

    Ok((def, warnings))
}

/// Parse `mcpServers:` from frontmatter into `Vec<AgentMcpServerSpec>`.
/// Handles three shapes:
/// - String list (string-ref form): `mcpServers: [github, slack]`
/// - Mixed sequence (string-ref + inline): `mcpServers: [github, {slack: {...}}]`
/// - Pure inline mapping list: `mcpServers: [{slack: {command: ./mcp}}]`
fn parse_mcp_servers(
    frontmatter: &std::collections::HashMap<String, FrontmatterValue>,
) -> Vec<AgentMcpServerSpec> {
    let raw = match frontmatter
        .get("mcpServers")
        .or_else(|| frontmatter.get("mcp_servers"))
    {
        Some(v) => v,
        None => return Vec::new(),
    };

    // Pure-string CSV form (single string with comma separators):
    // mcpServers: github,slack
    if let Some(s) = raw.as_str() {
        return s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| AgentMcpServerSpec::Name(s.to_string()))
            .collect();
    }

    let Some(items) = raw.as_sequence() else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| match item {
            FrontmatterValue::String(s) if !s.trim().is_empty() => {
                Some(AgentMcpServerSpec::Name(s.clone()))
            }
            FrontmatterValue::Mapping(m) if m.len() == 1 => {
                // Multiple keys per inline entry is rejected as malformed.
                let (name, config) = m.iter().next()?;
                let mut single = std::collections::BTreeMap::new();
                single.insert(name.clone(), config.to_json());
                Some(AgentMcpServerSpec::Inline(single))
            }
            _ => None,
        })
        .collect()
}

/// Validate a color string against the `AgentColorName` set. Invalid
/// values produce a warning and the color is dropped.
pub fn parse_color_value(raw: &str, warnings: &mut Vec<ValidationError>) -> Option<AgentColorName> {
    match raw.parse::<AgentColorName>() {
        Ok(c) => Some(c),
        Err(_) => {
            warnings.push(ValidationError::InvalidColor { value: raw.into() });
            None
        }
    }
}

pub fn parse_isolation_value(raw: &str) -> Result<AgentIsolation, ValidationError> {
    raw.parse::<AgentIsolation>()
        .map_err(|_| ValidationError::InvalidIsolation { value: raw.into() })
}

pub fn parse_memory_value(raw: &str) -> Result<MemoryScope, ValidationError> {
    raw.parse::<MemoryScope>()
        .map_err(|_| ValidationError::InvalidMemoryScope { value: raw.into() })
}

fn read_str(
    map: &std::collections::HashMap<String, FrontmatterValue>,
    key: &str,
) -> Option<String> {
    map.get(key).and_then(|v| v.as_str().map(str::to_owned))
}

fn read_int(map: &std::collections::HashMap<String, FrontmatterValue>, key: &str) -> Option<i64> {
    match map.get(key)? {
        FrontmatterValue::Int(n) => Some(*n),
        FrontmatterValue::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn read_bool(map: &std::collections::HashMap<String, FrontmatterValue>, key: &str) -> Option<bool> {
    map.get(key)
        .and_then(coco_frontmatter::FrontmatterValue::as_bool)
}

/// Try each key in order; return the first hit. Used for fields that accept
/// both camelCase and snake_case keys.
fn read_str_aliased(
    map: &std::collections::HashMap<String, FrontmatterValue>,
    keys: &[&str],
) -> Option<String> {
    keys.iter().find_map(|k| read_str(map, k))
}

fn read_int_aliased(
    map: &std::collections::HashMap<String, FrontmatterValue>,
    keys: &[&str],
) -> Option<i64> {
    keys.iter().find_map(|k| read_int(map, k))
}

fn read_bool_aliased(
    map: &std::collections::HashMap<String, FrontmatterValue>,
    keys: &[&str],
) -> Option<bool> {
    keys.iter().find_map(|k| read_bool(map, k))
}

fn read_csv_or_list_aliased(
    map: &std::collections::HashMap<String, FrontmatterValue>,
    keys: &[&str],
) -> Option<Vec<String>> {
    keys.iter().find_map(|k| read_csv_or_list(map, k))
}

// `validate_effort` deleted — parsing happens inline via
// `ReasoningEffort::from_str`. The numeric branch was dead code: the
// downstream consumer (`thinking_level_for_effort_from`) takes the
// enum, so numeric input was silently dropped.

/// Accepted `permissionMode` values; unrecognized values surface as a warning.
const VALID_PERMISSION_MODES: &[&str] = &[
    "default",
    "plan",
    "dontAsk",
    "acceptEdits",
    "bubble",
    "bypassPermissions",
    "auto",
    "ask",
    "deny",
];

/// Read a frontmatter value that may be either:
/// - a YAML list (`tools:\n  - Read\n  - Edit`), or
/// - a single comma-separated string (`tools: Read, Edit`).
///
/// The flat-string form is the most common idiom in user-authored agent files.
fn read_csv_or_list(
    map: &std::collections::HashMap<String, FrontmatterValue>,
    key: &str,
) -> Option<Vec<String>> {
    let raw = map.get(key)?.as_string_list()?;
    let mut out = Vec::with_capacity(raw.len());
    for entry in raw {
        // If a list element happens to contain commas, split it; otherwise
        // keep the trimmed value verbatim. Idempotent for clean YAML lists.
        if entry.contains(',') {
            out.extend(
                entry
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            );
        } else {
            let trimmed = entry.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_owned());
            }
        }
    }
    Some(out)
}

// Helper: AgentTypeId parse is infallible, but the FromStr `Err` is `Infallible`
// which is awkward at call sites. This wraps it.
trait AgentTypeIdExt {
    fn from_str_infallible(s: &str) -> Self;
}

impl AgentTypeIdExt for AgentTypeId {
    fn from_str_infallible(s: &str) -> Self {
        s.parse().unwrap_or_else(|_| AgentTypeId::Custom(s.into()))
    }
}
