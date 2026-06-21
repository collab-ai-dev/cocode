use super::*;
use pretty_assertions::assert_eq;

#[test]
fn parse_accepts_four_canonical_strings() {
    assert_eq!(MemoryEntryType::parse("user"), Some(MemoryEntryType::User));
    assert_eq!(
        MemoryEntryType::parse("feedback"),
        Some(MemoryEntryType::Feedback)
    );
    assert_eq!(
        MemoryEntryType::parse("project"),
        Some(MemoryEntryType::Project)
    );
    assert_eq!(
        MemoryEntryType::parse("reference"),
        Some(MemoryEntryType::Reference)
    );
}

#[test]
fn parse_rejects_unknown_strings() {
    assert_eq!(MemoryEntryType::parse(""), None);
    assert_eq!(MemoryEntryType::parse("USER"), None);
    assert_eq!(MemoryEntryType::parse("preference"), None);
}

#[test]
fn as_str_roundtrips_through_parse() {
    for ty in [
        MemoryEntryType::User,
        MemoryEntryType::Feedback,
        MemoryEntryType::Project,
        MemoryEntryType::Reference,
    ] {
        assert_eq!(MemoryEntryType::parse(ty.as_str()), Some(ty));
    }
}
