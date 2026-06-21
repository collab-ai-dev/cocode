//! Settings validation — check for invalid combinations, validate permission
//! rules, and verify known fields.

use super::PermissionsConfig;
use super::Settings;
use crate::model_allowlist::is_model_allowed;
use coco_types::MCP_TOOL_PREFIX;
use coco_types::PermissionMode;
use coco_types::PermissionRuleValue;
use coco_types::ToolName;

// ── Validation errors ──

/// A single validation error found in settings.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// Relative file path where the error was found (if known).
    pub file: Option<String>,
    /// Dot-notation field path, e.g. `"permissions.defaultMode"`.
    pub path: String,
    /// Human-readable error message.
    pub message: String,
    /// Expected value or type.
    pub expected: Option<String>,
    /// The invalid value that was provided (serialized).
    pub invalid_value: Option<String>,
    /// Suggestion for fixing the error.
    pub suggestion: Option<String>,
}

/// Settings snapshot with associated validation errors.
#[derive(Debug, Clone)]
pub struct SettingsWithErrors {
    pub settings: Settings,
    pub errors: Vec<ValidationError>,
}

// ── Top-level validation ──

/// Validate the merged settings for invalid combinations and constraint violations.
///
/// Returns a list of validation errors. An empty vec means the settings are valid.
pub fn validate_settings(settings: &Settings) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // Validate permission rules
    errors.extend(validate_permission_rules(&settings.permissions));

    // Validate mode combinations
    if let Some(mode) = settings.permissions.default_mode
        && settings.permissions.disable_bypass_mode
        && matches!(
            mode,
            PermissionMode::BypassPermissions | PermissionMode::DontAsk
        )
    {
        errors.push(ValidationError {
            file: None,
            path: "permissions.default_mode".into(),
            message: format!("Default mode '{mode:?}' conflicts with disable_bypass_mode=true"),
            expected: Some("A non-bypass mode (Default, Plan, AcceptEdits, Auto)".into()),
            invalid_value: Some(format!("{mode:?}")),
            suggestion: Some(
                "Either set disable_bypass_mode to false or choose a different default mode".into(),
            ),
        });
    }

    // Validate model allowlist vs selected Main model
    if let (Some(main), Some(available)) = (&settings.models.main, &settings.available_models)
        && !is_model_allowed(&main.primary.model_id, Some(available.as_slice()))
    {
        let selected = &main.primary.model_id;
        errors.push(ValidationError {
            file: None,
            path: "models.main".into(),
            message: format!(
                "Selected Main model '{selected}' is not in the available_models allowlist"
            ),
            expected: Some(format!("One of: {}", available.join(", "))),
            invalid_value: Some(selected.clone()),
            suggestion: Some(
                "Either add the Main model to available_models or choose an available model".into(),
            ),
        });
    }

    // Validate auto-mode config fields are non-empty strings
    if let Some(auto_mode) = &settings.auto_mode {
        for (field, values) in [
            ("allow", &auto_mode.allow),
            ("soft_deny", &auto_mode.soft_deny),
            ("environment", &auto_mode.environment),
        ] {
            for (i, v) in values.iter().enumerate() {
                if v.trim().is_empty() {
                    errors.push(ValidationError {
                        file: None,
                        path: format!("auto_mode.{field}[{i}]"),
                        message: "Empty string in auto_mode rule list".into(),
                        expected: Some("Non-empty rule string".into()),
                        invalid_value: Some(v.clone()),
                        suggestion: Some(
                            "Remove the empty entry or provide a valid rule pattern".into(),
                        ),
                    });
                }
            }
        }
    }

    // Validate hooks
    errors.extend(validate_hooks(settings));

    // Validate MCP configs
    errors.extend(validate_mcp_configs(settings));

    // Validate provider configs — flag plaintext api_key leaks.
    errors.extend(validate_providers(settings));

    errors
}

