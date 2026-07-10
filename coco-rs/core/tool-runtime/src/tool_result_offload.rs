//! Unified recoverable offload policy — head+tail window view, inline
//! budgets, and the Level-2 per-message aggregate budget.
//!
//! This is the policy layer shared by every over-budget tool result:
//! WebFetch, Bash, Level-1 spill and Level-2 eviction. It builds strictly on
//! [`crate::tool_result_storage`] (write mechanics: [`ToolOutputStore`],
//! [`ArtifactKey`]) — the dependency is offload → storage, never back.
//!
//! - [`WindowedView`] — a pure head+tail window computation (zero I/O, zero
//!   allocation), unit-testable in isolation.
//! - [`offload_windowed`] — window + persist + render the recoverable
//!   reference. A missing store degrades to a pointerless window, never an
//!   error.
//! - [`apply_tool_result_budget`] — the Level-2 per-message aggregate budget.
//! - [`scaled_per_message_bytes`] — the window-scaled Level-2 cap.
//!
//! The model-visible layout of a windowed result is **normative**: head,
//! then an omission marker line, then tail, then the `<persisted-output>`
//! reference block. The tag wraps ONLY the trailing reference block — the
//! text intentionally does not start with the tag so windowed output stays
//! eligible for Level-2 accounting (see `is_content_already_persisted`).

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use coco_utils_string::format_thousands;
use coco_utils_string::take_bytes_at_char_boundary;
use coco_utils_string::take_last_bytes_at_char_boundary;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::RwLock;

use coco_types::persisted_output::PERSISTED_OUTPUT_CLOSING_TAG;
use coco_types::persisted_output::PERSISTED_OUTPUT_TAG;
use coco_types::persisted_output::is_content_already_persisted;
pub use coco_types::persisted_output::is_pointer_bearing;

use crate::tool_result_storage::ArtifactKey;
use crate::tool_result_storage::ToolOutputStore;

/// Bytes reserved for the reference footer on top of an inline budget, so
/// `budget + footer` never crosses the tool's Level-1 threshold (the
/// no-double-persist invariant).
pub const FOOTER_RESERVE: i64 = 1_000;

/// Hard-wrap width for persisted artifact text: every line is capped at this
/// many bytes (at char boundaries) BEFORE windowing + persisting, so footer
/// line numbers and artifact line numbers agree by construction and a
/// `Read offset=… limit=200` slice stays ≈ ≤ 80KB — under Read's caps even
/// for single-line JSON or minified pages.
pub const HARD_WRAP_WIDTH: usize = 400;

/// Fixed limit for the number of lines the suggested Read call requests.
pub const READ_PAGE_LINES: i64 = 200;

/// Default per-message aggregate cap (used when the model window is unknown,
/// and as the ceiling of [`scaled_per_message_bytes`]).
pub const DEFAULT_MAX_PER_MESSAGE_BYTES: i64 = 200_000;

/// Inline byte budget for a single result. Newtype over `i64` (repo integer
/// convention); converted to `usize` only at slicing sites.
///
/// Two constructors, split by caller intent:
///  - [`Self::try_new`] — for CONFIG resolution: invalid (`<= 0`) yields
///    `None` and the caller keeps its default (matches the repo's
///    `.filter(|v| *v > 0)` convention; garbage is rejected, not clamped).
///  - [`Self::from_request`] — for MODEL-supplied per-call params, where
///    failing the tool call over a budget argument would be wrong — clamps
///    into `[MIN_REQUEST, MAX_REQUEST]`.
///
/// The type-level ceiling is a sanity bound only. The BINDING ceiling is
/// applied at each offload call site via [`Self::capped_to`]:
/// `effective = min(requested, configured, declared_bound - FOOTER_RESERVE)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineBudget(i64);

impl InlineBudget {
    /// Lower sanity bound for model-supplied budgets.
    pub const MIN_REQUEST: i64 = 2_000;
    /// Upper sanity bound for model-supplied budgets.
    pub const MAX_REQUEST: i64 = 500_000;

