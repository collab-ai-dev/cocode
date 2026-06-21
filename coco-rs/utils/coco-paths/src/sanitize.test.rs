use super::*;
use pretty_assertions::assert_eq;

#[test]
fn sanitize_replaces_non_alnum_with_dash() {
    // `/Users/foo/project`.replace(/[^a-zA-Z0-9]/g,'-')
    //   → `-Users-foo-project`
    assert_eq!(sanitize_path("/Users/foo/project"), "-Users-foo-project");
}

#[test]
fn sanitize_matches_observed_ts_slug_on_this_machine() {
    // The literal directory observed at
    // `~/.coco/projects/-Users-linyuzhi-codespace-myagent-codex/`.
    // Our slug for the same cwd MUST be identical — this is the bug
    // the legacy `memory::sanitize_project_path` introduced.
    assert_eq!(
        sanitize_path("/Users/linyuzhi/codespace/myagent/codex"),
        "-Users-linyuzhi-codespace-myagent-codex",
    );
}

#[test]
fn sanitize_punctuation_underscores_dashes_all_become_dash() {
    // Dots, underscores, dashes are NOT `[a-zA-Z0-9]` so they all
    // collapse to a single `-` each. (Note no run-collapsing — TS
    // doesn't collapse either.)
    assert_eq!(
        sanitize_path("/path/with.dots_and-dashes"),
        "-path-with-dots-and-dashes",
    );
}

#[test]
fn sanitize_empty_input_returns_empty() {
    assert_eq!(sanitize_path(""), "");
}

#[test]
fn sanitize_pure_alnum_passes_through() {
    assert_eq!(sanitize_path("abcXYZ123"), "abcXYZ123");
}

#[test]
fn sanitize_truncates_at_max_and_appends_djb2_suffix() {
    let input = "a".repeat(250);
    let out = sanitize_path(&input);
    assert!(
        out.starts_with(&"a".repeat(MAX_SANITIZED_LENGTH)),
        "prefix should be 200 'a's: {out}",
    );
    // The `-` joining the prefix and the hash sits at byte index 200.
    assert_eq!(&out[MAX_SANITIZED_LENGTH..MAX_SANITIZED_LENGTH + 1], "-");
    assert!(
        out.len() > MAX_SANITIZED_LENGTH + 1,
        "must include hash bytes after the joining dash: {out}",
    );
}

#[test]
fn sanitize_bmp_unicode_each_codeunit_becomes_dash() {
    // `中`, `文` are single UTF-16 code units (BMP); each is one `-`.
    // `/` is two `/` characters → two `-`s.
    assert_eq!(sanitize_path("/中文/x"), "----x");
}

#[test]
fn sanitize_emoji_surrogate_pair_becomes_two_dashes() {
    // 😀 (U+1F600) is encoded as a UTF-16 surrogate pair. TS regex
    // visits each surrogate independently → two `-`s. We must match.
    assert_eq!(sanitize_path("a😀b"), "a--b");
}

#[test]
fn sanitize_agent_type_replaces_only_colon() {
    assert_eq!(
        sanitize_agent_type_for_path("my-plugin:my-agent"),
        "my-plugin-my-agent",
    );
    assert_eq!(sanitize_agent_type_for_path("plain-agent"), "plain-agent",);
    // Confirm dots survive — agent types may carry them legitimately.
    assert_eq!(sanitize_agent_type_for_path("foo.bar"), "foo.bar",);
}
