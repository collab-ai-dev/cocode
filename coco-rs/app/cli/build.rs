/// Embed build provenance (git short hash, commit date, commit subject, and
/// build timestamp) so `cocode --version` reports exactly which commit a binary
/// was built from and when. Git-derived values prefer the `COCO_BUILD_*`
/// overrides (CI release sets them for reproducible builds), then fall back to
/// `git`, then `"unknown"`. The components are composed into the multi-line
/// version string in lib.rs — `cargo:rustc-env` values cannot contain newlines.
fn main() {
    coco_utils_common::emit_cargo_build_provenance();
}
