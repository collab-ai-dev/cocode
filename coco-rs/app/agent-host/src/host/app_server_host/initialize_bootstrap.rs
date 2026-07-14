/// Provides the data fields used to build an initialize response.
///
/// The concrete CLI implementation is installed by startup adapters, but the
/// process bootstrap snapshot is host state used before a runtime exists.
#[async_trait::async_trait]
pub trait InitializeBootstrap: Send + Sync {
    /// Currently-visible slash commands (hidden / feature-gated ones are
    /// filtered out). Empty if no registry is wired.
    async fn commands(&self) -> Vec<crate::session_runtime::SessionInitializeCommand>;

    /// Available subagents (built-ins + user-defined from disk). Empty if
    /// no agent source is wired.
    async fn agents(&self) -> Vec<crate::session_runtime::SessionInitializeAgent>;

    /// Account / auth info for the logged-in user. Returns `default()` if
    /// no auth source is wired.
    async fn account(&self) -> crate::session_runtime::SessionInitializeAccount;

    /// Currently-selected output style. Returns `"default"` if no source
    /// is wired.
    async fn output_style(&self) -> String;

    /// All output styles the server knows about (built-ins + user-defined
    /// markdown files). Returns `["default"]` if no source is wired.
    async fn available_output_styles(&self) -> Vec<String>;

    /// Current fast-mode rate-limit state, if tracked. Returns `None` to
    /// signal "feature not enabled" or "unknown".
    async fn fast_mode_state(&self) -> Option<coco_types::FastModeState>;

    /// Workspace cwd captured before a session/runtime exists.
    async fn cwd(&self) -> std::path::PathBuf;
}
