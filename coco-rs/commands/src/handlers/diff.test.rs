use super::*;

#[test]
fn test_count_stat_files_typical() {
    let stat = " src/main.rs | 10 +++---\n src/lib.rs  |  5 ++\n 2 files changed, 12 insertions(+), 3 deletions(-)";
    assert_eq!(count_stat_files(stat), 2);
}

#[test]
fn test_count_stat_files_single() {
    let stat = " src/main.rs | 3 +++\n 1 file changed, 3 insertions(+)";
    assert_eq!(count_stat_files(stat), 1);
}

#[test]
fn test_count_stat_files_empty() {
    assert_eq!(count_stat_files(""), 0);
    assert_eq!(count_stat_files("no changes"), 0);
}

#[test]
fn test_append_truncated_diff_short() {
    let mut out = String::new();
    append_truncated_diff(&mut out, "line1\nline2\nline3");
    assert_eq!(out, "line1\nline2\nline3");
}

#[test]
fn test_append_truncated_diff_empty() {
    let mut out = String::new();
    append_truncated_diff(&mut out, "   ");
    assert!(out.is_empty());
}

#[test]
fn test_append_truncated_diff_long() {
    let mut out = String::new();
    // Create a diff longer than MAX_DIFF_CHARS
    let long_diff: String = (0..1000)
        .map(|i| format!("line {i}: some content here that takes up space\n"))
        .collect();
    assert!(long_diff.len() > MAX_DIFF_CHARS);

    append_truncated_diff(&mut out, &long_diff);
    assert!(out.contains("truncated"));
    assert!(out.len() < long_diff.len());
}

#[tokio::test]
async fn test_diff_handler_in_git_repo() {
    // This test runs in the actual repo, so git should work
    let output = handler(String::new()).await.unwrap();
    // Should either show changes or say working tree is clean
    assert!(
        output.contains("clean")
            || output.contains("changes")
            || output.contains("Staged")
            || output.contains("Unstaged")
            || output.contains("Untracked"),
        "unexpected output: {output}"
    );
}

#[tokio::test]
async fn test_diff_handler_with_custom_args() {
    let output = handler("--name-only".to_string()).await.unwrap();
    // Should not panic, should produce some output
    assert!(!output.is_empty());
}
