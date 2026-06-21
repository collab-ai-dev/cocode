//! Layer 2 of the tool-filter pipeline: per-model tool overrides.
//!
//! Capabilities are **intrinsic to the model**, not the provider.
//! `gpt-5` emits `apply_patch` whether you reach it via OpenAI direct,
//! Azure, or any OpenAI-compatible gateway — the lookup keys on
//! `model_id` alone. Provider is a routing concern (URL, auth, wire
//! API) and orthogonal to which tools the model accepts.
//!
//! ## Two layers, in order
//!
//! 1. **Built-in registry** — pattern-match on `model_id` for
//!    well-known families (today: `gpt-5*`).
//! 2. **`ModelInfo.tool_overrides`** — settings.json can declare
//!    per-model overrides under
//!    `providers.<name>.models.<id>.tool_overrides`. User entries layer
//!    on top of the built-in diff (`excluded` always wins).
//!
//! Adding a new built-in family means: pattern-match in
//! `builtin_tool_overrides_for` and add a `Tool` impl gated on
//! `ToolOverrides::is_extra(...)` if it's a model-specific tool.

use coco_types::ToolId;
use coco_types::ToolName;
use coco_types::ToolOverrides;

use crate::model::ModelInfo;

/// Resolve the tool-overrides diff for a model, optionally layered with
/// a `ModelInfo` entry from settings.json.
///
/// Called at session bootstrap; the result is wrapped in `Arc` and
/// stored on `RuntimeConfig.tool_overrides` + the per-turn
/// `ToolUseContext.tool_overrides`. Subagent contexts inherit the
/// parent's value — they never widen it.
pub fn resolve_tool_overrides(model_id: &str, info: Option<&ModelInfo>) -> ToolOverrides {
    let mut overrides = builtin_tool_overrides_for(model_id);
    if let Some(info) = info
        && let Some(user) = &info.tool_overrides
    {
        overrides = overrides.merge(user);
    }
    overrides
}

/// Built-in registry of well-known model families.
///
/// Match order matters when a `model_id` could fit multiple patterns —
/// list more specific patterns before more general ones.
fn builtin_tool_overrides_for(model_id: &str) -> ToolOverrides {
    if model_id.starts_with("gpt-5") {
        // gpt-5 family routes ALL file mutation through `apply_patch`:
        // `Edit` and `Write` are both rejected (the patch envelope's
        // `*** Update File` / `*** Add File` hunks cover edits and new
        // files alike, matching the gpt-5 base instructions).
        return ToolOverrides::default()
            .with_extra(ToolId::Builtin(ToolName::ApplyPatch))
            .with_excluded(ToolId::Builtin(ToolName::Edit))
            .with_excluded(ToolId::Builtin(ToolName::Write));
    }
    ToolOverrides::none()
}

#[cfg(test)]
#[path = "tool_overrides.test.rs"]
mod tests;
