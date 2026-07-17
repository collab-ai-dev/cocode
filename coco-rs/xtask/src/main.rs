//! Dev-only automation for coco-rs. Not part of the shipped binary — kept out
//! of `default-members` so normal builds never pay for it.
//!
//! `xtask docs-gen` regenerates the reference tables in `docs/*.md` from the
//! code that owns the facts; `--check` turns it into a drift gate for CI.

mod blocks;
mod markers;

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use clap::Parser;
use clap::Subcommand;
use similar::ChangeTag;
use similar::TextDiff;

#[derive(Parser)]
#[command(name = "xtask", about = "coco-rs repository automation")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Regenerate the marked reference tables in docs/.
    DocsGen {
        /// Report drift and exit non-zero instead of writing. The CI gate.
        #[arg(long)]
        check: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::DocsGen { check } => docs_gen(check),
    }
}

/// Resolved from `CARGO_MANIFEST_DIR` (baked in at compile time), so the
/// generator works from any working directory: `<root>/coco-rs/xtask` → `<root>`.
fn repo_root() -> Result<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(root) = manifest_dir.parent().and_then(Path::parent) else {
        bail!("cannot resolve the repository root from {manifest_dir:?}");
    };
    Ok(root.to_path_buf())
}

fn docs_gen(check: bool) -> Result<()> {
    let root = repo_root()?;
    let blocks = blocks::render_all()?;

    // Group by file: cli-reference.md owns two blocks, and each write has to
    // carry both.
    let mut files: Vec<&str> = blocks.iter().map(|block| block.file).collect();
    files.dedup();

    let mut drifted = Vec::new();
    for file in files {
        let path = root.join(file);
        let original = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;

        let mut updated = original.clone();
        for block in blocks.iter().filter(|block| block.file == file) {
            updated = markers::splice(&updated, block.id, &block.body, file)?;
        }

        if updated == original {
            continue;
        }
        if check {
            println!("{}", render_diff(file, &original, &updated));
            drifted.push(file);
        } else {
            std::fs::write(&path, &updated)
                .with_context(|| format!("writing {}", path.display()))?;
            println!("regenerated {file}");
        }
    }

    if check && !drifted.is_empty() {
        let list = drifted.join(", ");
        bail!(
            "generated docs are out of date ({list}). Run `just docs-gen` and commit the result."
        );
    }
    if !check {
        println!("docs-gen: all blocks up to date.");
    }
    Ok(())
}

fn render_diff(file: &str, original: &str, updated: &str) -> String {
    let diff = TextDiff::from_lines(original, updated);
    let mut out = format!("--- {file} (committed)\n+++ {file} (generated)\n");
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => continue,
        };
        out.push_str(&format!("{sign}{change}"));
    }
    out
}
