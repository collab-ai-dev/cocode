/// Clap-independent inputs consumed by agent-host use cases.
///
/// The CLI maps parsed arguments into this value once. Other surfaces and
/// tests can construct it directly without depending on command-line syntax.
#[derive(Clone, Debug, Default)]
pub struct AgentHostOptions {
    pub prompt: Option<String>,
    pub models_main: Option<String>,
    pub settings: Option<String>,
    pub event_hub_url: Option<String>,
    pub max_tokens: Option<i64>,
    pub max_turns: Option<i32>,
    pub permission_mode: Option<String>,
    pub cwd: Option<String>,
    pub resume: Option<String>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub continue_session: bool,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub fallback_model: Vec<String>,
    pub add_dir: Vec<String>,
    pub dangerously_skip_permissions: bool,
    pub allow_dangerously_skip_permissions: bool,
    pub no_session_persistence: bool,
    pub json_schema: Option<String>,
    pub include_hook_events: bool,
    pub append_system_prompt_file: Option<String>,
    pub plan_mode_instructions: Option<String>,
    pub setting_sources: Option<String>,
    pub fork_session: bool,
    pub session_id: Option<String>,
}