    /// Const constructor for compile-time constants (e.g. [`REFERENCE_BUDGET`]).
    /// Panics in `const` evaluation if `n <= 0`.
    pub const fn new(n: i64) -> Self {
        assert!(n > 0, "InlineBudget requires a positive byte budget");
        Self(n)
    }

    /// Fallible constructor for config resolution. Rejects `<= 0`.
    pub const fn try_new(n: i64) -> Option<Self> {
        if n > 0 { Some(Self(n)) } else { None }
    }

    /// Clamping constructor for model-supplied per-call params.
    pub fn from_request(n: i64) -> Self {
        Self(n.clamp(Self::MIN_REQUEST, Self::MAX_REQUEST))
    }

    /// Byte budget as `i64`.
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Byte budget as `usize` (slicing sites).
    pub const fn bytes(self) -> usize {
        self.0 as usize
    }

    /// Bind this budget under a tool's declared Level-1 threshold: the
    /// effective inline budget is `min(self, threshold - FOOTER_RESERVE)`,
    /// floored at [`Self::MIN_REQUEST`] so a pathologically small threshold
    /// still leaves room for head+tail. This is what makes the
    /// no-double-persist invariant structural (only breachable by a tool
    /// declaring a bound under `MIN_REQUEST + FOOTER_RESERVE` = 3_000 bytes —
    /// none exists).
    pub fn capped_to(self, threshold: i64) -> Self {
        let ceiling = (threshold - FOOTER_RESERVE).max(Self::MIN_REQUEST);
        Self(self.0.min(ceiling))
    }
}

/// Fixed budget for Level-1 spill references and Level-2 eviction
/// replacements (≈ the historical 2KB preview, now split ~3k head + 1k tail).
/// Deliberately small and CONSTANT: Level-2 convergence requires each
/// replacement to free nearly the entire candidate, and replacements are
/// re-sent every turn. Tool-level inline budgets (WebFetch, Bash) apply only
/// to a tool's OWN inline output, never to reference renders.
pub const REFERENCE_BUDGET: InlineBudget = InlineBudget::new(4_000);

/// Pure computation result of a head+tail window. Borrows slices of the
/// original — no allocation, no I/O — unit-testable in isolation.
///
/// Invariant: `head` is a prefix of the source, `tail` is a suffix, and
/// `head.len() + tail.len() <= source.len()` (they never overlap). The
/// omitted middle is exactly `source[head.len() .. source.len() - tail.len()]`.
///
/// Line-number contract (both sides are CONSERVATIVE — the reported omitted
/// range may overlap what is shown, but never leaves an unreported gap):
/// - head ends mid-line → `omitted_start_line` is that partial line (the
///   pointer re-reads it, so its unseen remainder is covered);
/// - tail starts mid-line → `omitted_end_line` INCLUDES that partial line
///   (its unseen first half is covered), and `tail_start_line` equals it.
#[derive(Debug, PartialEq, Eq)]
pub struct WindowedView<'a> {
    pub head: &'a str,
    pub tail: &'a str,
    /// Byte length of the text the window was computed from (== the text that
    /// gets persisted).
    pub total_bytes: usize,
    /// Number of lines (`'\n'` count + 1) of that same text.
    pub total_lines: usize,
    /// 1-indexed first line of the omitted middle:
    /// `omitted_start_line = count('\n' in head) + 1`.
    pub omitted_start_line: usize,
    /// 1-indexed last omitted line. When the tail starts exactly at a line
    /// boundary this is `first tail line - 1`; when the tail starts mid-line
    /// it is the partial line itself (conservative — see struct docs).
    pub omitted_end_line: usize,
    /// 1-indexed line on which the tail begins (possibly mid-line).
    pub tail_start_line: usize,
}

