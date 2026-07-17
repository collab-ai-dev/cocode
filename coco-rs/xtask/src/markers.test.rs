use super::*;
use pretty_assertions::assert_eq;

const DOC: &str = "# Title\n\nProse above.\n\n<!-- BEGIN GENERATED: demo -->\n\nold body\n\n<!-- END GENERATED: demo -->\n\nProse below.\n";

#[test]
fn test_splice_replaces_only_marker_body() {
    let out = splice(DOC, "demo", "new body", "demo.md").expect("splice must succeed");
    assert_eq!(
        out,
        "# Title\n\nProse above.\n\n<!-- BEGIN GENERATED: demo -->\n\nnew body\n\n<!-- END GENERATED: demo -->\n\nProse below.\n"
    );
}

#[test]
fn test_splice_is_idempotent() {
    let once = splice(DOC, "demo", "new body", "demo.md").expect("first splice");
    let twice = splice(&once, "demo", "new body", "demo.md").expect("second splice");
    assert_eq!(once, twice);
}

#[test]
fn test_splice_preserves_multibyte_prose() {
    let doc = "序言 — 中文\n\n<!-- BEGIN GENERATED: demo -->\n\nx\n\n<!-- END GENERATED: demo -->\n\n尾巴 ─ 结束\n";
    let out = splice(doc, "demo", "y", "demo.md").expect("splice must succeed");
    assert_eq!(
        out,
        "序言 — 中文\n\n<!-- BEGIN GENERATED: demo -->\n\ny\n\n<!-- END GENERATED: demo -->\n\n尾巴 ─ 结束\n"
    );
}

#[test]
fn test_splice_missing_marker_errors() {
    let err = splice("no markers here", "demo", "x", "demo.md").expect_err("must error");
    assert!(err.to_string().contains("found 0 begin / 0 end"), "{err}");
}

#[test]
fn test_splice_duplicate_marker_errors() {
    let doc = format!("{DOC}{DOC}");
    let err = splice(&doc, "demo", "x", "demo.md").expect_err("must error");
    assert!(err.to_string().contains("found 2 begin / 2 end"), "{err}");
}

#[test]
fn test_splice_unbalanced_marker_errors() {
    let doc = "<!-- BEGIN GENERATED: demo -->\n\nbody\n";
    let err = splice(doc, "demo", "x", "demo.md").expect_err("must error");
    assert!(err.to_string().contains("found 1 begin / 0 end"), "{err}");
}

#[test]
fn test_splice_reversed_markers_error() {
    let doc = "<!-- END GENERATED: demo -->\n\nbody\n\n<!-- BEGIN GENERATED: demo -->\n";
    let err = splice(doc, "demo", "x", "demo.md").expect_err("must error");
    assert!(err.to_string().contains("appears before"), "{err}");
}
