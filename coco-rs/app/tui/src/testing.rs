//! Native-surface rendering helpers for integration tests.

use std::sync::Arc;

use coco_messages::AssistantContent;
use coco_messages::ReasoningContent;
use coco_messages::TextContent;
use coco_messages::ToolCallContent;
use coco_messages::create_assistant_message;
use coco_messages::create_tool_result_message;
use coco_messages::create_user_message_with_uuid;
use coco_types::TokenUsage;
use coco_types::ToolId;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use uuid::Uuid;

use crate::state::AppState;
use crate::state::ui::StreamingState;
use crate::surface::controller::NativeSurfaceController;
use crate::surface::modal::ModalSurfacePlacement;
use crate::surface::modal::ModalSurfaceState;
use crate::surface::viewport::interactive_viewport_desired_height;
use crate::terminal::NATIVE_VIEWPORT_MAX_HEIGHT;
use crate::terminal::native_viewport_area_with_max;
use crate::theme::Theme;
use crate::transcript::cells::RenderedCell;
use crate::transcript::derive::message_to_cells;
use crate::transcript::render::DEFAULT_MAX_REFLOW_ROWS;
use crate::transcript::render::HistoryLineRenderOptions;
use crate::transcript::render::HistoryReplayCache;
use crate::transcript::render::HistoryReplayCachePolicy;
use crate::transcript::render::render_replay_history_lines;
use crate::transcript::render::render_replay_history_lines_cached;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::engine::compatibility::TerminalCompatibility;
use coco_tui_ui::engine::terminal::SurfaceTerminal;
use coco_tui_ui::style::UiStyles;

#[derive(Debug, Default)]
pub struct NativeSurfaceTestState {
    modal_surface: ModalSurfaceState,
}

/// Benchmark harness for native finalized-history replay.
///
/// Lives behind the `testing` feature so Criterion benches can exercise the
/// app-owned replay renderer without widening the production surface API.
pub struct NativeReplayBench {
    cells: Vec<RenderedCell>,
    theme: Theme,
    content: NativeReplayBenchContent,
    cache: HistoryReplayCache,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeReplayBenchContent {
    Markdown,
    SyntaxCode,
    Mermaid,
    /// Realistic agent-session shape: thinking + text + tool call + tool
    /// result per turn. This is the dominant production transcript and the
    /// shape that historically bailed out of the replay cache entirely —
    /// the cached bench cases assert it HITS, guarding that regression.
    ToolHeavy,
}

impl NativeReplayBench {
    pub fn new(turns: usize) -> Self {
        Self::new_with_content(turns, NativeReplayBenchContent::Markdown)
    }

    pub fn new_with_content(turns: usize, content: NativeReplayBenchContent) -> Self {
        let mut cells = Vec::with_capacity(turns * 2);
        for i in 0..turns {
            cells.extend(message_to_cells(Arc::new(create_user_message_with_uuid(
                Uuid::new_v4(),
                &format!("please inspect native replay case {i}"),
            ))));
            let assistant_text = match content {
                NativeReplayBenchContent::Markdown => {
                    format!("{BENCH_ASSISTANT_BLOCK}\n\nturn: {i}")
                }
                NativeReplayBenchContent::SyntaxCode => {
                    format!("{BENCH_SYNTAX_BLOCK}\n\nturn: {i}")
                }
                NativeReplayBenchContent::Mermaid => {
                    format!("{BENCH_MERMAID_BLOCK}\n\nturn: {i}")
                }
                NativeReplayBenchContent::ToolHeavy => {
                    format!("Inspecting replay case {i} with a tool pass.")
                }
            };
            let assistant_content = match content {
                NativeReplayBenchContent::ToolHeavy => vec![
                    AssistantContent::Reasoning(ReasoningContent::new(format!(
                        "Considering how to inspect case {i} before running the tool."
                    ))),
                    AssistantContent::Text(TextContent::new(assistant_text)),
                    AssistantContent::ToolCall(ToolCallContent::new(
                        format!("bench-call-{i}"),
                        "Grep",
                        serde_json::json!({"pattern": format!("case_{i}"), "path": "src/"}),
                    )),
                ],
                _ => vec![AssistantContent::Text(TextContent::new(assistant_text))],
            };
            cells.extend(message_to_cells(Arc::new(create_assistant_message(
                assistant_content,
                "bench-model",
                TokenUsage::default(),
            ))));
            if content == NativeReplayBenchContent::ToolHeavy {
                cells.extend(message_to_cells(Arc::new(create_tool_result_message(
                    &format!("bench-call-{i}"),
                    "Grep",
                    ToolId::Custom("bench".into()),
                    &format!(
                        "src/lib.rs:{i}:fn case_{i}() {{}}\nsrc/lib.rs:{}:fn other() {{}}",
                        i + 1
                    ),
                    /*is_error*/ false,
                ))));
            }
        }
        Self {
            cells,
            theme: Theme::default(),
            content,
            cache: HistoryReplayCache::default(),
        }
    }