impl<'a> WindowedView<'a> {
    /// Returns `None` when content fits the budget (whole-inline zero-cost
    /// path — caller returns the content unchanged).
    ///
    /// Split: 75% head + 25% tail. UTF-8 safety via the repo's blessed
    /// primitives, then snapped to line boundaries: head retreats to its last
    /// `'\n'`, tail advances past its first `'\n'`. Snapping happens only when
    /// it costs less than half the respective sub-budget; head and tail ranges
    /// never overlap (`tail_start` is clamped to `>= head_end`).
    pub fn compute(content: &'a str, budget: InlineBudget) -> Option<Self> {
        let total_bytes = content.len();
        let budget_bytes = budget.bytes();
        if total_bytes <= budget_bytes {
            return None;
        }

        let head_budget = budget_bytes * 3 / 4;
        let tail_budget = budget_bytes - head_budget;

        // Byte-safe raw cuts, then line-boundary snapping.
        let mut head_end = take_bytes_at_char_boundary(content, head_budget).len();
        if let Some(nl) = content.as_bytes()[..head_end]
            .iter()
            .rposition(|&b| b == b'\n')
        {
            let snapped = nl + 1;
            if head_end - snapped < head_budget / 2 {
                head_end = snapped;
            }
        }

        let mut tail_start =
            total_bytes - take_last_bytes_at_char_boundary(content, tail_budget).len();
        if let Some(rel) = content.as_bytes()[tail_start..]
            .iter()
            .position(|&b| b == b'\n')
        {
            let advanced = tail_start + rel + 1;
            if advanced - tail_start < tail_budget / 2 && advanced <= total_bytes {
                tail_start = advanced;
            }
        }
        // Defensive: never overlap.
        tail_start = tail_start.max(head_end);

        let head = &content[..head_end];
        let tail = &content[tail_start..];

        let total_lines = count_newlines(content) + 1;
        let omitted_start_line = count_newlines(head) + 1;
        // Conservative tail accounting: a tail that begins mid-line leaves the
        // first half of that line unseen, so the omitted range must INCLUDE
        // the partial line — otherwise the suggested Read range would never
        // recover those bytes.
        let tail_at_line_start = tail_start == 0 || content.as_bytes()[tail_start - 1] == b'\n';
        let full_lines_before_tail = count_newlines(&content[..tail_start]);
        let omitted_end_line = full_lines_before_tail + usize::from(!tail_at_line_start);
        let tail_start_line = full_lines_before_tail + 1;

        Some(Self {
            head,
            tail,
            total_bytes,
            total_lines,
            omitted_start_line,
            omitted_end_line,
            tail_start_line,
        })
    }
}

/// Offload output. `stored_path = None` in two distinguishable situations,
/// both degrading to "window without pointer" and model-visible in the footer
/// ("full text not saved — persistence unavailable"):
///  - no store (forks with transcript disabled, SDK embeddings, tests),
///  - store present but the write failed (best-effort, warn-logged).
///
/// A missing store must NEVER become a tool error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffloadedText {
    pub model_text: String,
    pub stored_path: Option<PathBuf>,
    pub was_windowed: bool,
}

/// Offload `content` into a head+tail window plus a persisted artifact.
///
/// Free function (not a [`ToolOutputStore`] method) so it is callable with
/// `store = None`. Steps:
///  1. Hard-wrap the content at [`HARD_WRAP_WIDTH`] (Read-navigability).
///  2. Compute the window. If it fits the budget, return the wrapped text
///     verbatim (no persistence, `was_windowed = false`).
///  3. Otherwise persist the wrapped text (atomic for `Named` keys) and render
///     head + omission marker + tail + `<persisted-output>` footer.
pub async fn offload_windowed(
    store: Option<&ToolOutputStore>,
    key: &ArtifactKey,
    content: &str,
    budget: InlineBudget,
) -> OffloadedText {
    let wrapped = hard_wrap(content, HARD_WRAP_WIDTH);

    let Some(view) = WindowedView::compute(&wrapped, budget) else {
        return OffloadedText {
            model_text: wrapped.into_owned(),
            stored_path: None,
            was_windowed: false,
        };
    };

    // Persist the exact in-memory wrapped string — never re-read from disk.
    // Write failures degrade to a pointerless window; warn so silent
    // persistence loss is diagnosable.
    let stored_path = match store {
        Some(store) => match store.write_artifact(key, &wrapped).await {
            Ok(path) => Some(path),
            Err(error) => {
                tracing::warn!(
                    %error,
                    artifact = ?key,
                    "tool-result artifact write failed; emitting pointerless window"
                );
                None
            }
        },
        None => None,
    };

    let model_text = render_windowed(&view, stored_path.as_deref());
    OffloadedText {
        model_text,
        stored_path,
        was_windowed: true,
    }
}

