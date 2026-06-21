use super::*;
use pretty_assertions::assert_eq;
use std::path::PathBuf;

#[test]
fn parses_well_formed_entry() {
    let content = "---\nname: my_role\ndescription: I'm a data scientist\ntype: user\n---\n\nUser is a data scientist.\n";
    let path = PathBuf::from("/abs/my_role.md");
    let entry = parse_memory_entry(&path, content).expect("parse");
    assert_eq!(entry.name, "my_role");
    assert_eq!(entry.description, "I'm a data scientist");
    assert_eq!(entry.memory_type, MemoryEntryType::User);
    assert_eq!(entry.content.trim(), "User is a data scientist.");
    assert_eq!(entry.filename, "my_role.md");
    assert_eq!(entry.file_path, path);
}

#[test]
fn rejects_missing_frontmatter() {
    let content = "no frontmatter here";
    let path = PathBuf::from("/abs/x.md");
    assert!(parse_memory_entry(&path, content).is_none());
}

#[test]
fn rejects_unknown_type() {
    let content = "---\nname: x\ndescription: y\ntype: nonsense\n---\nbody\n";
    let path = PathBuf::from("/abs/x.md");
    assert!(parse_memory_entry(&path, content).is_none());
}

#[test]
fn rejects_missing_required_field() {
    let content = "---\nname: x\ntype: user\n---\nbody\n";
    let path = PathBuf::from("/abs/x.md");
    assert!(parse_memory_entry(&path, content).is_none());
}

#[test]
fn formats_round_trips_through_parser() {
    let entry = MemoryEntry {
        name: "feedback_no_mocks".into(),
        description: "Don't mock the database".into(),
        memory_type: MemoryEntryType::Feedback,
        content: "Body line 1\nBody line 2".into(),
        filename: "feedback_no_mocks.md".into(),
        file_path: PathBuf::from("/abs/feedback_no_mocks.md"),
    };
    let md = format_entry_as_markdown(&entry);
    let path = PathBuf::from("/abs/feedback_no_mocks.md");
    let parsed = parse_memory_entry(&path, &md).expect("parse");
    assert_eq!(parsed.name, entry.name);
    assert_eq!(parsed.description, entry.description);
    assert_eq!(parsed.memory_type, entry.memory_type);
    assert_eq!(parsed.content.trim(), entry.content.trim());
}
