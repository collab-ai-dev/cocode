//! `/feedback` — prepare a coco-rs GitHub issue draft.

use std::io::Read;
use std::io::Seek;
use std::path::PathBuf;

use async_trait::async_trait;
use coco_secret_redact::redact_secrets;
use coco_secret_redact::scan_secrets;
use coco_utils_common::BuildProvenance;

use crate::CommandHandler;
use crate::SharedBuildProvenance;
use crate::snapshot_build_provenance;

const ISSUE_NEW_URL: &str = "https://github.com/collab-ai-dev/cocode/issues/new";
const LOG_TAIL_BYTES: u64 = 12 * 1024;
const LOG_BODY_CHAR_LIMIT: usize = 6_000;

#[derive(Debug, Clone, PartialEq, Eq)]
struct FeedbackArgs {
    include_logs: bool,
    description: String,
}

pub struct FeedbackHandler {
    provenance: SharedBuildProvenance,
}

impl FeedbackHandler {
    pub fn new(provenance: SharedBuildProvenance) -> Self {
        Self { provenance }
    }
}

#[async_trait]
impl CommandHandler for FeedbackHandler {
    async fn execute(&self, args: &str) -> crate::Result<String> {
        Ok(render(args, &snapshot_build_provenance(&self.provenance)))
    }

    fn handler_name(&self) -> &str {
        "feedback"
    }
}

pub fn handler(args: &str) -> String {
    render(args, &BuildProvenance::unknown(env!("CARGO_PKG_VERSION")))
}

fn render(args: &str, provenance: &BuildProvenance) -> String {
    let parsed = parse_args(args);
    if parsed.description.trim().is_empty() {
        return usage();
    }

    let title = issue_title(&parsed.description);
    let log_section = if parsed.include_logs {
        log_section_with_tail()
    } else {
        "Logs: not included. To include a best-effort redacted log tail, rerun with `--with-logs`.\n".to_string()
    };
    let body = issue_body(&parsed.description, &log_section, provenance);
    let url = format!(
        "{ISSUE_NEW_URL}?title={}&body={}",
        urlencoding::encode(&title),
        urlencoding::encode(&body),
    );

    format!(
        "Prepared coco-rs feedback issue draft:\n{url}\n\n\
         Logs included: {}\n\
         Review the issue body before submitting, especially if logs are included.",
        if parsed.include_logs {
            "yes, best-effort redacted tail"
        } else {
            "no"
        }
    )
}

fn usage() -> String {
    "Usage: /feedback [--with-logs] <bug report or feedback>\n\n\
     Opens a prefilled GitHub issue draft for collab-ai-dev/cocode.\n\
     Logs are not included by default. `--with-logs` includes only a \
     best-effort redacted tail of the current coco log; review before submitting."
        .to_string()
}

fn parse_args(args: &str) -> FeedbackArgs {
    let mut include_logs = false;
    let mut description = Vec::new();
    for part in args.split_whitespace() {
        match part {
            "--with-logs" | "--logs" | "--include-logs" => include_logs = true,
            "--no-logs" => include_logs = false,
            other => description.push(other),
        }
    }
    FeedbackArgs {
        include_logs,
        description: description.join(" "),
    }
}

fn issue_title(description: &str) -> String {
    let first_line = description
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Feedback")
        .trim();
    let mut title = first_line.chars().take(80).collect::<String>();
    if first_line.chars().count() > 80 {
        title.push_str("...");
    }
    if title.starts_with('[') {
        title
    } else {
        format!("[Feedback] {title}")
    }
}

fn issue_body(description: &str, log_section: &str, provenance: &BuildProvenance) -> String {
    format!(
        "## Description\n\n{}\n\n\
         ## Runtime\n\n\
         - Version: {}\n\
         - Commit: {} ({}) {}\n\
         - Built: {}\n\
         - OS: {}\n\
         - Arch: {}\n\
         - Family: {}\n\
         - Timestamp: {}\n\n\
         ## Logs\n\n{}",
        description.trim(),
        provenance.package_version,
        provenance.git_hash,
        provenance.git_date,
        provenance.git_subject,
        provenance.build_time,
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::FAMILY,
        chrono::Utc::now().to_rfc3339(),
        log_section,
    )
}

fn log_section_with_tail() -> String {
    let Some(path) = current_log_path() else {
        return "Requested logs, but no current coco log file was found.\n".to_string();
    };
    match read_tail(&path, LOG_TAIL_BYTES) {
        Ok(tail) => {
            let had_secret_match = !scan_secrets(&tail).is_empty();
            let redacted = redact_secrets(&tail);
            let mut excerpt = redacted
                .chars()
                .take(LOG_BODY_CHAR_LIMIT)
                .collect::<String>();
            if redacted.chars().count() > LOG_BODY_CHAR_LIMIT {
                excerpt.push_str("\n... [truncated]\n");
            }
            let sensitive_note = if had_secret_match {
                "Known secret-like patterns were detected and redacted. "
            } else {
                ""
            };
            format!(
                "{sensitive_note}Best-effort redacted log tail from the current coco process. \
                 Review before submitting.\n\n```text\n{excerpt}\n```\n"
            )
        }
        Err(err) => {
            format!("Requested logs, but the current coco log tail could not be read: {err}\n")
        }
    }
}

fn current_log_path() -> Option<PathBuf> {
    let pid = std::process::id();
    let mut candidates = Vec::new();
    if let Ok(explicit) = std::env::var("COCO_LOG_FILE")
        && !explicit.trim().is_empty()
    {
        candidates.extend(rotating_candidates(&PathBuf::from(explicit)));
    }

    let default = coco_config::global_config::config_home()
        .join("logs")
        .join(format!("coco.{pid}.log"));
    candidates.extend(rotating_candidates(&default));

    candidates.into_iter().find(|p| p.is_file())
}

fn rotating_candidates(base: &std::path::Path) -> Vec<PathBuf> {
    let mut out = vec![base.to_path_buf()];
    let today_utc = chrono::Utc::now().format("%Y-%m-%d").to_string();
    out.push(PathBuf::from(format!("{}.{}", base.display(), today_utc)));
    let today_local = chrono::Local::now().format("%Y-%m-%d").to_string();
    if today_local != today_utc {
        out.push(PathBuf::from(format!("{}.{}", base.display(), today_local)));
    }
    out
}

fn read_tail(path: &std::path::Path, max_bytes: u64) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(std::io::SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
#[path = "feedback.test.rs"]
mod tests;
