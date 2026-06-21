//! `/permissions` — show and modify tool permission rules.
//!
//! Reads permission rules from project and user settings files,
//! displays them by priority source, and supports add/remove/reset.

use coco_types::MCP_TOOL_PREFIX;
use coco_types::ToolName;
use std::path::Path;
use std::pin::Pin;

/// Priority order for permission rule sources (highest to lowest).
const RULE_SOURCES: &[(&str, &str)] = &[
    ("Session", "(set during this session via /permissions)"),
    ("Command", "(from --allow-tool / --deny-tool flags)"),
    ("CLI", "(from command-line arguments)"),
    ("Flag", "(from feature flags)"),
    ("Local", ".coco/settings.local.json"),
    ("Project", ".coco/settings.json"),
    ("Policy", "(organization policy)"),
    ("User", "~/.coco/settings.json"),
];

/// Async handler for `/permissions [allow|deny|reset|list]`.
pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let subcommand = args.trim().to_string();

        if subcommand.is_empty() || subcommand == "list" {
            return list_permissions().await;
        }

        if let Some(tool) = subcommand.strip_prefix("allow ") {
            return add_permission_rule("allow", tool.trim()).await;
        }

        if let Some(tool) = subcommand.strip_prefix("deny ") {
            return add_permission_rule("deny", tool.trim()).await;
        }

        if subcommand == "reset" {
            // The TUI dispatcher (`tui_runner::dispatch_permissions_mutation`)
            // intercepts this branch and mutates `engine_config.allow_rules /
            // deny_rules` directly. This handler runs only for non-TUI
            // contexts (SDK, tests) where the engine config isn't reachable
            // — be honest about that.
            return Ok(
                "Session permission rules: reset is only effective inside the TUI \
                 (it mutates the live engine config). File-based rules \
                 (.coco/settings.json, ~/.coco/settings.json) are unchanged — \
                 edit those files directly to modify persistent rules."
                    .to_string(),
            );
        }

        Ok(format!(
            "Unknown permissions subcommand: {subcommand}\n\n\
             Usage:\n\
             /permissions              Show all rules\n\
             /permissions allow <tool> Add an allow rule for this session\n\
             /permissions deny <tool>  Add a deny rule for this session\n\
             /permissions reset        Clear session rules"
        ))
    })
}

/// Gather and display permission rules from all sources.
async fn list_permissions() -> crate::Result<String> {
    let mut out = String::from("## Permission Rules\n\n");

    out.push_str("Rule sources (highest to lowest priority):\n\n");
    for (i, (name, desc)) in RULE_SOURCES.iter().enumerate() {
        out.push_str(&format!("  {}. {:<10} {desc}\n", i + 1, name));
    }
    out.push('\n');

    // Read project settings
    let project_rules = read_permission_rules(".coco/settings.json").await;
    let local_rules = read_permission_rules(".coco/settings.local.json").await;
    let user_rules =
        read_permission_rules_from_path(&coco_config::global_config::user_settings_path()).await;

    let has_any = !project_rules.is_empty() || !local_rules.is_empty() || !user_rules.is_empty();

    if has_any {
        out.push_str("### Active Rules\n\n");

        if !project_rules.is_empty() {
            out.push_str("**Project** (.coco/settings.json):\n");
            for rule in &project_rules {
                out.push_str(&format!("  {rule}\n"));
            }
            out.push('\n');
        }

        if !local_rules.is_empty() {
            out.push_str("**Local** (.coco/settings.local.json):\n");
            for rule in &local_rules {
                out.push_str(&format!("  {rule}\n"));
            }
            out.push('\n');
        }

        if !user_rules.is_empty() {
            out.push_str("**User** (~/.coco/settings.json):\n");
            for rule in &user_rules {
                out.push_str(&format!("  {rule}\n"));
            }
            out.push('\n');
        }
    } else {
        out.push_str("No permission rules configured.\n\n");
    }

    out.push_str("Commands:\n");
    out.push_str("  /permissions allow <tool>  Allow a tool for this session\n");
    out.push_str("  /permissions deny <tool>   Deny a tool for this session\n");
    out.push_str("  /permissions reset         Clear session rules");

    Ok(out)
}

/// Stub for non-TUI contexts. The TUI dispatcher
/// (`tui_runner::dispatch_permissions_mutation`) handles `allow` / `deny`
/// before this handler runs and mutates `engine_config` directly. Outside
/// the TUI we surface a hint pointing at file-based rules.
async fn add_permission_rule(action: &str, tool: &str) -> crate::Result<String> {
    if tool.is_empty() {
        return Ok(format!("Usage: /permissions {action} <tool-name>"));
    }

    let valid_tools: &[&str] = &[
        ToolName::Bash.as_str(),
        ToolName::Read.as_str(),
        ToolName::Write.as_str(),
        ToolName::Edit.as_str(),
        ToolName::Glob.as_str(),
        ToolName::Grep.as_str(),
        ToolName::WebFetch.as_str(),
        ToolName::WebSearch.as_str(),
        ToolName::NotebookEdit.as_str(),
        ToolName::TodoWrite.as_str(),
        MCP_TOOL_PREFIX,
    ];
    let is_known = valid_tools.contains(&tool) || tool.starts_with(MCP_TOOL_PREFIX);

    let mut out = format!(
        "/permissions {action} {tool}: only effective inside the TUI (mutates the live \
         engine config there). For SDK / non-interactive contexts, add the rule to \
         settings.json directly.\n"
    );

    if !is_known {
        out.push_str(&format!(
            "\n  Warning: '{tool}' is not a known built-in tool.\n\
             Known tools: {}\n\
             MCP tools use prefix: mcp__<server>__<tool>",
            valid_tools[..6].join(", "),
        ));
    }

    Ok(out)
}

/// Read permission rules from a settings JSON file (relative path).
async fn read_permission_rules(path: &str) -> Vec<String> {
    read_permission_rules_from_path(Path::new(path)).await
}

/// Read permission rules from an absolute or relative path.
async fn read_permission_rules_from_path(path: &Path) -> Vec<String> {
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return Vec::new();
    };

    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Vec::new();
    };

    let mut rules = Vec::new();

    // Check "permissions" object
    if let Some(perms) = parsed.get("permissions").and_then(|v| v.as_object()) {
        for (key, value) in perms {
            if let Some(arr) = value.as_array() {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        rules.push(format!("{key}: {s}"));
                    }
                }
            } else if let Some(s) = value.as_str() {
                rules.push(format!("{key}: {s}"));
            }
        }
    }

    // Check "allowedTools" array
    if let Some(allowed) = parsed.get("allowedTools").and_then(|v| v.as_array()) {
        for item in allowed {
            if let Some(s) = item.as_str() {
                rules.push(format!("allow: {s}"));
            }
        }
    }

    // Check "deniedTools" array
    if let Some(denied) = parsed.get("deniedTools").and_then(|v| v.as_array()) {
        for item in denied {
            if let Some(s) = item.as_str() {
                rules.push(format!("deny: {s}"));
            }
        }
    }

    rules
}

#[cfg(test)]
#[path = "permissions.test.rs"]
mod tests;