/// Validate provider configurations for safe secret handling.
///
/// Flags any provider entry that stores `api_key` in plaintext in
/// settings.json. Secrets should live in the provider's `env_key` env
/// var, not in the config file. The partial overlay shape carries
/// `Option<RedactedSecret>` for `api_key`; presence here means the
/// user wrote one in JSON.
pub fn validate_providers(settings: &Settings) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    for (name, partial) in &settings.providers {
        if let Some(key) = &partial.api_key
            && !key.is_empty()
        {
            let env_key = partial.env_key.as_deref().unwrap_or("");
            // A provider with no configured `env_key` would otherwise render
            // an empty-backtick placeholder ("move it to the `` environment
            // variable"). Fall back to a descriptive token in BOTH the message
            // and the suggestion so neither emits the malformed form.
            let env_key_display = if env_key.is_empty() {
                "<provider env_key>"
            } else {
                env_key
            };
            errors.push(ValidationError {
                file: None,
                path: format!("providers.{name}.api_key"),
                message: format!(
                    "Provider '{name}' stores api_key in plaintext — \
                     move it to the `{env_key_display}` environment variable instead."
                ),
                expected: Some("api_key unset; use env var".into()),
                invalid_value: Some("<redacted>".into()),
                suggestion: Some(format!(
                    "Unset `providers.{name}.api_key` and export the key \
                     via `{env_key_display}=...` in your shell."
                )),
            });
        }
    }
    errors
}

// ── Permission rule validation ──

/// Validate the permissions configuration: check rule syntax, detect conflicts.
///
/// Rules are stored as strings matching TS on-disk format: `"Bash"`, `"Bash(git *)"`.
pub fn validate_permission_rules(config: &PermissionsConfig) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    for (category, rules) in [
        ("allow", &config.allow),
        ("deny", &config.deny),
        ("ask", &config.ask),
    ] {
        for (i, rule_str) in rules.iter().enumerate() {
            let path = format!("permissions.{category}[{i}]");

            if let Err(msg) = validate_permission_rule_string(rule_str) {
                errors.push(ValidationError {
                    file: None,
                    path,
                    message: msg,
                    expected: Some("A rule like 'Bash', 'Bash(git *)', or 'mcp__server'".into()),
                    invalid_value: Some(rule_str.clone()),
                    suggestion: None,
                });
            }
        }
    }

    // Detect allow+deny conflicts on the same tool
    for allow_str in &config.allow {
        let allow_parsed = parse_rule_value_from_string(allow_str);
        for deny_str in &config.deny {
            let deny_parsed = parse_rule_value_from_string(deny_str);
            if allow_parsed.tool_pattern == deny_parsed.tool_pattern
                && allow_parsed.rule_content == deny_parsed.rule_content
            {
                errors.push(ValidationError {
                    file: None,
                    path: "permissions".into(),
                    message: format!(
                        "Rule '{allow_str}' appears in both allow and deny lists — deny will win"
                    ),
                    expected: None,
                    invalid_value: None,
                    suggestion: Some("Remove the rule from one list to clarify intent".into()),
                });
            }
        }
    }

    errors
}

/// Parse a rule string into a PermissionRuleValue (tool_pattern + optional content).
///
/// Format: `"ToolName"` or `"ToolName(content)"`.
fn parse_rule_value_from_string(s: &str) -> PermissionRuleValue {
    if let Some(paren_pos) = s.find('(')
        && let Some(end) = s.rfind(')')
    {
        return PermissionRuleValue {
            tool_pattern: s[..paren_pos].to_string(),
            rule_content: Some(s[paren_pos + 1..end].to_string()),
        };
    }
    PermissionRuleValue {
        tool_pattern: s.to_string(),
        rule_content: None,
    }
}

// ── MCP config validation ──

