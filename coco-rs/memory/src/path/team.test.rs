//! Tests for team-memory path combinators.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::path::validate::PathValidationError;

fn team_dir() -> (TempDir, PathBuf) {
    let td = TempDir::new().unwrap();
    let team = td.path().join("memory").join("team");
    std::fs::create_dir_all(&team).unwrap();
    (td, team)
}

// ── validate_team_mem_key ────────────────────────────────────────────

#[test]
fn key_simple_relative_ok() {
    let (_td, team) = team_dir();
    let resolved = validate_team_mem_key("notes.md", &team).unwrap();
    assert!(resolved.ends_with("memory/team/notes.md"));
}

#[test]
fn key_nested_ok() {
    let (_td, team) = team_dir();
    let resolved = validate_team_mem_key("topics/a.md", &team).unwrap();
    assert!(resolved.ends_with("memory/team/topics/a.md"));
}

#[test]
fn key_with_traversal_rejected() {
    let (_td, team) = team_dir();
    let err = validate_team_mem_key("../escape.md", &team).unwrap_err();
    assert_eq!(err, PathValidationError::Traversal);
}

#[test]
fn key_absolute_rejected() {
    let (_td, team) = team_dir();
    let err = validate_team_mem_key("/etc/passwd", &team).unwrap_err();
    assert_eq!(err, PathValidationError::AbsolutePath);
}

#[test]
fn key_with_null_byte_rejected() {
    let (_td, team) = team_dir();
    let err = validate_team_mem_key("notes\0.md", &team).unwrap_err();
    assert_eq!(err, PathValidationError::NullByte);
}

#[test]
fn key_with_url_encoded_traversal_rejected() {
    let (_td, team) = team_dir();
    let err = validate_team_mem_key("%2e%2e%2fescape.md", &team).unwrap_err();
    assert_eq!(err, PathValidationError::Traversal);
}

#[test]
fn key_with_fullwidth_unicode_rejected() {
    let (_td, team) = team_dir();
    let err = validate_team_mem_key("\u{FF0E}\u{FF0E}/escape.md", &team).unwrap_err();
    assert_eq!(err, PathValidationError::UnicodeTraversal);
}

#[test]
fn key_empty_rejected() {
    let (_td, team) = team_dir();
    let err = validate_team_mem_key("", &team).unwrap_err();
    assert_eq!(err, PathValidationError::Empty);
}

#[cfg(unix)]
#[test]
fn key_symlink_escape_rejected() {
    // Plant a symlink inside team_dir that points outside, then ask
    // `validate_team_mem_key` to write under it. The lexical check
    // can't see the escape; the realpath pass must.
    let (td, team) = team_dir();
    let outside = td.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let link = team.join("badlink");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let err = validate_team_mem_key("badlink/secret.md", &team).unwrap_err();
    assert_eq!(err, PathValidationError::Escape);
}

#[test]
fn key_missing_team_dir_skips_realpath_check() {
    // When team_dir doesn't exist, a symlink escape is impossible
    // (no symlink can have been planted); realpath check returns
    // true on ENOENT.
    let td = TempDir::new().unwrap();
    let team = td.path().join("missing").join("team");
    let resolved = validate_team_mem_key("notes.md", &team).unwrap();
    assert!(resolved.ends_with("missing/team/notes.md"));
}

// ── validate_team_mem_write_path ─────────────────────────────────────

#[test]
fn write_path_inside_team_ok() {
    let (_td, team) = team_dir();
    let target = team.join("notes.md");
    let resolved = validate_team_mem_write_path(&target, &team).unwrap();
    assert_eq!(resolved, target);
}

#[test]
fn write_path_nested_ok() {
    let (_td, team) = team_dir();
    let target = team.join("topics").join("nested").join("a.md");
    let resolved = validate_team_mem_write_path(&target, &team).unwrap();
    assert!(resolved.ends_with("topics/nested/a.md"));
}

#[test]
fn write_path_outside_team_rejected() {
    let (td, team) = team_dir();
    let outside = td.path().join("other").join("evil.md");
    let err = validate_team_mem_write_path(&outside, &team).unwrap_err();
    assert_eq!(err, PathValidationError::Escape);
}

#[test]
fn write_path_with_dotdot_normalized_then_rejected() {
    let (_td, team) = team_dir();
    let trick = team.join("..").join("escape.md");
    let err = validate_team_mem_write_path(&trick, &team).unwrap_err();
    assert_eq!(err, PathValidationError::Escape);
}

#[test]
fn write_path_with_null_byte_rejected() {
    let (_td, team) = team_dir();
    let p = PathBuf::from(format!("{}/bad\0name.md", team.to_string_lossy()));
    let err = validate_team_mem_write_path(&p, &team).unwrap_err();
    assert_eq!(err, PathValidationError::NullByte);
}

#[test]
fn write_path_prefix_attack_rejected() {
    // `team_dir = .../team`, candidate `.../team-evil/...` should be
    // rejected because it shares the string prefix but is not inside.
    let (td, team) = team_dir();
    let evil = td.path().join("memory").join("team-evil").join("x.md");
    let err = validate_team_mem_write_path(&evil, &team).unwrap_err();
    assert_eq!(err, PathValidationError::Escape);
}

#[cfg(unix)]
#[test]
fn write_path_symlink_escape_rejected() {
    let (td, team) = team_dir();
    let outside = td.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let link = team.join("link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let trick = link.join("secret.md");
    let err = validate_team_mem_write_path(&trick, &team).unwrap_err();
    assert_eq!(err, PathValidationError::Escape);
}
