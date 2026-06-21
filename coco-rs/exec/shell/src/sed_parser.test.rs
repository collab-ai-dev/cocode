use super::*;

#[test]
fn test_is_sed_in_place() {
    assert!(is_sed_in_place_edit("sed -i 's/old/new/' file.txt"));
    assert!(is_sed_in_place_edit("sed --in-place 's/a/b/' x.rs"));
    assert!(!is_sed_in_place_edit("sed 's/old/new/' file.txt"));
    assert!(!is_sed_in_place_edit("grep -i pattern file"));
}

#[test]
fn test_parse_basic_sed() {
    let info = parse_sed_edit_command("sed -i 's/old/new/g' file.txt").unwrap();
    assert_eq!(info.file_path, "file.txt");
    assert_eq!(info.pattern, "old");
    assert_eq!(info.replacement, "new");
    assert_eq!(info.flags, "g");
    assert!(!info.extended_regex);
}

#[test]
fn test_parse_extended_regex() {
    let info = parse_sed_edit_command("sed -i -E 's/foo+/bar/' test.rs").unwrap();
    assert_eq!(info.pattern, "foo+");
    assert_eq!(info.replacement, "bar");
    assert!(info.extended_regex);
}

#[test]
fn test_parse_no_in_place_returns_none() {
    assert!(parse_sed_edit_command("sed 's/old/new/' file.txt").is_none());
}

#[test]
fn test_parse_no_file_returns_none() {
    assert!(parse_sed_edit_command("sed -i 's/old/new/'").is_none());
}

#[test]
fn test_parse_expression_flag() {
    let info = parse_sed_edit_command("sed -i -e 's/a/b/' file.txt").unwrap();
    assert_eq!(info.pattern, "a");
    assert_eq!(info.replacement, "b");
}

#[test]
fn test_parse_different_delimiter() {
    let info = parse_sed_edit_command("sed -i 's|/usr/old|/usr/new|' paths.conf").unwrap();
    assert_eq!(info.pattern, "/usr/old");
    assert_eq!(info.replacement, "/usr/new");
}

// ── has_dangerous_sed (P5) ──

#[test]
fn test_has_dangerous_sed() {
    // Execute (`e`) is always dangerous, even in acceptEdits.
    assert!(has_dangerous_sed(
        "sed 's/.*/x/e' f",
        /*allow_file_writes*/ true
    ));
    assert!(has_dangerous_sed("sed -e 'e rm -rf /' f", true));
    // File write (`w` command / `s///w` flag): dangerous unless acceptEdits.
    assert!(has_dangerous_sed("sed 'w /etc/cron.d/x' f", false));
    assert!(has_dangerous_sed("sed 's/a/b/w /tmp/out' f", false));
    assert!(!has_dangerous_sed("sed 'w /tmp/out' f", true)); // acceptEdits allows writes
    // Benign substitution is never dangerous.
    assert!(!has_dangerous_sed("sed -i 's/old/new/g' f", false));
    assert!(!has_dangerous_sed("sed -n '1,10p' f", false));
    // Non-sed command → not dangerous.
    assert!(!has_dangerous_sed("echo hi", false));
}