/// Known hook event types — synced 1:1 with `HookEventType` (coco-types)
/// and TS `HOOK_EVENTS` (`coreSchemas.ts:355-383`).
const KNOWN_HOOK_EVENTS: &[&str] = &[
    // Tool lifecycle
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    // Session lifecycle
    "SessionStart",
    "SessionEnd",
    "Setup",
    "Stop",
    "StopFailure",
    // Subagent lifecycle
    "SubagentStart",
    "SubagentStop",
    // User interaction
    "UserPromptSubmit",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "Elicitation",
    "ElicitationResult",
    // Compaction
    "PreCompact",
    "PostCompact",
    // Task lifecycle
    "TeammateIdle",
    "TaskCreated",
    "TaskCompleted",
    // Config & environment
    "ConfigChange",
    "InstructionsLoaded",
    "CwdChanged",
    "FileChanged",
    // Worktree
    "WorktreeCreate",
    "WorktreeRemove",
];

/// Validate MCP server configurations for structural correctness.
///
/// Checks:
/// - Server names are non-empty
/// - No duplicate allowed server names
/// - No server appears in both allowed and denied lists
/// - Config values (if present) are valid JSON objects
pub fn validate_mcp_configs(settings: &Settings) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // Check for empty server names in allowed list
    for (i, entry) in settings.allowed_mcp_servers.iter().enumerate() {
        if entry.name.trim().is_empty() {
            errors.push(ValidationError {
                file: None,
                path: format!("allowed_mcp_servers[{i}].name"),
                message: "MCP server name cannot be empty".into(),
                expected: Some("Non-empty server name".into()),
                invalid_value: Some(entry.name.clone()),
                suggestion: None,
            });
        }

        // Validate config is an object if present
        if let Some(config) = &entry.config
            && !config.is_object()
        {
            errors.push(ValidationError {
                file: None,
                path: format!("allowed_mcp_servers[{i}].config"),
                message: "MCP server config must be a JSON object".into(),
                expected: Some("object".into()),
                invalid_value: Some(config.to_string()),
                suggestion: None,
            });
        }
    }

    // Check for empty server names in denied list
    for (i, entry) in settings.denied_mcp_servers.iter().enumerate() {
        if entry.name.trim().is_empty() {
            errors.push(ValidationError {
                file: None,
                path: format!("denied_mcp_servers[{i}].name"),
                message: "MCP server name cannot be empty".into(),
                expected: Some("Non-empty server name".into()),
                invalid_value: Some(entry.name.clone()),
                suggestion: None,
            });
        }
    }

    // Check for duplicate allowed server names
    let mut seen_allowed = std::collections::HashSet::new();
    for entry in &settings.allowed_mcp_servers {
        let name_lower = entry.name.to_lowercase();
        if !seen_allowed.insert(name_lower.clone()) {
            errors.push(ValidationError {
                file: None,
                path: "allowed_mcp_servers".into(),
                message: format!(
                    "Duplicate MCP server name '{}' in allowed_mcp_servers",
                    entry.name
                ),
                expected: None,
                invalid_value: Some(entry.name.clone()),
                suggestion: Some("Remove the duplicate entry".into()),
            });
        }
    }

    // Check for servers in both allowed and denied lists
    let denied_names: std::collections::HashSet<String> = settings
        .denied_mcp_servers
        .iter()
        .map(|e| e.name.to_lowercase())
        .collect();
    for entry in &settings.allowed_mcp_servers {
        if denied_names.contains(&entry.name.to_lowercase()) {
            errors.push(ValidationError {
                file: None,
                path: "allowed_mcp_servers".into(),
                message: format!(
                    "MCP server '{}' appears in both allowed and denied lists — denied wins",
                    entry.name
                ),
                expected: None,
                invalid_value: Some(entry.name.clone()),
                suggestion: Some("Remove the server from one list to clarify intent".into()),
            });
        }
    }

    errors
}

// ── Hooks validation ──

