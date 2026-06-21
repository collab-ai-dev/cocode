use serde::Deserialize;
use serde::Serialize;

/// User-facing sandbox mode selection.
/// Needed by exec/sandbox and coco-config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    FullAccess,
    ExternalSandbox,
}