/// Render the normative windowed layout: head, omission marker, tail, then the
/// `<persisted-output>` footer (the ONLY part wrapped by the tag).
pub(crate) fn render_windowed(view: &WindowedView<'_>, stored: Option<&Path>) -> String {
    let head_end_line = view.omitted_start_line.saturating_sub(1);

    let mut buf = String::with_capacity(view.head.len() + view.tail.len() + 512);
    buf.push_str(view.head);
    buf.push_str("\n\n[... middle omitted — see footer ...]\n\n");
    buf.push_str(view.tail);
    buf.push_str("\n\n");
    buf.push_str(PERSISTED_OUTPUT_TAG);
    buf.push('\n');
    buf.push_str(&format!(
        "Showing {} bytes (head, lines 1-{}) + {} bytes (tail, lines {}-{}) of {} bytes ({} lines). \
         Omitted: lines {}-{}.\n",
        format_thousands(view.head.len() as i64),
        format_thousands(head_end_line as i64),
        format_thousands(view.tail.len() as i64),
        format_thousands(view.tail_start_line as i64),
        format_thousands(view.total_lines as i64),
        format_thousands(view.total_bytes as i64),
        format_thousands(view.total_lines as i64),
        format_thousands(view.omitted_start_line as i64),
        format_thousands(view.omitted_end_line as i64),
    ));
    match stored {
        Some(path) => {
            let path = path.display();
            buf.push_str(&format!("Full text saved to: {path}\n"));
            buf.push_str(&format!(
                "To read the omitted middle: Read file_path=\"{path}\" offset={} limit={}\n",
                view.omitted_start_line, READ_PAGE_LINES
            ));
            buf.push_str("(raise offset to page onward)\n");
        }
        None => {
            buf.push_str("Full text not saved — persistence unavailable.\n");
        }
    }
    buf.push_str(PERSISTED_OUTPUT_CLOSING_TAG);
    buf
}

/// Level-2 budget scaled to the model window. Pure function, table-driven
/// tests. Models with `>= 200k`-token windows are byte-identical to the fixed
/// default; small-window models get proportional protection with a floor.
pub fn scaled_per_message_bytes(context_window_tokens: i64) -> i64 {
    /// Matches the budget-plan estimator.
    const BYTES_PER_TOKEN: i64 = 4;
    /// One turn's tool output should stay under this share of the window.
    const WINDOW_PCT: i64 = 30;
    /// Absolute floor so a tiny-window model still gets a workable budget.
    const FLOOR: i64 = 16_000;

    if context_window_tokens <= 0 {
        return DEFAULT_MAX_PER_MESSAGE_BYTES;
    }
    let scaled = context_window_tokens
        .saturating_mul(BYTES_PER_TOKEN)
        .saturating_mul(WINDOW_PCT)
        / 100;
    scaled.clamp(FLOOR, DEFAULT_MAX_PER_MESSAGE_BYTES)
}

/// Hard-wrap `content` so every line is at most `width` bytes, breaking at
/// char boundaries and preferring the last whitespace within the line.
/// Returns `Cow::Borrowed` unchanged when no line exceeds `width` (idempotent —
/// re-wrapping already-wrapped text is a no-op).
pub fn hard_wrap(content: &str, width: usize) -> Cow<'_, str> {
    if content.split('\n').all(|line| line.len() <= width) {
        return Cow::Borrowed(content);
    }
    let mut out = String::with_capacity(content.len() + content.len() / width + 16);
    let mut first = true;
    for line in content.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        wrap_line(line, width, &mut out);
    }
    Cow::Owned(out)
}

