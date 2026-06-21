use pretty_assertions::assert_eq;

use super::render;

#[test]
fn render_matches_ts_format() {
    // chalk wraps the entire input in ONE SGR pair, so the wire bytes
    // are `\x1b[2m` + the multi-line body + `\x1b[22m`.
    let out = render("f7a376f4-02f4-4773-b7f3-4100e5e76642");
    assert_eq!(
        out,
        "\x1b[2m\nResume this session with:\ncoco-cli --resume f7a376f4-02f4-4773-b7f3-4100e5e76642\n\x1b[22m"
    );
}

#[test]
fn render_includes_session_id_verbatim() {
    // Custom titles, slashes, spaces should pass through unmolested —
    // the caller is responsible for quoting policy. A raw uuid is the
    // only currently legal input.
    let out = render("session-with-dashes_and_underscores");
    assert!(out.contains("coco-cli --resume session-with-dashes_and_underscores"));
}
