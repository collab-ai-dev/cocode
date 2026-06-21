//! Enhanced skill tool features.
//!
//! Provides skill invocation with argument substitution, prompt expansion,
//! context fork vs inline execution, and skill validation.

// ── Argument substitution ──

/// Options for expanding a skill prompt template.
pub struct ExpandOptions<'a> {
    /// Arguments provided by the user (e.g., `/skill arg1 arg2`).
    pub args: &'a str,
    /// Named argument names from the skill definition (e.g., `["env", "region"]`).
    pub argument_names: &'a [String],
    /// The skill's source directory path (for `${CLAUDE_SKILL_DIR}`).
    pub skill_dir: Option<&'a str>,
    /// The current session ID (for `${CLAUDE_SESSION_ID}`).
    pub session_id: Option<&'a str>,
    /// Base directory to prepend (for `"Base directory for this skill: ..."` line).
    pub base_dir: Option<&'a str>,
    /// Plugin root directory (for `${CLAUDE_PLUGIN_ROOT}`).
    pub plugin_root: Option<&'a str>,
    /// Plugin persistent data directory (for `${CLAUDE_PLUGIN_DATA}`).
    pub plugin_data_dir: Option<&'a str>,
    /// User config values for `${user_config.KEY}` substitution.
    ///
    /// Keys are option names, values are (value, sensitive) pairs.
    /// Sensitive keys resolve to a placeholder instead of the actual value.
    pub user_config: Option<&'a [(&'a str, &'a str, bool)]>,
}

/// Expand placeholders in a skill prompt template.
///
/// Supports (in substitution order):
/// 1. Named args: `$env`, `$region` — from `argument_names`
/// 2. Indexed: `$ARGUMENTS[0]`, `$ARGUMENTS[1]`
/// 3. Positional shorthand: `$0`, `$1`, `${1}` — word-boundary safe
/// 4. Full: `$ARGUMENTS` / `${ARGUMENTS}` — replaced with the full args string
/// 5. `${CLAUDE_SKILL_DIR}` — replaced with the skill's source directory
/// 6. `${CLAUDE_SESSION_ID}` — replaced with the current session ID
/// 7. `${CLAUDE_PLUGIN_ROOT}` — replaced with the plugin's install directory
/// 8. `${CLAUDE_PLUGIN_DATA}` — replaced with the plugin's persistent data dir
/// 9. `${user_config.KEY}` — replaced with user config values (sensitive masked)
///
/// If no argument placeholders are found, appends `ARGUMENTS: {args}` to the prompt.
pub fn expand_skill_prompt(template: &str, opts: &ExpandOptions<'_>) -> String {
    let trimmed_args = opts.args.trim();
    let parts: Vec<&str> = trimmed_args.split_whitespace().collect();

    // Prepend base directory if provided
    let mut result = match opts.base_dir {
        Some(dir) if !dir.is_empty() => {
            format!("Base directory for this skill: {dir}\n\n{template}")
        }
        _ => template.to_string(),
    };

    // Replace ${CLAUDE_SKILL_DIR} with skill directory path
    if let Some(skill_dir) = opts.skill_dir {
        result = result.replace("${CLAUDE_SKILL_DIR}", skill_dir);
    }

    // Replace ${CLAUDE_SESSION_ID} with session ID
    if let Some(session_id) = opts.session_id {
        result = result.replace("${CLAUDE_SESSION_ID}", session_id);
    }

    // Replace ${CLAUDE_PLUGIN_ROOT} with plugin root directory
    if let Some(plugin_root) = opts.plugin_root {
        result = result.replace("${CLAUDE_PLUGIN_ROOT}", plugin_root);
    }

    // Replace ${CLAUDE_PLUGIN_DATA} with plugin data directory
    if let Some(plugin_data) = opts.plugin_data_dir {
        result = result.replace("${CLAUDE_PLUGIN_DATA}", plugin_data);
    }

    // Replace ${user_config.KEY} with user config values
    // Sensitive keys resolve to a descriptive placeholder instead of the
    // actual value to prevent secrets from leaking into model prompts.
    if let Some(config_entries) = opts.user_config {
        for &(key, value, sensitive) in config_entries {
            let placeholder = format!("${{user_config.{key}}}");
            if result.contains(&placeholder) {
                let replacement = if sensitive {
                    format!("[SENSITIVE:{key}]")
                } else {
                    value.to_string()
                };
                result = result.replace(&placeholder, &replacement);
            }
        }
    }

    // Track whether any argument placeholder was found
    let mut found_placeholder = false;

    // 1. Named args: $env, $region
    for (i, name) in opts.argument_names.iter().enumerate() {
        if name.is_empty() {
            continue;
        }
        let placeholder = format!("${name}");
        if result.contains(&placeholder) {
            let value = parts.get(i).copied().unwrap_or("");
            result = result.replace(&placeholder, value);
            found_placeholder = true;
        }
    }

    // 2. Indexed: $ARGUMENTS[0], $ARGUMENTS[1]
    for i in 0..20 {
        let placeholder = format!("$ARGUMENTS[{i}]");
        if result.contains(&placeholder) {
            let value = parts.get(i).copied().unwrap_or("");
            result = result.replace(&placeholder, value);
            found_placeholder = true;
        }
    }

    // 3. Positional shorthand: $0, $1, ${0}, ${1}.
    //    The shorthand is a zero-indexed alias for `$ARGUMENTS[N]`:
    //    `$0` = first arg, `$1` = second arg. This mirrors JS array
    //    indexing rather than shell positional parameters
    //    (see doc comment on `substituteArguments`).
    let has_positional = (0..=20)
        .any(|i| result.contains(&format!("${i}")) || result.contains(&format!("${{{i}}}")));
    if has_positional {
        for (i, part) in parts.iter().enumerate() {
            result = result.replace(&format!("${{{i}}}"), part);
            result = result.replace(&format!("${i}"), part);
        }
        // Clear remaining positional placeholders (0..=20).
        for idx in 0..=20 {
            result = result.replace(&format!("${{{idx}}}"), "");
            result = result.replace(&format!("${idx}"), "");
        }
        found_placeholder = true;
    }

    // 4. Full: $ARGUMENTS / ${ARGUMENTS}
    if result.contains("$ARGUMENTS") || result.contains("${ARGUMENTS}") {
        result = result.replace("${ARGUMENTS}", trimmed_args);
        result = result.replace("$ARGUMENTS", trimmed_args);
        found_placeholder = true;
    }

    // If no placeholders found, append args with "ARGUMENTS:" prefix
    if !found_placeholder && !trimmed_args.is_empty() {
        result = format!("{result}\n\nARGUMENTS: {trimmed_args}");
    }

    result
}

/// Normalize a skill name by stripping a leading slash and trimming.
pub fn normalize_skill_name(name: &str) -> &str {
    let trimmed = name.trim();
    trimmed.strip_prefix('/').unwrap_or(trimmed)
}

#[cfg(test)]
#[path = "skill_advanced.test.rs"]
mod tests;