/// Break one logical line into `<= width`-byte chunks, appending `'\n'`
/// between chunks. Prefers to break at the last whitespace past the halfway
/// point; otherwise hard-breaks at the char boundary near `width`.
fn wrap_line(line: &str, width: usize, out: &mut String) {
    let mut rest = line;
    while rest.len() > width {
        let hard = take_bytes_at_char_boundary(rest, width).len().max(1);
        let cut = match rest.as_bytes()[..hard]
            .iter()
            .rposition(|&b| b == b' ' || b == b'\t')
        {
            Some(ws) if ws + 1 >= width / 2 => ws + 1,
            _ => hard,
        };
        out.push_str(&rest[..cut]);
        out.push('\n');
        rest = &rest[cut..];
    }
    out.push_str(rest);
}

fn count_newlines(s: &str) -> usize {
    s.as_bytes().iter().filter(|&&b| b == b'\n').count()
}

// ---------------------------------------------------------------------------
// Level 2 — per-message aggregate budget
// ---------------------------------------------------------------------------

/// Per-session content-replacement state for the Level-2 budget. Tracks:
///
/// - `replacements`: tool_use_id → replacement string. Keyed for prompt-cache
///   stability — same id always projects to the same replacement.
/// - `seen_ids`: tool_use_ids the budget has already considered. Once seen, a
///   result is "frozen" — never re-replaced even if it shrinks under the cap.
/// - `per_message_bytes`: budget cap. `i64::MAX` ⇒ feature off.
#[derive(Debug, Default, Clone)]
pub struct ContentReplacementState {
    pub replacements: HashMap<String, String>,
    pub seen_ids: std::collections::HashSet<String>,
    pub per_message_bytes: i64,
}

impl ContentReplacementState {
    pub fn new(per_message_bytes: i64) -> Self {
        Self {
            per_message_bytes,
            ..Default::default()
        }
    }

    pub fn is_active(&self) -> bool {
        self.per_message_bytes != i64::MAX
    }
}

/// Shared handle for engine wiring.
pub type ContentReplacementStateRef = Arc<RwLock<ContentReplacementState>>;

/// One tool-result candidate for budget evaluation. Caller projects from its
/// message representation. The runtime consumes a flat list because the
/// engine's message types live in a higher layer.
#[derive(Debug, Clone)]
pub struct ToolResultCandidate {
    pub tool_use_id: String,
    pub content: String,
    pub content_bytes: i64,
    /// Tool name when known. `None` ⇒ apply Level 2 only.
    pub tool_name: Option<String>,
    /// Whether this candidate's tool opted out of persistence (declared
    /// [`crate::tool_result_storage::ResultSizeBound::Unbounded`]). When
    /// `true`, the budget pipeline skips it (canonical-content tools like
    /// `Read`).
    pub persistence_opted_out: bool,
    /// Whether the persisted file should use `.json` rather than `.txt`.
    pub is_json: bool,
}

/// A single tool-result replacement record.
///
/// Returned by [`apply_tool_result_budget`] as `BudgetOutcome.newly_replaced`
/// and persisted alongside the message log. Serializable for transcript
/// persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentReplacement {
    pub tool_use_id: String,
    pub replacement: String,
}

/// Type alias for transcript-persisted records. Identical layout to
/// [`ContentReplacement`].
pub type ContentReplacementRecord = ContentReplacement;

/// Outcome of running [`apply_tool_result_budget`].
#[derive(Debug, Clone, Default)]
pub struct BudgetOutcome {
    /// Tool-use IDs that got newly replaced this pass.
    pub newly_replaced: Vec<ContentReplacement>,
    /// Total bytes freed from the in-message aggregate.
    pub freed_bytes: i64,
}

