//! The generated doc blocks.
//!
//! Each block renders a markdown table from the code that owns the facts, and
//! is spliced into the `<!-- BEGIN/END GENERATED: <id> -->` markers in its file.
//! Rendering is pure and ordering follows each source's natural order, so output
//! is byte-stable across runs.

mod cli_flags;
mod cli_subcommands;
mod features;
mod model_roles;
mod providers;

use anyhow::Context;
use anyhow::Result;

pub struct Block {
    /// Marker id, e.g. `providers`.
    pub id: &'static str,
    /// Path relative to the repository root.
    pub file: &'static str,
    pub body: String,
}

/// Render every block. Fails if any source has grown a row the generator does
/// not know how to document.
pub fn render_all() -> Result<Vec<Block>> {
    Ok(vec![
        block("providers", "docs/providers-and-auth.md", providers::render)?,
        block("model-roles", "docs/models-and-moa.md", model_roles::render)?,
        block("features", "docs/configuration.md", features::render)?,
        block("cli-flags", "docs/cli-reference.md", cli_flags::render)?,
        block(
            "cli-subcommands",
            "docs/cli-reference.md",
            cli_subcommands::render,
        )?,
    ])
}

fn block(id: &'static str, file: &'static str, render: fn() -> Result<String>) -> Result<Block> {
    let body = render().with_context(|| format!("rendering the `{id}` block for {file}"))?;
    Ok(Block { id, file, body })
}
