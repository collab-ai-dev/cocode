use crate::AssistantContent;
use crate::LlmMessage;
use crate::Message;
use crate::MessageKind;
use coco_types::ProviderModelSelection;
use coco_types::TokenUsage;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Anchor for the last successful API call.
///
/// Stored on [`MessageHistory`] so messages and their last-usage state
/// share a single source of truth. The walk-back formula is
/// `usage.total() + estimate(messages_since_anchor)` — see
/// [`MessageHistory::tokens_with_last_usage`].
///
/// Lifecycle: in-memory only; recreated on every successful API call
/// via [`MessageHistory::push_assistant_with_usage`]. Resume / `/clear`
/// / compaction / rewind / micro-compact / drain all invalidate via
/// the mutation methods on `MessageHistory`.
///
/// The internal anchor index is **not exposed** — callers consume the
/// marker only via [`MessageHistory::last_usage`] (for the typed
/// usage/model data) and [`MessageHistory::tokens_with_last_usage`]
/// (for the precision estimate). The marker has no public constructor;
/// the only path to create one is via the cohesive push method above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastUsageMarker {
    /// Total billed usage from the API call that produced the assistant
    /// message at the marker boundary (input + output + cache buckets).
    pub usage: TokenUsage,
    /// `(provider, model_id)` that produced this usage. Multi-model
    /// sessions: a `/model switch` after this marker overwrites it on
    /// the next successful call. Identity only — `ProviderApi` and
    /// `display_name` intentionally not carried.
    ///
    /// Currently read by `/context` overlay (P3 plan). Carried on the
    /// marker rather than re-derived because multi-model sessions may
    /// switch model after the anchor, and the historical model is what
    /// produced the historical usage.
    pub model: ProviderModelSelection,
    /// `messages.len()` at the moment the marker was anchored — the
    /// first index of the post-call tail. Private: callers cannot
    /// observe the raw index, only its effect via
    /// [`MessageHistory::tokens_with_last_usage`].
    anchor_count: usize,
}

/// In-memory message history with turn tracking.
///
/// Storage is `Vec<Arc<Message>>` so the engine's authoritative
/// transcript can be shared with wire emit payloads
/// (`ServerNotification::MessageAppended`,
/// `ServerNotification::HistoryReplaced`) and TUI derived cells
/// without deep-cloning the `Message` body on every transcript
/// mutation — see `engine-tui-unified-transcript-plan.md` §11 F8.
///
/// The backing storage is `pub(crate)` so internal helpers can do
/// bulk operations cheaply; external callers go through the
/// controlled API ([`push`](Self::push), [`push_arc`](Self::push_arc),
/// [`truncate`](Self::truncate), [`clear`](Self::clear),
/// [`iter`](Self::iter), [`as_slice`](Self::as_slice),
/// [`first`](Self::first), [`last`](Self::last), [`get`](Self::get))
/// or the explicit escape hatch
/// [`messages_mut`](Self::messages_mut) for raw in-place mutation
/// (which obligates the caller to call [`rebuild_index`](Self::rebuild_index)
/// and emit `MessageTruncated`/`MessageAppended` events).
///
/// Restricting direct field access enforces I-1 from
/// `docs/coco-rs/engine-tui-unified-transcript-plan.md`: every
/// transcript mutation must be observable via the wire-level event
/// stream so TUI / SDK consumers stay coherent with engine state.
#[derive(Debug, Default)]
pub struct MessageHistory {
    pub(crate) messages: Vec<Arc<Message>>,
    /// Message UUID -> index in messages vec.
    index: HashMap<Uuid, usize>,
    /// Active session id stamped onto every transcript-lifecycle
    /// emit (`MessageAppended`, `MessageTruncated`, `HistoryReplaced`,
    /// `SessionResetForResume`). AgentTeams (plan §11 F9) consumers
    /// read this off the wire to demux merged timelines. Empty for
    /// legacy paths; set via [`set_envelope`](Self::set_envelope).
    session_id: String,
    /// Active agent id — `None` for the main agent, `Some` for
    /// teammate / subagent emits. Forward-compat field for AgentTeams.
    agent_id: Option<String>,
    /// Last-usage anchor — see [`LastUsageMarker`] and
    /// [`Self::push_assistant_with_usage`]. `None` initially; populated
    /// after every successful API call; cleared by any mutation that
    /// invalidates the "base = previous total" invariant (compaction,
    /// clear, rewind, in-place body rewrite, drain, raw escape hatch).
    /// The internal `anchor_count` lives inside [`LastUsageMarker`] so
    /// invalidate is one field reset, not two.
    last_usage: Option<LastUsageMarker>,
}

