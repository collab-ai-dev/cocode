//! `model-roles` block — every `ModelRole` and the settings key that binds it.
//!
//! Role and settings key come from `coco_types::ModelRole`. The purpose blurb
//! is not in the code, so it lives in the exhaustive match below: a new variant
//! stops compiling until someone writes a blurb for it.

use anyhow::Result;
use coco_types::ModelRole;

pub fn render() -> Result<String> {
    let mut out = String::from("| Role | Settings key | Used for |\n| --- | --- | --- |");
    for role in ModelRole::ALL {
        let (display, purpose) = role_doc(role);
        let key = role.as_str();
        out.push_str(&format!("\n| {display} | `models.{key}` | {purpose} |"));
    }
    Ok(out)
}

/// Display name + purpose blurb per role. Exhaustive on purpose — see module docs.
fn role_doc(role: ModelRole) -> (&'static str, &'static str) {
    match role {
        ModelRole::Main => (
            "Main",
            "The primary conversation and coding agent. Required.",
        ),
        ModelRole::Plan => ("Plan", "Plan mode"),
        ModelRole::Fast => ("Fast", "Cheap helper calls, such as title generation"),
        ModelRole::Explore => ("Explore", "Read-only codebase exploration"),
        ModelRole::Review => ("Review", "Review-oriented subagent work"),
        ModelRole::Subagent => ("Subagent", "Generic spawned subagents"),
        ModelRole::Memory => ("Memory", "Memory extraction and recall"),
        ModelRole::HookAgent => ("HookAgent", "Agents invoked by hooks"),
    }
}
