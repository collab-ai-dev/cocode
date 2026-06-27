use pretty_assertions::assert_eq;

use super::DIM_OFF;
use super::DIM_ON;
use super::render;

#[test]
fn render_matches_ts_format() {
    // chalk wraps the entire input in ONE SGR pair, so the wire bytes
    // are `\x1b[2m` + the multi-line body + `\x1b[22m`.
    let session_id = "f7a376f4-02f4-4773-b7f3-4100e5e76642";
    let out = render(session_id);
    let cli_bin_name = coco_config::constants::CLI_BIN_NAME;
    assert_eq!(
        out,
        format!(
            "{DIM_ON}\nResume this session with:\n{cli_bin_name} --resume {session_id}\n{DIM_OFF}"
        )
    );
}

#[test]
fn render_includes_session_id_verbatim() {
    // Custom titles, slashes, spaces should pass through unmolested —
    // the caller is responsible for quoting policy. A raw uuid is the
    // only currently legal input.
    let session_id = "session-with-dashes_and_underscores";
    let out = render(session_id);
    let cli_bin_name = coco_config::constants::CLI_BIN_NAME;
    assert!(out.contains(&format!("{cli_bin_name} --resume {session_id}")));
}