    pub fn render_uncached(&self, width: u16) -> usize {
        render_replay_history_lines(
            &self.cells,
            replay_options(&self.theme, width, self.content),
            DEFAULT_MAX_REFLOW_ROWS,
        )
        .lines
        .len()
    }

    pub fn insert_uncached(&self, width: u16, height: u16) -> u16 {
        let replay = render_replay_history_lines(
            &self.cells,
            replay_options(&self.theme, width, self.content),
            DEFAULT_MAX_REFLOW_ROWS,
        );
        insert_replay_lines(width, height, replay.lines.iter().cloned())
    }

    pub fn render_cached(&mut self, width: u16) -> ReplayBenchOutput {
        let replay = render_replay_history_lines_cached(
            &self.cells,
            replay_options(&self.theme, width, self.content),
            DEFAULT_MAX_REFLOW_ROWS,
            &mut self.cache,
        );
        ReplayBenchOutput {
            lines: replay.lines.len(),
            rows: 0,
            cache_hit: replay.stats.cache_hit,
            finalized_render_calls: replay.stats.finalized_render_calls,
        }
    }

    pub fn insert_cached(&mut self, width: u16, height: u16) -> ReplayBenchOutput {
        let replay = render_replay_history_lines_cached(
            &self.cells,
            replay_options(&self.theme, width, self.content),
            DEFAULT_MAX_REFLOW_ROWS,
            &mut self.cache,
        );
        let rows = insert_replay_lines(width, height, replay.lines.iter().cloned());
        ReplayBenchOutput {
            lines: replay.lines.len(),
            rows,
            cache_hit: replay.stats.cache_hit,
            finalized_render_calls: replay.stats.finalized_render_calls,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReplayBenchOutput {
    pub lines: usize,
    pub rows: u16,
    pub cache_hit: bool,
    pub finalized_render_calls: usize,
}

/// Benchmark harness for the normal native surface redraw path.
pub struct NativeSurfaceNormalBench {
    state: AppState,
    terminal: SurfaceTerminal<TestBackend>,
    controller: NativeSurfaceController,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeSurfaceNormalBenchContent {
    Markdown,
    SyntaxCode,
    Mermaid,
    StreamNoNewline,
    StreamNewlineHeavy,
    StreamTable,
    Plain,
}

impl NativeSurfaceNormalBench {
    pub fn new(
        turns: usize,
        width: u16,
        height: u16,
        content: NativeSurfaceNormalBenchContent,
        streaming: bool,
    ) -> Self {
        let mut state = AppState::new();
        if matches!(content, NativeSurfaceNormalBenchContent::SyntaxCode) {
            state.ui.display_settings.syntax_highlighting = SyntaxHighlighting::Enabled;
        }
        push_bench_turns(&mut state, turns, content);
        if streaming {
            let mut live = StreamingState::new();
            live.append_text(streaming_text(content));
            live.reveal_all();
            state.ui.streaming = Some(live);
        }

        let backend = TestBackend::new(width, height);
        let mut terminal = SurfaceTerminal::new(backend).expect("test backend is infallible");
        terminal.set_viewport_area(Rect::new(
            0,
            height.saturating_sub(NATIVE_VIEWPORT_MAX_HEIGHT),
            width,
            NATIVE_VIEWPORT_MAX_HEIGHT.min(height),
        ));
        let mut controller = NativeSurfaceController::default();
        controller
            .draw(&mut terminal, &state)
            .expect("test backend is infallible");
        Self {
            state,
            terminal,
            controller,
        }
    }

    pub fn redraw_no_transcript_change(&mut self) -> usize {
        self.controller
            .draw(&mut self.terminal, &self.state)
            .expect("test backend is infallible");
        self.terminal.last_viewport_draw_stats().buffer_updates
            + self.state.session.transcript.len()
    }

    pub fn redraw_after_input_animation(&mut self) -> usize {
        self.state.ui.input.textarea.insert_str("x");
        self.redraw_no_transcript_change()
    }

    pub fn append_one_committed_message(
        &mut self,
        content: NativeSurfaceNormalBenchContent,
    ) -> usize {
        let turn = self.state.session.transcript.len();
        push_assistant_bench_message(&mut self.state, content, turn);
        self.controller
            .draw(&mut self.terminal, &self.state)
            .expect("test backend is infallible");
        self.terminal.last_history_insert_stats().wrapped_rows as usize
    }

    pub fn start_streaming_message(&mut self, content: NativeSurfaceNormalBenchContent) -> usize {
        let mut live = StreamingState::new();
        live.append_text(streaming_text(content));
        live.reveal_all();
        self.state.ui.streaming = Some(live);
        self.controller
            .draw(&mut self.terminal, &self.state)
            .expect("test backend is infallible");
        self.terminal.last_history_insert_stats().wrapped_rows as usize
    }

    pub fn finalize_streaming_message(
        &mut self,
        content: NativeSurfaceNormalBenchContent,
    ) -> usize {
        self.state.ui.streaming = None;
        let turn = self.state.session.transcript.len();
        push_assistant_bench_message(&mut self.state, content, turn);
        self.controller
            .draw(&mut self.terminal, &self.state)
            .expect("test backend is infallible");
        self.terminal.last_history_insert_stats().wrapped_rows as usize
    }
}

const BENCH_ASSISTANT_BLOCK: &str = "\
Here is a concise replay benchmark response with enough text to wrap across
typical terminal widths and enough Markdown to exercise the committed renderer.

```rust
fn replay_width(width: u16) -> u16 {
    width.saturating_sub(4)
}
```

- cache key includes width and display settings
- cache value stores finalized lines behind Arc
- replay still clones lines before terminal insertion";

const BENCH_SYNTAX_BLOCK: &str = "\
The replay contains fenced Rust code with syntax highlighting enabled.

```rust
pub fn replay_width(width: u16) -> u16 {
    width.saturating_sub(4)
}

pub fn render_label(index: usize) -> String {
    format!(\"case-{index}\")
}
```";

const BENCH_MERMAID_BLOCK: &str = "\
```mermaid
flowchart LR
  A[Start] --> B{Cache hit?}
  B -->|yes| C[Clone lines]
  B -->|no| D[Render markdown]
```";

const BENCH_STREAM_NO_NEWLINE: &str = "\
This is one long streaming paragraph with no newline terminator. It stays in the mutable tail so redraw cost should be bounded by the visible tail instead of repeatedly reparsing the whole committed transcript while the spinner is ticking and the input/status rows animate.";

const BENCH_STREAM_NEWLINE_HEAVY: &str = "\
line 000: stable stream row
line 001: stable stream row
line 002: stable stream row
line 003: stable stream row
line 004: stable stream row
line 005: stable stream row
line 006: stable stream row
line 007: stable stream row
line 008: stable stream row
line 009: mutable stream row";

const BENCH_STREAM_TABLE: &str = "\
Before the table.

| key | value |
| --- | ----- |
| alpha | one |
| beta | two |
| gamma | three |
";

fn push_bench_turns(state: &mut AppState, turns: usize, content: NativeSurfaceNormalBenchContent) {
    for i in 0..turns {
        let user = create_user_message_with_uuid(
            Uuid::new_v4(),
            &format!("please inspect native surface case {i}"),
        );
        state.session.transcript.on_message_appended(Arc::new(user));
        push_assistant_bench_message(state, content, i);
    }
}

fn push_assistant_bench_message(
    state: &mut AppState,
    content: NativeSurfaceNormalBenchContent,
    i: usize,
) {
    let block = streaming_text(content);
    let msg = create_assistant_message(
        vec![AssistantContent::Text(TextContent::new(format!(
            "{block}\n\nturn: {i}"
        )))],
        "bench-model",
        TokenUsage::default(),
    );
    state.session.transcript.on_message_appended(Arc::new(msg));
}

fn streaming_text(content: NativeSurfaceNormalBenchContent) -> &'static str {
    match content {
        NativeSurfaceNormalBenchContent::Markdown => BENCH_ASSISTANT_BLOCK,
        NativeSurfaceNormalBenchContent::SyntaxCode => BENCH_SYNTAX_BLOCK,
        NativeSurfaceNormalBenchContent::Mermaid => BENCH_MERMAID_BLOCK,
        NativeSurfaceNormalBenchContent::StreamNoNewline => BENCH_STREAM_NO_NEWLINE,
        NativeSurfaceNormalBenchContent::StreamNewlineHeavy => BENCH_STREAM_NEWLINE_HEAVY,
        NativeSurfaceNormalBenchContent::StreamTable => BENCH_STREAM_TABLE,
        NativeSurfaceNormalBenchContent::Plain => "plain append line\n\nsecond plain append line",
    }
}

pub fn clear_native_replay_markdown_memo() {
    crate::transcript::render::clear_committed_markdown_memo_for_tests();
}

fn replay_options(
    theme: &Theme,
    width: u16,
    content: NativeReplayBenchContent,
) -> HistoryLineRenderOptions<'_> {
    let syntax_highlighting = match content {
        NativeReplayBenchContent::Markdown | NativeReplayBenchContent::ToolHeavy => {
            SyntaxHighlighting::Disabled
        }
        NativeReplayBenchContent::SyntaxCode | NativeReplayBenchContent::Mermaid => {
            SyntaxHighlighting::Enabled
        }
    };
    HistoryLineRenderOptions {
        styles: UiStyles::new(theme),
        width,
        syntax_highlighting,
        show_system_reminders: false,
        show_thinking: false,
        cwd: None,
        kb_handle: None,
        replay_cache_policy: HistoryReplayCachePolicy::default(),
        reasoning_metadata: None,
        subagent_summaries: None,
    }
}

fn insert_replay_lines(
    width: u16,
    height: u16,
    lines: impl IntoIterator<Item = ratatui::text::Line<'static>>,
) -> u16 {
    let rows = coco_tui_ui::engine::history_insert::render_history_rows(
        lines.into_iter().collect(),
        width,
    );
    let backend = TestBackend::new(width, height);
    let mut terminal = SurfaceTerminal::new(backend).expect("test backend is infallible");
    terminal.set_viewport_area(Rect::new(0, height.saturating_sub(1), width, height.min(1)));
    terminal
        .insert_history_rows(&rows)
        .expect("test backend is infallible")
}

/// Render `state` through the native-scrollback surface into a string.
///
/// This mirrors the production `Tui::draw` surface path closely enough for
/// integration tests while keeping raw-mode and crossterm stdin ownership out
/// of test binaries.
pub fn render_native_surface_to_string(state: &AppState, width: u16, height: u16) -> String {
    let mut surface_state = NativeSurfaceTestState::default();
    render_native_surface_to_string_with_surface_state(state, width, height, &mut surface_state)
}

/// Render with caller-owned state surface state so tests can exercise
/// production placement latching across multiple frames.
pub fn render_native_surface_to_string_with_surface_state(
    state: &AppState,
    width: u16,
    height: u16,
    surface_state: &mut NativeSurfaceTestState,
) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = SurfaceTerminal::new(backend).expect("test backend is infallible");
    let size = Size { width, height };
    let plan = surface_state.modal_surface.plan_for_native_viewport(
        state,
        TerminalCompatibility::NativeScrollback,
        std::time::Instant::now(),
        width,
        NATIVE_VIEWPORT_MAX_HEIGHT,
    );
    let area = match plan.modal_placement {
        Some(ModalSurfacePlacement::AltScreen) => Rect::new(0, 0, width, height),
        _ => {
            let desired_height = interactive_viewport_desired_height(
                state,
                width,
                NATIVE_VIEWPORT_MAX_HEIGHT,
                plan,
                None,
            );
            native_viewport_area_with_max(
                terminal.history_bottom_y(),
                size,
                desired_height,
                NATIVE_VIEWPORT_MAX_HEIGHT,
            )
        }
    };
    terminal.set_viewport_area(area);

    let mut controller = NativeSurfaceController::default();
    controller
        .draw_with_plan(&mut terminal, state, plan, None)
        .expect("test backend is infallible");

    buffer_to_string(terminal.backend().buffer())
}

fn buffer_to_string(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}
