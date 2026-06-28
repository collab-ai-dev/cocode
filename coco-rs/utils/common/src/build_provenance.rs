use std::process::Command;

/// Build provenance stamped into a coco binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildProvenance {
    pub package_version: String,
    pub git_hash: String,
    pub git_date: String,
    pub git_subject: String,
    pub build_time: String,
}

impl BuildProvenance {
    pub fn new(
        package_version: impl Into<String>,
        git_hash: impl Into<String>,
        git_date: impl Into<String>,
        git_subject: impl Into<String>,
        build_time: impl Into<String>,
    ) -> Self {
        Self {
            package_version: package_version.into(),
            git_hash: git_hash.into(),
            git_date: git_date.into(),
            git_subject: git_subject.into(),
            build_time: build_time.into(),
        }
    }

    pub fn unknown(package_version: impl Into<String>) -> Self {
        Self::new(package_version, "unknown", "unknown", "unknown", "unknown")
    }
}

/// Emit the `COCO_BUILD_*` env values consumed by binary crates at compile
/// time. This is intended to be called from a Cargo `build.rs`.
pub fn emit_cargo_build_provenance() {
    // Re-run when an override changes, or when the checked-out commit changes
    // (HEAD on branch switch, logs/HEAD on commit/reset). Do not watch
    // .git/index, so routine `git add` / `git status` does not rebuild.
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

    let provenance = cargo_build_provenance();
    println!(
        "cargo:rustc-env=COCO_BUILD_GIT_HASH={}",
        provenance.git_hash
    );
    println!(
        "cargo:rustc-env=COCO_BUILD_GIT_DATE={}",
        provenance.git_date
    );
    println!(
        "cargo:rustc-env=COCO_BUILD_GIT_SUBJECT={}",
        provenance.git_subject
    );
    println!("cargo:rustc-env=COCO_BUILD_TIME={}", provenance.build_time);
}

fn cargo_build_provenance() -> BuildProvenance {
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

    BuildProvenance::new("unknown", hash, date, subject, build_time)
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
