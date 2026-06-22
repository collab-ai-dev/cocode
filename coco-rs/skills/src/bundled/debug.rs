//! `/debug` — help diagnose issues in the current session. Mirrors claude-code's debug.ts.

pub fn prompt() -> String {
    let config_dir = coco_utils_common::COCO_CONFIG_DIR_NAME;
    let log_path = format!("~/{config_dir}/logs/coco.log");
    let user_settings = format!("~/{config_dir}/settings.json");
    let project_settings = format!("{config_dir}/settings.json");
    let local_settings = format!("{config_dir}/settings.local.json");
    TEMPLATE
        .replace("__PRODUCT__", coco_config::constants::PRODUCT_NAME)
        .replace("__LOG_PATH__", &log_path)
        .replace("__USER_SETTINGS__", &user_settings)
        .replace("__PROJECT_SETTINGS__", &project_settings)
        .replace("__LOCAL_SETTINGS__", &local_settings)
}

const TEMPLATE: &str = r#"# Debug Skill

Help the user debug an issue they're encountering in this current Claude Code session.

## Session Log

__PRODUCT__ writes one rotating log at `__LOG_PATH__`. Read the tail of this file yourself using the Read or Bash tool (e.g. the last ~100 lines) and analyze it.

For additional context, grep for [ERROR] and [WARN] lines across the full file.

## Settings

Remember that settings are in:
* user - `__USER_SETTINGS__`
* user - `__USER_SETTINGS__`
* project - `__PROJECT_SETTINGS__`
* local - `__LOCAL_SETTINGS__`

## Instructions

1. Review the user's issue description
2. Tail `__LOG_PATH__` to see the debug file format. Look for [ERROR] and [WARN] entries, stack traces, and failure patterns across the file
3. Consider launching the claude-code-guide subagent to understand the relevant Claude Code features
4. Explain what you found in plain language
5. Suggest concrete fixes or next steps
"#;
