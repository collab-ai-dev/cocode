//! `/diff` — show git diff of current changes with formatted output.
//!
//! Runs `git diff` (staged + unstaged) and `git diff --stat` to produce
//! a human-readable summary of uncommitted changes.

use std::pin::Pin;

/// Maximum characters of diff output before truncation.
const MAX_DIFF_CHARS: usize = 6000;

/// Async handler for `/diff [options]`.
///
/// Runs git to collect staged changes, unstaged changes, and untracked
/// files, then formats a combined report.
pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let extra_args = args.trim().to_string();

        // Verify we're in a git repo
        let in_repo = run_git(&["rev-parse", "--is-inside-work-tree"]).await;
        if in_repo.is_err() {
            return Ok(
                "Not a git repository. Run this command from inside a git project.".to_string(),
            );
        }

        let mut out = String::new();

        // Staged changes
        let staged_stat = run_git(&["diff", "--cached", "--stat"]).await?;
        let staged_diff = run_git(&["diff", "--cached"]).await?;

        // Unstaged changes
        let unstaged_stat = run_git(&["diff", "--stat"]).await?;
        let unstaged_diff = run_git(&["diff"]).await?;

        // Untracked files
        let untracked = run_git(&["ls-files", "--others", "--exclude-standard"]).await?;

        let has_staged = !staged_stat.trim().is_empty();
        let has_unstaged = !unstaged_stat.trim().is_empty();
        let has_untracked = !untracked.trim().is_empty();

        if !has_staged && !has_unstaged && !has_untracked {
            return Ok("Working tree clean. No uncommitted changes.".to_string());
        }

        // Staged changes section
        if has_staged {
            out.push_str("## Staged changes\n\n");
            out.push_str(&staged_stat);
            out.push_str("\n\n");
            append_truncated_diff(&mut out, &staged_diff);
            out.push('\n');
        }

        // Unstaged changes section
        if has_unstaged {
            out.push_str("## Unstaged changes\n\n");
            out.push_str(&unstaged_stat);
            out.push_str("\n\n");
            append_truncated_diff(&mut out, &unstaged_diff);
            out.push('\n');
        }

        // Untracked files section
        if has_untracked {
            out.push_str("## Untracked files\n\n");
            let files: Vec<&str> = untracked.lines().collect();
            let display_count = files.len().min(30);
            for f in &files[..display_count] {
                out.push_str(&format!("  {f}\n"));
            }
            if files.len() > 30 {
                out.push_str(&format!("  ... and {} more\n", files.len() - 30));
            }
        }

        // Summary line
        let staged_file_count = count_stat_files(&staged_stat);
        let unstaged_file_count = count_stat_files(&unstaged_stat);
        let untracked_count = untracked.lines().filter(|l| !l.trim().is_empty()).count();

        out.push_str(&format!(
            "\nSummary: {staged_file_count} staged, {unstaged_file_count} unstaged, {untracked_count} untracked"
        ));

        // Handle extra args (e.g., --name-only)
        if !extra_args.is_empty() {
            let mut git_args = vec!["diff"];
            let parts: Vec<&str> = extra_args.split_whitespace().collect();
            git_args.extend(parts);
            let custom = run_git(&git_args).await?;
            if !custom.trim().is_empty() {
                out.push_str(&format!("\n\n## Custom diff ({extra_args})\n\n"));
                append_truncated_diff(&mut out, &custom);
            }
        }

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

/// Append diff text, truncating if it exceeds `MAX_DIFF_CHARS`.
fn append_truncated_diff(out: &mut String, diff: &str) {
    let trimmed = diff.trim();
    if trimmed.is_empty() {
        return;
    }
    if trimmed.len() > MAX_DIFF_CHARS {
        // Find a line boundary near the limit
        let truncate_at = trimmed[..MAX_DIFF_CHARS]
            .rfind('\n')
            .unwrap_or(MAX_DIFF_CHARS);
        out.push_str(&trimmed[..truncate_at]);
        let remaining_lines = trimmed[truncate_at..].lines().count();
        out.push_str(&format!("\n\n... truncated ({remaining_lines} more lines)"));
    } else {
        out.push_str(trimmed);
    }
}

/// Count the number of changed files from `git diff --stat` output.
///
/// The last line of `--stat` output looks like:
///   ` 3 files changed, 10 insertions(+), 2 deletions(-)`
fn count_stat_files(stat: &str) -> i64 {
    let last_line = stat.lines().last().unwrap_or("");
    // Parse "N file(s) changed" from the summary line
    if let Some(idx) = last_line.find(" file") {
        let before = last_line[..idx].trim();
        // The number is the last whitespace-separated token before " file"
        if let Some(num_str) = before.split_whitespace().next_back() {
            return num_str.parse().unwrap_or(0);
        }
    }
    0
}

#[cfg(test)]
#[path = "diff.test.rs"]
mod tests;
