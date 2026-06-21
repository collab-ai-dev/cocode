//! `/memory` — list and manage CLAUDE.md memory files.
//!
//! Scans well-known locations for CLAUDE.md memory files and reports
//! their paths, line counts, and sizes.

use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;

/// Async handler for `/memory [refresh]`.
pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        match args.trim() {
            "refresh" => Ok("Memory files will be reloaded on the next turn.\n\
                 Changes to CLAUDE.md files take effect immediately."
                .to_string()),
            "" => list_memory_files().await,
            other => Ok(format!(
                "Unknown memory subcommand: {other}\n\n\
                 Usage:\n\
                 /memory           List all memory files\n\
                 /memory refresh   Report that memory files will be reloaded"
            )),
        }
    })
}

/// Candidate memory file locations to probe.
fn memory_candidates(home: Option<PathBuf>) -> Vec<(PathBuf, &'static str)> {
    let mut candidates: Vec<(PathBuf, &'static str)> = vec![
        (PathBuf::from("CLAUDE.md"), "CLAUDE.md"),
        (PathBuf::from("CLAUDE.local.md"), "CLAUDE.local.md"),
    ];

    // .coco/rules/*.md — scanned separately; placeholder entry omitted here.
    // We handle the glob inline in list_memory_files.

    if let Some(home) = home {
        candidates.push((home.join(".coco").join("CLAUDE.md"), "~/.coco/CLAUDE.md"));
    }

    candidates
}

/// Information about a found memory file.
struct MemoryFile {
    display_path: String,
    line_count: usize,
    byte_size: u64,
}

/// List all memory files with their stats.
async fn list_memory_files() -> crate::Result<String> {
    let home = dirs::home_dir();
    let mut files: Vec<MemoryFile> = Vec::new();

    // Fixed candidate locations
    for (path, label) in memory_candidates(home) {
        if let Some(info) = probe_file(&path, label).await {
            files.push(info);
        }
    }

    // .coco/rules/*.md — scan directory
    scan_rules_dir(Path::new(".coco/rules"), &mut files).await;

    let mut out = String::from("## Memory Files\n\n");

    if files.is_empty() {
        out.push_str("No memory files found.\n\n");
        out.push_str("Checked locations:\n");
        out.push_str("  CLAUDE.md              (project root)\n");
        out.push_str("  CLAUDE.local.md        (personal, gitignored)\n");
        out.push_str("  .coco/rules/*.md       (project rules)\n");
        out.push_str("  ~/.coco/CLAUDE.md      (user global)\n");
    } else {
        out.push_str(&format!(
            "{} memory file{} found:\n\n",
            files.len(),
            if files.len() == 1 { "" } else { "s" },
        ));

        out.push_str("| File                        | Lines | Size     |\n");
        out.push_str("|-----------------------------|-------|----------|\n");

        for f in &files {
            out.push_str(&format!(
                "| {:<27} | {:>5} | {:>8} |\n",
                f.display_path,
                f.line_count,
                format_bytes(f.byte_size),
            ));
        }

        out.push_str(&format!(
            "\nTotal: {} file{}, {} lines, {}",
            files.len(),
            if files.len() == 1 { "" } else { "s" },
            files.iter().map(|f| f.line_count).sum::<usize>(),
            format_bytes(files.iter().map(|f| f.byte_size).sum()),
        ));
    }

    out.push_str("\n\nCommands:\n");
    out.push_str("  /memory           List memory files\n");
    out.push_str("  /memory refresh   Reload memory files");

    Ok(out)
}

/// Probe a single file path and return its stats if it exists.
async fn probe_file(path: &Path, display_path: &str) -> Option<MemoryFile> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    let meta = tokio::fs::metadata(path).await.ok()?;
    Some(MemoryFile {
        display_path: display_path.to_string(),
        line_count: content.lines().count(),
        byte_size: meta.len(),
    })
}

/// Scan `.coco/rules/` for `*.md` files and append any found.
async fn scan_rules_dir(dir: &Path, files: &mut Vec<MemoryFile>) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };

    let mut found: Vec<(String, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let label = format!(".coco/rules/{name}");
            found.push((label, path));
        }
    }

    // Sort for stable output order
    found.sort_by(|a, b| a.0.cmp(&b.0));

    for (label, path) in found {
        if let Some(info) = probe_file(&path, &label).await {
            files.push(info);
        }
    }
}

/// Format a byte count as a human-readable string.
fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
#[path = "memory.test.rs"]
mod tests;
