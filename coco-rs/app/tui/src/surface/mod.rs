//! Native-scrollback surface substrate.

// The pure paint engine (terminal, history_insert, history_reflow,
// compatibility) lives in `coco_tui_ui::engine`; the transcript pipeline
// logic (commit boundary, committed renderer, stream splitter, anchored
// finalize) lives in `crate::transcript`. This module keeps the per-frame
// drivers that connect the two: live-tail preparation, finalized-history
// emission state, viewport/modal planning, and terminal I/O.
pub(crate) mod agent_view;
pub(crate) mod controller;
pub(crate) mod history_driver;
pub(crate) mod line_fingerprint;
pub(crate) mod modal;
pub(crate) mod stream;
pub(crate) mod viewport;