/// Validate hook definitions for structural correctness.
///
/// Checks:
/// - hooks is a JSON object (not array/string/etc.)
/// - Each key is a known hook event type
/// - Each value is a valid hook definition (object or array of objects)
/// - Hook definitions have required fields (command, type)
pub fn validate_hooks(settings: &Settings) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    let hooks = match &settings.hooks {
        Some(h) => h,
        None => return errors,
    };

    // Must be an object
    let hooks_obj = match hooks.as_object() {
        Some(obj) => obj,
        None => {
            errors.push(ValidationError {
                file: None,
                path: "hooks".into(),
                message: "Hooks must be an object mapping event types to hook definitions".into(),
                expected: Some("object".into()),
                invalid_value: Some(hooks.to_string()),
                suggestion: None,
            });
            return errors;
        }
    };

    for (event_type, definition) in hooks_obj {
        let path = format!("hooks.{event_type}");

        // Check event type is known
        if !KNOWN_HOOK_EVENTS.contains(&event_type.as_str()) {
            errors.push(ValidationError {
                file: None,
                path: path.clone(),
                message: format!("Unknown hook event type '{event_type}'"),
                expected: Some(format!("One of: {}", KNOWN_HOOK_EVENTS.join(", "))),
                invalid_value: Some(event_type.clone()),
                suggestion: None,
            });
            continue;
        }

        // Definition can be a single hook object or an array of hook objects
        let hook_defs: Vec<&serde_json::Value> = if let Some(arr) = definition.as_array() {
            arr.iter().collect()
        } else if definition.is_object() {
            vec![definition]
        } else {
            errors.push(ValidationError {
                file: None,
                path,
                message: format!(
                    "Hook definition for '{event_type}' must be an object or array of objects"
                ),
                expected: Some("object or array of objects".into()),
                invalid_value: Some(definition.to_string()),
                suggestion: None,
            });
            continue;
        };

        for (i, hook_def) in hook_defs.iter().enumerate() {
            let def_path = if hook_defs.len() > 1 {
                format!("hooks.{event_type}[{i}]")
            } else {
                format!("hooks.{event_type}")
            };

            let obj = match hook_def.as_object() {
                Some(o) => o,
                None => {
                    errors.push(ValidationError {
                        file: None,
                        path: def_path,
                        message: "Hook definition must be an object".into(),
                        expected: Some("object with 'type' and 'command' fields".into()),
                        invalid_value: Some(hook_def.to_string()),
                        suggestion: None,
                    });
                    continue;
                }
            };

            // Validate required 'type' field
            match obj.get("type").and_then(|t| t.as_str()) {
                Some(hook_type) => {
                    let valid_types = ["command", "prompt", "agent", "webhook", "http"];
                    if !valid_types.contains(&hook_type) {
                        errors.push(ValidationError {
                            file: None,
                            path: format!("{def_path}.type"),
                            message: format!("Unknown hook type '{hook_type}'"),
                            expected: Some(format!("One of: {}", valid_types.join(", "))),
                            invalid_value: Some(hook_type.to_string()),
                            suggestion: None,
                        });
                    }
                }
                None => {
                    errors.push(ValidationError {
                        file: None,
                        path: format!("{def_path}.type"),
                        message: "Hook definition is missing required 'type' field".into(),
                        expected: Some("command, prompt, agent, webhook, or http".into()),
                        invalid_value: None,
                        suggestion: None,
                    });
                }
            }

            // For 'command' type hooks, validate 'command' field exists
            if obj
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t == "command")
                && obj.get("command").is_none()
            {
                errors.push(ValidationError {
                    file: None,
                    path: format!("{def_path}.command"),
                    message: "Command-type hook is missing required 'command' field".into(),
                    expected: Some("string or array of strings".into()),
                    invalid_value: None,
                    suggestion: None,
                });
            }

            // For 'webhook' type hooks, validate 'url' field exists
            if obj
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t == "webhook")
                && obj.get("url").is_none()
            {
                errors.push(ValidationError {
                    file: None,
                    path: format!("{def_path}.url"),
                    message: "Webhook-type hook is missing required 'url' field".into(),
                    expected: Some("URL string".into()),
                    invalid_value: None,
                    suggestion: None,
                });
            }

            // Validate matcher if present (accepts string or object)
            if let Some(matcher) = obj.get("matcher")
                && !matcher.is_string()
                && !matcher.is_object()
            {
                errors.push(ValidationError {
                    file: None,
                    path: format!("{def_path}.matcher"),
                    message: "Hook matcher must be a string or object".into(),
                    expected: Some("string pattern or object with 'tool_name' field".into()),
                    invalid_value: Some(matcher.to_string()),
                    suggestion: None,
                });
            }

            // Validate 'http' type hooks also require 'url' field
            if obj
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t == "http")
                && obj.get("url").is_none()
            {
                errors.push(ValidationError {
                    file: None,
                    path: format!("{def_path}.url"),
                    message: "HTTP-type hook is missing required 'url' field".into(),
                    expected: Some("URL string".into()),
                    invalid_value: None,
                    suggestion: None,
                });
            }
        }
    }

    errors
}

