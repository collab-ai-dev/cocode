//! Golden snapshot of the diff row-model parser.
//!
//! First insta coverage in `coco-tui-ui` — the pure paint/primitive crate
//! previously had none, so visual/structural regressions in its lowest-level
//! primitives were only caught by hand-written field asserts. This freezes the
//! structured classification + line-number tracking that the diff widget styles
//! and renders; a parser change shows up as a reviewable snapshot diff.
//!
//! Regenerate an intentional change with:
//!   INSTA_UPDATE=always cargo test -p coco-tui-ui --test diff_golden

#![allow(clippy::unwrap_used)]

use coco_tui_ui::diff::diff_line_views;

const SAMPLE_DIFF: &str = "\
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,4 +1,4 @@
 fn main() {
-    println!(\"hello\");
+    println!(\"hello, world\");
 }
";

#[test]
fn diff_row_model_is_frozen() {
    let rows = diff_line_views(SAMPLE_DIFF);
    insta::assert_debug_snapshot!("diff_row_model", rows);
}