/// Decide which fresh tool-result candidates to offload to fit the per-message
/// byte budget:
///
/// 1. Re-apply cached replacements from `state.replacements`.
/// 2. Compute aggregate size using cached replacement length for
///    already-replaced IDs and inline length for everything else.
/// 3. If aggregate exceeds the cap, offload largest fresh candidates through
///    the windowed reference at [`REFERENCE_BUDGET`], and store the exact
///    replacement.
/// 4. Mark every candidate as seen. Offload failures freeze without a
///    replacement so later turns make the same decision.
///
/// Eligibility: opted-out tools and prefix-persisted references never count
/// and never offload. Pointer-bearing windowed results (suffix footer) COUNT
/// toward the trigger total (they are real prompt bytes) but are never
/// re-offloaded — re-offloading one under the same `ToolUse` id would compute
/// footer numbers from the windowed text while the existing artifact holds the
/// full original (`create_new` keeps the first write), leaving a pointer that
/// describes the wrong bytes.
pub async fn apply_tool_result_budget(
    candidates: &[ToolResultCandidate],
    state: &ContentReplacementStateRef,
    session_dir: &Path,
) -> BudgetOutcome {
    let snapshot = {
        let state = state.read().await;
        if !state.is_active() {
            return BudgetOutcome::default();
        }
        (
            state.per_message_bytes,
            state.seen_ids.clone(),
            state.replacements.clone(),
        )
    };
    let (per_message_bytes, seen_ids, replacements) = snapshot;

    // Partition by prior decision. Already-replaced IDs are re-applied by the
    // caller and contribute 0 to the trigger total; `frozen` (seen, never
    // replaced) counts at full size; `fresh` is budgeted this pass.
    let mut fresh: Vec<&ToolResultCandidate> = Vec::new();
    let mut frozen_bytes: i64 = 0;
    for c in candidates {
        if replacements.contains_key(&c.tool_use_id) {
            // mustReapply — excluded from the trigger total.
        } else if seen_ids.contains(&c.tool_use_id) {
            frozen_bytes += c.content_bytes;
        } else {
            fresh.push(c);
        }
    }

    // A re-processed message has no fresh candidates; freeze and return.
    if fresh.is_empty() {
        let mut state = state.write().await;
        for c in candidates {
            state.seen_ids.insert(c.tool_use_id.clone());
        }
        return BudgetOutcome::default();
    }

    // Accountable = counts toward the trigger; eligible = may be offloaded.
    let accountable: Vec<&ToolResultCandidate> = fresh
        .iter()
        .copied()
        .filter(|c| !c.persistence_opted_out && !is_content_already_persisted(&c.content))
        .collect();
    let fresh_bytes: i64 = accountable.iter().map(|c| c.content_bytes).sum();
    let mut eligible: Vec<&ToolResultCandidate> = accountable
        .into_iter()
        .filter(|c| !is_pointer_bearing(&c.content))
        .collect();

    let mut still_over = frozen_bytes + fresh_bytes;
    if still_over <= per_message_bytes {
        let mut state = state.write().await;
        for c in candidates {
            state.seen_ids.insert(c.tool_use_id.clone());
        }
        return BudgetOutcome::default();
    }

    eligible.sort_by(|a, b| b.content_bytes.cmp(&a.content_bytes));

    let store = ToolOutputStore::new(session_dir);
    let mut outcome = BudgetOutcome::default();
    for cand in eligible {
        if still_over <= per_message_bytes {
            break;
        }
        let key = ArtifactKey::ToolUse {
            id: cand.tool_use_id.clone(),
            is_json: cand.is_json,
        };
        let offloaded = offload_windowed(Some(&store), &key, &cand.content, REFERENCE_BUDGET).await;
        // Only a windowed render frees meaningful bytes; a candidate that fits
        // REFERENCE_BUDGET returns verbatim and would free nothing.
        let freed = cand.content_bytes - offloaded.model_text.len() as i64;
        if !offloaded.was_windowed || freed <= 0 {
            continue;
        }
        still_over -= freed;
        outcome.freed_bytes += freed;
        outcome.newly_replaced.push(ContentReplacement {
            tool_use_id: cand.tool_use_id.clone(),
            replacement: offloaded.model_text,
        });
    }
    let mut state = state.write().await;
    for replacement in &outcome.newly_replaced {
        state.replacements.insert(
            replacement.tool_use_id.clone(),
            replacement.replacement.clone(),
        );
    }
    for c in candidates {
        state.seen_ids.insert(c.tool_use_id.clone());
    }
    outcome
}

#[cfg(test)]
#[path = "tool_result_offload.test.rs"]
mod tests;