// ── Known fields ──

/// All known top-level setting field names.
const KNOWN_SETTINGS_FIELDS: &[&str] = &[
    "api_key_helper",
    "permissions",
    "models",
    "available_models",
    "thinking_level",
    "fast_mode",
    "providers",
    "env",
    "log",
    "api",
    "loop",
    "tool",
    "shell",
    "sandbox",
    "memory",
    "mcp",
    "web_fetch",
    "web_search",
    "diagnostics",
    "lsp",
    "paths",
    "agent_teams",
    "compact",
    "prompt_cache",
    "account",
    "features",
    "hooks",
    "disable_all_hooks",
    "allowed_mcp_servers",
    "denied_mcp_servers",
    "enable_all_project_mcp_servers",
    "output_style",
    "language",
    crate::settings::SYNTAX_HIGHLIGHTING_DISABLED_KEY,
    crate::settings::SHOW_THINKING_KEY,
    crate::settings::COPY_FULL_RESPONSE_KEY,
    "statusLine",
    "status_line",
    "tui",
    "enabled_plugins",
    "skill_overrides",
    "worktree",
    "plans_directory",
    "plan_mode",
    "session",
    "auto_mode",
    "include_co_authored_by",
    "include_git_instructions",
    "allow_managed_hooks_only",
    "strict_plugin_only_customization",
    "strict_known_marketplaces",
    "blocked_marketplaces",
    "file_checkpointing_enabled",
];

/// Check if a setting field name is a known field.
pub fn is_setting_supported(field: &str) -> bool {
    KNOWN_SETTINGS_FIELDS.contains(&field)
}

/// Filter raw JSON data to remove invalid permission rules, returning warnings.
///
/// Prevents one bad rule from poisoning the entire settings file.
pub fn filter_invalid_permission_rules(
    data: &mut serde_json::Value,
    file_path: &str,
) -> Vec<ValidationError> {
    let mut warnings = Vec::new();

    let permissions = match data.get_mut("permissions") {
        Some(serde_json::Value::Object(obj)) => obj,
        _ => return warnings,
    };

    for category in ["allow", "deny", "ask"] {
        let rules = match permissions.get_mut(category) {
            Some(serde_json::Value::Array(arr)) => arr,
            _ => continue,
        };

        let mut i = 0;
        while i < rules.len() {
            match &rules[i] {
                serde_json::Value::String(s) => {
                    if let Err(err) = validate_permission_rule_string(s) {
                        warnings.push(ValidationError {
                            file: Some(file_path.to_string()),
                            path: format!("permissions.{category}"),
                            message: format!("Invalid permission rule \"{s}\" was skipped: {err}"),
                            expected: None,
                            invalid_value: Some(s.clone()),
                            suggestion: None,
                        });
                        rules.remove(i);
                        continue;
                    }
                }
                other => {
                    warnings.push(ValidationError {
                        file: Some(file_path.to_string()),
                        path: format!("permissions.{category}"),
                        message: format!("Non-string value in {category} array was removed"),
                        expected: Some("string".into()),
                        invalid_value: Some(other.to_string()),
                        suggestion: None,
                    });
                    rules.remove(i);
                    continue;
                }
            }
            i += 1;
        }
    }

    warnings
}

