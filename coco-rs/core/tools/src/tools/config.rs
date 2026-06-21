//! ConfigTool — get/set known config keys.
//!
//! Input is `setting` (required) plus `value` (optional; omit to read).
//! Mutating sets currently emit an instructional message instead of writing
//! config — the authoritative path is the CLI `config` subcommand or direct
//! edits to `~/.coco/config.json`.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Model-facing prompt. Covers intro + get/set usage +
/// configurable-settings list.
const CONFIG_PROMPT: &str = "Get or set coco configuration settings.

View or change coco settings. Use when the user requests configuration changes, asks about current settings, or when adjusting a setting would benefit them.

## Usage
- **Get current value:** Omit the \"value\" parameter
- **Set new value:** Include the \"value\" parameter

## Configurable settings
- model — the active model
- provider — the active provider
- thinking_level — reasoning effort level
- max_budget_usd — per-session budget ceiling
- permission_mode — default permission mode
- sandbox_mode — sandbox enforcement mode
- custom_system_prompt — replace the system prompt
- append_system_prompt — append to the system prompt
- verbose — true/false — verbose output
- debug — true/false — debug logging";

/// Typed input for [`ConfigTool`].
/// `setting` (required) + `value` (optional — omit to read).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ConfigInput {
    /// The setting key (e.g., "model", "provider", "permission_mode")
    pub setting: String,
    /// The new value. Omit to get current value.
    #[serde(default)]
    pub value: Option<ConfigValue>,
}

/// A config value — `string | boolean | number`. Untagged so the model
/// can pass any of the three scalar JSON types.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ConfigValue {
    Bool(bool),
    Number(f64),
    String(String),
}

/// Which side of the get/set request a [`ConfigOutput`] describes.
/// Closed 2-value set produced and consumed inside coco-rs — typed rather
/// than a stringly `"get"`/`"set"`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigOperation {
    Get,
    Set,
}

/// Typed output. `operation` is `get`/`set`; `setting`/`value` echo the
/// resolved request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigOutput {
    /// Human-readable status / next-step instruction.
    pub message: String,
    /// `get` or `set`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<ConfigOperation>,
    /// The setting key the request targeted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting: Option<String>,
    /// Populated for `set` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

pub struct ConfigTool;

#[async_trait::async_trait]
impl Tool for ConfigTool {
    type Input = ConfigInput;
    coco_tool_runtime::impl_runtime_schema!(ConfigInput);
    type Output = ConfigOutput;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::Config)
    }
    fn name(&self) -> &str {
        ToolName::Config.as_str()
    }
    fn description(&self, _input: &ConfigInput, _options: &DescriptionOptions) -> String {
        "Get or set coco configuration settings.".into()
    }
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        CONFIG_PROMPT.into()
    }

    /// Reads are obviously safe; sets currently just emit an instructional
    /// message rather than writing config, so they're safe too. Should
    /// the tool ever start mutating a shared settings file, demote to
    /// input-conditional safety like BashTool.
    fn is_concurrency_safe(&self, _input: &ConfigInput) -> bool {
        true
    }
    fn is_read_only(&self, input: &ConfigInput) -> bool {
        // A read is `value` omitted.
        input.value.is_none()
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("get or set coco settings (model, provider)")
    }

    /// Render the prebuilt `message` field, optionally followed by the
    /// list of available keys (for the `list` action). Skips JSON
    /// envelope overhead — the model only needs the human prose.
    fn render_for_model(&self, out: &ConfigOutput) -> Vec<ToolResultContentPart> {
        let text = if out.message.is_empty() {
            serde_json::to_string(out).unwrap_or_default()
        } else {
            out.message.clone()
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: ConfigInput,
        _ctx: &ToolUseContext,
    ) -> Result<ToolResult<ConfigOutput>, ToolError> {
        let setting = input.setting;
        if setting.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "setting parameter is required".into(),
                error_code: None,
            });
        }

        // `value` absent ⇒ get, else set.
        let data = match input.value {
            None => ConfigOutput {
                message: format!(
                    "Configuration value for '{setting}' is managed by ConfigManager. Use the CLI 'config' subcommand to view or edit settings."
                ),
                operation: Some(ConfigOperation::Get),
                setting: Some(setting),
                value: None,
            },
            Some(value) => {
                let value = serde_json::to_value(&value).unwrap_or(Value::Null);
                ConfigOutput {
                    message: format!(
                        "To set '{setting}', use the CLI 'config set {setting} <value>' command or edit the config file directly."
                    ),
                    operation: Some(ConfigOperation::Set),
                    setting: Some(setting),
                    value: Some(value),
                }
            }
        };

        Ok(ToolResult {
            data,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}