impl MessageHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stamp the session + agent identity onto this history so every
    /// `history_sync::*` emit carries the envelope automatically.
    /// Called once at history construction by the engine builder; no
    /// other call site should need to touch it.
    pub fn set_envelope(&mut self, session_id: String, agent_id: Option<String>) {
        self.session_id = session_id;
        self.agent_id = agent_id;
    }

    /// Active session id. Empty when [`set_envelope`](Self::set_envelope)
    /// hasn't been called (legacy / test fixtures).
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Active agent id, if any.
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    /// Push a message into the history and return the shared
    /// [`Arc<Message>`] for emit reuse. The same Arc is stored in
    /// history and returned to the caller, so a downstream
    /// `MessageAppended` emit can ride the same allocation — no
    /// deep `Message` clone.
    pub fn push(&mut self, msg: Message) -> Arc<Message> {
        self.push_arc(Arc::new(msg))
    }

    /// Push an already-constructed [`Arc<Message>`] into the history
    /// and return it. Used by resume hydration paths that built the
    /// Arc upstream (e.g. JSONL load).
    ///
    /// **Last-usage invariant**: pure appends do NOT invalidate
    /// [`Self::last_usage`] — the anchor count stays < new len, and the
    /// tail slice grows naturally.
    pub fn push_arc(&mut self, arc: Arc<Message>) -> Arc<Message> {
        if let Some(uuid) = arc.uuid() {
            self.index.insert(*uuid, self.messages.len());
        }
        self.messages.push(Arc::clone(&arc));
        arc
    }

    /// Rebuild a history from shared transcript messages and preserve
    /// the latest assistant usage marker when the source was flattened
    /// to `Vec<Arc<Message>>`.
    pub fn from_arcs_preserving_latest_usage(messages: Vec<Arc<Message>>) -> Self {
        let latest_usage =
            messages
                .iter()
                .enumerate()
                .rev()
                .find_map(|(idx, arc)| match arc.as_ref() {
                    Message::Assistant(a) => a.usage.map(|usage| {
                        (
                            idx,
                            usage,
                            ProviderModelSelection {
                                provider: String::new(),
                                model_id: a.model.clone(),
                            },
                        )
                    }),
                    _ => None,
                });

        let mut history = Self::new();
        for (idx, arc) in messages.into_iter().enumerate() {
            if let Some((usage_idx, usage, model)) = latest_usage.as_ref()
                && idx == *usage_idx
            {
                history.push_arc_assistant_with_usage(arc, *usage, model.clone());
                continue;
            }
            history.push_arc(arc);
        }
        history
    }

    /// Raw mutable access to the backing message vector. **Bypasses
    /// the controlled API.** Callers MUST:
    ///
    /// 1. Call [`rebuild_index`](Self::rebuild_index) after any
    ///    structural change (push / insert / remove / truncate /
    ///    reorder).
    /// 2. Emit the matching `ServerNotification` events
    ///    (`MessageAppended` for new entries, `MessageTruncated` for
    ///    removals) so TUI / SDK consumers stay coherent.
    ///
    /// Reserved for in-place compaction passes (`coco_compact::*`)
    /// that operate on the vector representation and re-publish via
    /// `history_replace_and_emit` afterwards. New call sites should
    /// prefer [`push`](Self::push), [`truncate`](Self::truncate),
    /// [`clear`](Self::clear), or
    /// `coco_query::history_sync::history_push_and_emit`.
    ///
    /// **Last-usage invariant**: this is an escape hatch — the caller
    /// may rewrite bodies, reorder, or remove messages, any of which
    /// would invalidate the [`LastUsageMarker`]. The marker is cleared
    /// here unconditionally.
    pub fn messages_mut(&mut self) -> &mut Vec<Arc<Message>> {
        self.invalidate_last_usage();
        &mut self.messages
    }

    /// Read-only iterator over the underlying messages.
    pub fn iter(&self) -> std::slice::Iter<'_, Arc<Message>> {
        self.messages.iter()
    }

    /// First message, if any.
    pub fn first(&self) -> Option<&Arc<Message>> {
        self.messages.first()
    }

    /// Last message, if any.
    pub fn last(&self) -> Option<&Arc<Message>> {
        self.messages.last()
    }

    /// Indexed access; returns `None` if out of bounds.
    pub fn get(&self, idx: usize) -> Option<&Arc<Message>> {
        self.messages.get(idx)
    }

    /// Cloned snapshot of the underlying message Arcs.
    /// Cheap — each entry is an `Arc::clone` (pointer copy), not a
    /// deep `Message` clone.
    pub fn to_vec(&self) -> Vec<Arc<Message>> {
        self.messages.clone()
    }

    /// Shallow clone preserving usage-marker state.
    ///
    /// Intended for read-only analyzers that need a stable snapshot of
    /// history without holding the session lock across async work. Message
    /// bodies stay shared by `Arc`; marker and index metadata are cloned.
    pub fn snapshot(&self) -> Self {
        Self {
            messages: self.messages.clone(),
            index: self.index.clone(),
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
            last_usage: self.last_usage.clone(),
        }
    }

    /// Drain messages pushed since `since_len` and rebuild the UUID index.
    ///
    /// Used by the streaming agent loop to capture synthetic-error
    /// `tool_result` rows that the preparer pushed during a streamed
    /// `ToolCallEnd` so they can be re-committed *after* the assistant
    /// message lands — preserving Anthropic's `tool_use` / `tool_result`
    /// adjacency invariant (I1). Without this, an early-error tool call
    /// in the middle of a multi-tool stream would land at history index
    /// `N` while the assistant message lands at `N+1`, producing a
    /// malformed user→assistant ordering.
    ///
    /// The index is fully rebuilt because mid-vec drain shifts every
    /// later message's positional index and the `HashMap<Uuid, usize>`
    /// must point at the post-drain offsets (or contain no stale keys).
    /// Cost is O(N) over all remaining messages, but the streaming path
    /// only calls this on synthetic-error capture (rare) so the
    /// amortized impact is negligible.
    pub fn drain_pushed_since(&mut self, since_len: usize) -> Vec<Message> {
        if since_len >= self.messages.len() {
            return Vec::new();
        }
        // Conservatively invalidate the marker — drain re-orders the
        // vec and the anchor may have been at index > since_len.
        // Streaming-error path runs rarely; precision loss for one
        // turn is acceptable.
        self.invalidate_last_usage();
        // The drained Arcs are typically uniquely held (just popped out
        // of storage); try_unwrap returns the Message without cloning
        // when refcount is 1 and falls back to a deep clone otherwise.
        let captured: Vec<Message> = self
            .messages
            .drain(since_len..)
            .map(|a| Arc::try_unwrap(a).unwrap_or_else(|a| (*a).clone()))
            .collect();
        self.rebuild_index();
        captured
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn find_by_uuid(&self, uuid: &Uuid) -> Option<&Arc<Message>> {
        self.index.get(uuid).and_then(|&i| self.messages.get(i))
    }

    /// Get messages as a slice.
    pub fn as_slice(&self) -> &[Arc<Message>] {
        &self.messages
    }

    /// Return the text from the last Assistant message, if any.
    ///
    /// Walks the message content in emission order and emits text parts
    /// separated by `\n`, with `[tool: <name>]` placeholder lines for
    /// non-text parts. Preserves the tool-call boundary so consumers
    /// (Stop hook input, memory extraction, etc.) see the structural
    /// transitions instead of a silently-concatenated blob.
    ///
    /// Before the multi-part streaming reconstruction landed, this
    /// method always saw `Vec<Text(combined)>` and the empty-string
    /// join was correct. With per-part `provider_metadata` now
    /// preserved, multiple `Text` parts can appear interleaved with
    /// `ToolCall`s — the placeholder keeps downstream semantics intact.
    pub fn last_assistant_text(&self) -> Option<String> {
        self.messages
            .iter()
            .rev()
            .find_map(|msg| match msg.as_ref() {
                Message::Assistant(a) => match &a.message {
                    LlmMessage::Assistant { content, .. } => {
                        let mut chunks: Vec<String> = Vec::new();
                        for c in content {
                            match c {
                                AssistantContent::Text(t) if !t.text.is_empty() => {
                                    chunks.push(t.text.clone());
                                }
                                AssistantContent::ToolCall(tc) => {
                                    chunks.push(format!("[tool: {}]", tc.tool_name));
                                }
                                _ => {}
                            }
                        }
                        if chunks.is_empty() {
                            None
                        } else {
                            Some(chunks.join("\n"))
                        }
                    }
                    _ => None,
                },
                _ => None,
            })
    }

    /// Count messages of a given kind.
    pub fn count_by_kind(&self, kind: MessageKind) -> usize {
        self.messages.iter().filter(|m| m.kind() == kind).count()
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.index.clear();
        self.invalidate_last_usage();
    }

    /// Truncate the history to the first `keep_count` messages.
    /// Indices `>= keep_count` are discarded and the UUID index
    /// rebuilt.
    ///
    /// Used by the engine `Rewind` handler (both explicit and
    /// auto-restore modes per
    /// `engine-tui-unified-transcript-plan.md` §4.2) — the resulting
    /// length is the new authoritative history size that the engine
    /// then broadcasts via `ServerNotification::MessageTruncated`.
    ///
    /// No-op when `keep_count >= len()`.
    pub fn truncate(&mut self, keep_count: usize) {
        if keep_count >= self.messages.len() {
            return;
        }
        // Partial preservation: if the anchor sits in the retained
        // prefix (anchor_count <= keep_count), the marker stays valid —
        // base + estimate(messages[anchor_count..keep_count]) still
        // matches the new history's "next API call" estimate. Only
        // invalidate when the anchor would land outside the kept range.
        if let Some(marker) = &self.last_usage
            && marker.anchor_count > keep_count
        {
            self.invalidate_last_usage();
        }
        self.messages.truncate(keep_count);
        self.rebuild_index();
    }

    /// Truncate to keep only the last `n` messages.
    ///
    /// Rebuilds the UUID index after truncation.
    pub fn truncate_keep_last(&mut self, n: usize) {
        if n >= self.messages.len() {
            return;
        }
        // Drains the head, shifting every retained index down by `start`.
        // The anchor's coordinate system is invalidated wholesale.
        self.invalidate_last_usage();
        let start = self.messages.len() - n;
        self.messages.drain(..start);
        self.rebuild_index();
    }

    /// Rebuild the UUID index from the current messages. Must be
    /// called by any caller that mutated the underlying vector via
    /// [`messages_mut`](Self::messages_mut) in a way that changed
    /// indices (push/insert/remove/reorder/truncate).
    pub fn rebuild_index(&mut self) {
        self.index.clear();
        for (i, msg) in self.messages.iter().enumerate() {
            if let Some(uuid) = msg.uuid() {
                self.index.insert(*uuid, i);
            }
        }
    }

    /// Borrow the storage as a temporary `&mut Vec<Message>` for legacy
    /// in-place mutating passes (`coco_compact::micro_compact` family).
    ///
    /// Materializes one `Vec<Message>` (deep-clone of N Arcs at entry),
    /// runs `f`, then re-Arcs the result and rebuilds the index. The
    /// per-emit hot path is unaffected — only mutating compaction passes
    /// (rare: every ~50 turns) pay the bridge cost. See plan §11 F8.
    pub fn with_owned_messages<R>(&mut self, f: impl FnOnce(&mut Vec<Message>) -> R) -> R {
        // The closure may rewrite content bodies (micro-compact replaces
        // tool_result text with placeholders) — base usage no longer
        // reflects the new byte sizes. Clear marker unconditionally;
        // the next successful API call will rebuild it.
        self.invalidate_last_usage();
        let mut owned: Vec<Message> = self.messages.iter().map(|a| (**a).clone()).collect();
        let result = f(&mut owned);
        self.messages = owned.into_iter().map(Arc::new).collect();
        self.rebuild_index();
        result
    }

    // ── Last-usage marker API ────────────────────────────────────────

    /// Push an assistant message produced by a successful API call,
    /// atomically anchoring the [`LastUsageMarker`] in one operation.
    ///
    /// **The ONLY way to set the marker.** Replaces what would otherwise
    /// be a two-step "push then set" sequence — eliminating the
    /// possibility of a stale push without an anchor or vice versa.
    /// The standalone setter is intentionally absent.
    ///
    /// **Caller contract** — `msg` must be a `Message::Assistant`
    /// (debug-asserted). `usage` must be the real API-returned
    /// `TokenUsage` for the response that produced this message — not
    /// a partial / refusal / context-window-exceeded number. `model`
    /// is the `(provider, model_id)` that produced the response
    /// (post-fallback for multi-model sessions).
    ///
    /// Engine call site: `app/query::engine::run_session_loop` invokes
    /// this only on the normal-stop-reason path. Abnormal branches
    /// (ContentFilter / ContextWindowExceeded / MaxTokens recovery)
    /// must use plain [`Self::push`] / [`Self::push_arc`] so partial
    /// responses do not anchor the marker.
    ///
    /// Cleared automatically by every structural mutation (clear /
    /// truncate beyond anchor / truncate_keep_last / `with_owned_messages`
    /// / `messages_mut` / `drain_pushed_since`) so stale anchors cannot
    /// outlive a body rewrite.
    pub fn push_assistant_with_usage(
        &mut self,
        msg: Message,
        usage: TokenUsage,
        model: ProviderModelSelection,
    ) -> Arc<Message> {
        self.push_arc_assistant_with_usage(Arc::new(msg), usage, model)
    }

    /// Pre-Arc-wrapped variant of [`Self::push_assistant_with_usage`].
    /// Used by paths that already hold an `Arc<Message>` (resume
    /// hydration, replay) and want to avoid the implicit `Arc::new`.
    pub fn push_arc_assistant_with_usage(
        &mut self,
        arc: Arc<Message>,
        usage: TokenUsage,
        model: ProviderModelSelection,
    ) -> Arc<Message> {
        debug_assert!(
            matches!(arc.as_ref(), Message::Assistant(_)),
            "push_assistant_with_usage must receive a Message::Assistant \
             (got: {:?})",
            arc.kind(),
        );
        let result = self.push_arc(arc);
        // Anchor at the post-push length so the tail (messages added
        // later) starts at `anchor_count`.
        self.last_usage = Some(LastUsageMarker {
            usage,
            model,
            anchor_count: self.messages.len(),
        });
        result
    }

    /// Borrow the current marker, if any.
    ///
    /// Returns `None` initially, after every invalidating mutation
    /// (compaction / clear / rewind beyond anchor / in-place rewrite /
    /// drain / raw mutate), and until the next successful API call
    /// commits via [`Self::push_assistant_with_usage`].
    pub fn last_usage(&self) -> Option<&LastUsageMarker> {
        self.last_usage.as_ref()
    }

    /// Messages added since the last [`LastUsageMarker`] was anchored.
    /// Empty when no marker is set OR when no append has happened
    /// since. Read-only observability — production callers normally
    /// want [`Self::tokens_with_last_usage`] (folds in the base) or
    /// [`Self::last_usage`] (for the marker fields). Useful for
    /// `/context` overlay and tests that verify tail invariants.
    pub fn messages_since_last_usage(&self) -> &[Arc<Message>] {
        match &self.last_usage {
            None => &[],
            Some(marker) => {
                let start = marker.anchor_count.min(self.messages.len());
                &self.messages[start..]
            }
        }
    }

    /// Precision walk-back: `last_usage.usage.total() + estimate(tail)`.
    ///
    /// When the marker is set, the previous API call's billed total is
    /// the baseline and only messages added since are char-estimated.
    /// Marker unset (initial / post-compact / post-clear / post-rewind
    /// / post-`with_owned_messages`) → falls back to a full walk via
    /// [`crate::token_estimation::estimate_tokens_for_messages`].
    ///
    /// This is the canonical entry point for auto-compaction triggers,
    /// `/context` displays, and any caller that needs the best
    /// "current input tokens" estimate available without an API
    /// round-trip — fully cohesive with the marker state on this type.
    pub fn tokens_with_last_usage(&self) -> i64 {
        if let Some(marker) = &self.last_usage {
            let start = marker.anchor_count.min(self.messages.len());
            marker.usage.total()
                + crate::token_estimation::estimate_tokens_for_messages(&self.messages[start..])
        } else {
            crate::token_estimation::estimate_tokens_for_messages(&self.messages)
        }
    }

    fn invalidate_last_usage(&mut self) {
        self.last_usage = None;
    }
}

#[cfg(test)]
#[path = "history.test.rs"]
mod tests;

/// Persisted history entry (for session replay).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub display: String,
    pub timestamp: String,
    pub project: String,
    pub session_id: String,
}