/// Validate a single permission rule string.
///
/// Returns `Ok(())` if valid, `Err(reason)` if invalid.
pub fn validate_permission_rule_string(rule: &str) -> Result<(), String> {
    if rule.is_empty() || rule.trim().is_empty() {
        return Err("Permission rule cannot be empty".into());
    }

    // Check parentheses balance
    let open_count = count_unescaped(rule, '(');
    let close_count = count_unescaped(rule, ')');
    if open_count != close_count {
        return Err("Mismatched parentheses".into());
    }

    // Check for empty parentheses
    if has_unescaped_empty_parens(rule) {
        let tool_name = rule.split('(').next().unwrap_or("");
        if tool_name.is_empty() {
            return Err("Empty parentheses with no tool name".into());
        }
        return Err(format!(
            "Empty parentheses — use just '{tool_name}' without parentheses or add a pattern"
        ));
    }

    // Extract tool name
    let tool_name = if let Some(paren_pos) = rule.find('(') {
        &rule[..paren_pos]
    } else {
        rule
    };

    if tool_name.is_empty() {
        return Err("Tool name cannot be empty".into());
    }

    // MCP rules cannot have parenthesized content
    if tool_name.starts_with(MCP_TOOL_PREFIX) && rule.contains('(') {
        return Err("MCP rules do not support patterns in parentheses".into());
    }

    // Non-MCP, non-wildcard tool names should start with uppercase
    if !tool_name.starts_with(MCP_TOOL_PREFIX)
        && !tool_name.starts_with('*')
        && let Some(first) = tool_name.chars().next()
        && first.is_ascii_lowercase()
    {
        return Err(format!(
            "Tool names must start with uppercase — use '{}{}'",
            first.to_uppercase(),
            &tool_name[1..]
        ));
    }

    // Bash/PowerShell :* must be at end
    if (tool_name == ToolName::Bash.as_str() || tool_name == ToolName::PowerShell.as_str())
        && let Some(content_start) = rule.find('(')
        && let Some(content_end) = rule.rfind(')')
    {
        let content = &rule[content_start + 1..content_end];
        if content.contains(":*") && !content.ends_with(":*") {
            return Err("The :* pattern must be at the end".into());
        }
        if content == ":*" {
            return Err("Prefix cannot be empty before :*".into());
        }
    }

    Ok(())
}

// ── Helpers ──

/// Count unescaped occurrences of `ch` in `s`.
fn count_unescaped(s: &str, ch: char) -> usize {
    let bytes = s.as_bytes();
    let mut count = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == ch as u8 && !is_escaped(bytes, i) {
            count += 1;
        }
    }
    count
}

/// Check if position `i` in `bytes` is preceded by an odd number of backslashes.
fn is_escaped(bytes: &[u8], i: usize) -> bool {
    let mut backslash_count = 0;
    let mut j = i;
    while j > 0 {
        j -= 1;
        if bytes[j] == b'\\' {
            backslash_count += 1;
        } else {
            break;
        }
    }
    backslash_count % 2 != 0
}

/// Check if `s` contains unescaped adjacent `()`.
fn has_unescaped_empty_parens(s: &str) -> bool {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b'(' && bytes[i + 1] == b')' && !is_escaped(bytes, i) {
            return true;
        }
    }
    false
}

#[cfg(test)]
#[path = "validation.test.rs"]
mod tests;
