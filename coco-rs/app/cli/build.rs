use std::process::Command;

/// Embed build provenance (git short hash, commit date, commit subject, and
/// build timestamp) so `coco --version` reports exactly which commit a binary
/// was built from and when. Git-derived values prefer the `COCO_BUILD_*`
/// overrides (CI release sets them for reproducible builds), then fall back to
/// `git`, then `"unknown"`. The components are composed into the multi-line
/// version string in lib.rs — `cargo:rustc-env` values cannot contain newlines.
fn main() {
    // Re-run when an override changes, or when the checked-out commit changes
    // (HEAD on branch switch, logs/HEAD on commit/reset). We do NOT watch
    // .git/index, so routine `git add` / `git status` never rebuild.
    for key in [
        "COCO_BUILD_GIT_HASH",
        "COCO_BUILD_GIT_DATE",
        "COCO_BUILD_GIT_SUBJECT",
        "COCO_BUILD_TIME",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    for path in ["HEAD", "logs/HEAD"] {
        if let Some(p) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }

    let hash = env_override("COCO_BUILD_GIT_HASH")
        .or_else(|| git(&["rev-parse", "--short", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let date = env_override("COCO_BUILD_GIT_DATE")
        .or_else(|| git(&["log", "-1", "--format=%cs"]))
        .unwrap_or_else(|| "unknown".to_string());
    let subject = env_override("COCO_BUILD_GIT_SUBJECT")
        .or_else(|| git(&["log", "-1", "--format=%s"]))
        .unwrap_or_else(|| "unknown".to_string());
    let build_time = env_override("COCO_BUILD_TIME").unwrap_or_else(|| {
        chrono::Utc::now()
            .format("%Y-%m-%d %H:%M:%S UTC")
            .to_string()
    });

    println!("cargo:rustc-env=COCO_BUILD_GIT_HASH={hash}");
    println!("cargo:rustc-env=COCO_BUILD_GIT_DATE={date}");
    println!("cargo:rustc-env=COCO_BUILD_GIT_SUBJECT={subject}");
    println!("cargo:rustc-env=COCO_BUILD_TIME={build_time}");
}

fn env_override(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}
