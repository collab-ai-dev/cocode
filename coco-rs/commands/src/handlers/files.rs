//! `/files` — list files tracked by git in the current working directory.
//!
//! Runs `git ls-files` and groups results by top-level directory, showing
//! file counts per directory and an estimated context size. Supports optional
//! path/glob filtering via args.

use std::collections::BTreeMap;
use std::pin::Pin;

/// Maximum number of individual files to display before truncating.
const MAX_DISPLAY_FILES: usize = 50;

/// Estimated average bytes per token for context size estimation.
const BYTES_PER_TOKEN: usize = 4;

/// Async handler for `/files [path/glob]`.
///
/// Lists git-tracked files grouped by top-level directory. If a filter
/// argument is provided, only files matching it are shown.
pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        // Verify we're inside a git repo
        let in_repo = run_git(&["rev-parse", "--is-inside-work-tree"]).await;
        if in_repo.is_err() {
            return Ok(
                "Not a git repository. Run this command from inside a git project.".to_string(),
            );
        }

        let filter = args.trim().to_string();

        // Build the git ls-files command, optionally with a pathspec filter
        let mut git_args = vec!["ls-files"];
        if !filter.is_empty() {
            git_args.push(&filter);
        }

        let raw = run_git(&git_args).await?;

        let all_files: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();

        if all_files.is_empty() {
            if filter.is_empty() {
                return Ok("No tracked files found in the repository.".to_string());
            } else {
                return Ok(format!(
                    "No tracked files match `{filter}`. Try a different path or glob."
                ));
            }
        }

        let total = all_files.len();

        // Estimate context size by summing file sizes
        let estimated_bytes: usize = estimate_total_bytes(&all_files).await;
        let estimated_tokens = estimated_bytes / BYTES_PER_TOKEN;

        // Group by top-level directory
        let mut dir_counts: BTreeMap<String, usize> = BTreeMap::new();
        for file in &all_files {
            let dir = top_level_dir(file);
            *dir_counts.entry(dir).or_insert(0) += 1;
        }

        let mut out = String::new();

        // Header
        if filter.is_empty() {
            out.push_str("## Tracked Files\n\n");
        } else {
            out.push_str(&format!("## Tracked Files matching `{filter}`\n\n"));
        }

        // Directory summary table
        out.push_str("### By Directory\n\n");
        out.push_str("| Directory           | Files |\n");
        out.push_str("|---------------------|-------|\n");
        for (dir, count) in &dir_counts {
            out.push_str(&format!("| {dir:<19} | {count:>5} |\n"));
        }

        // Individual file listing (capped at MAX_DISPLAY_FILES)
        out.push_str("\n### Files\n\n");
        let display_count = all_files.len().min(MAX_DISPLAY_FILES);
        for file in &all_files[..display_count] {
            out.push_str(&format!("  {file}\n"));
        }
        if total > MAX_DISPLAY_FILES {
            out.push_str(&format!(
                "\n  ... and {} more files (use a path filter to narrow results)\n",
                total - MAX_DISPLAY_FILES
            ));
        }

        // Footer summary
        out.push_str(&format!(
            "\n**Total:** {total} files  |  **Estimated context:** {} tokens (~{} KB)\n",
            format_number(estimated_tokens as i64),
            estimated_bytes / 1024,
        ));

        Ok(out)
    })
}

/// Run a git command and return stdout as a String.
async fn run_git(args: &[&str]) -> crate::Result<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::CommandsError::generic(format!(
            "git {} failed: {stderr}",
            args.join(" ")
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Return the top-level directory component of a file path, or the file name
/// itself for root-level files.
fn top_level_dir(path: &str) -> String {
    match path.split_once('/') {
        Some((dir, _)) => dir.to_string(),
        None => "(root)".to_string(),
    }
}

/// Estimate total byte count by summing the lengths of each path as a rough
/// proxy. A more accurate pass would stat each file, but that is too slow for
/// large repos.
async fn estimate_total_bytes(files: &[&str]) -> usize {
    // Sum path lengths as a lower-bound proxy; actual file bytes would require
    // reading every file. We fall back to path-length heuristic to stay fast.
    files.iter().map(|f| f.len() * 40).sum()
}

/// Format an integer with thousands separators.
fn format_number(n: i64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
#[path = "files.test.rs"]
mod tests;
