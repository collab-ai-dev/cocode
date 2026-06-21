# coco-assistant — Crate Plan

Directory: `app/session/` (assistant submodule, v2)
TS source: `assistant/sessionHistory.ts` (87 LOC), `hooks/useAssistantHistory.ts` (250 LOC)
Total: ~337 LOC across 2 files

## Dependencies

```
coco-assistant depends on:
  - coco-types    (Message, SessionId)
  - coco-config   (OAuth token, org UUID)
  - coco-error
  - reqwest       (HTTP client for session events API)
  - tokio

coco-assistant does NOT depend on:
  - coco-tui      (TUI consumes assistant history, not the reverse)
  - coco-remote   (separate concern — remote manages WS, assistant manages history)
  - coco-inference (no LLM calls)
```

## Data Definitions

```rust
/// A page of session history events with cursor-based pagination.
pub struct HistoryPage {
    pub events: Vec<Message>,
    pub first_id: Option<String>,
    pub has_more: bool,
}

/// Auth context for session history API (reusable across requests).
pub struct HistoryAuthCtx {
    pub base_url: String,
    pub headers: HashMap<String, String>,
}

/// Scroll-trigger pagination state.
pub enum HistoryState {
    Loading,
    Ready { has_more: bool },
    StartOfSession,
    Failed { error: String },
}
```

## Core Logic

### History Client (from `sessionHistory.ts`, 87 LOC)

```rust
/// Paginated fetcher for remote session event history.
pub struct HistoryClient;

impl HistoryClient {
    /// Prepare auth context once, reuse for all requests.
    /// Endpoint: /v1/sessions/{session_id}/events
    /// Headers: OAuth Bearer + org UUID + beta flag
    pub async fn create_auth_ctx(session_id: &SessionId) -> Result<HistoryAuthCtx>;

    /// Fetch latest events (anchor_to_latest). Default: 100 per page.
    pub async fn fetch_latest_events(
        ctx: &HistoryAuthCtx,
        limit: Option<i32>,
    ) -> Result<HistoryPage>;

    /// Fetch events before cursor (cursor-based pagination).
    pub async fn fetch_older_events(
        ctx: &HistoryAuthCtx,
        before_id: &str,
        limit: Option<i32>,
    ) -> Result<HistoryPage>;
}
```

### Viewport-Aware Loader (business logic from `useAssistantHistory.ts`, 250 LOC)

```rust
/// Lazy-loading history with viewport fill and scroll-trigger pagination.
pub struct HistoryLoader {
    client: HistoryClient,
    state: HistoryState,
    inflight: bool,
}

impl HistoryLoader {
    /// Initial load: fetch latest page, then chain up to 10 more if viewport unfilled.
    pub async fn load_initial(
        &mut self,
        viewport_rows: i32,
    ) -> Result<Vec<Message>>;

    /// Scroll-triggered: load older page when within 40 rows of top.
    /// Returns (new_messages, scroll_anchor_delta) for scroll compensation.
    pub async fn maybe_load_older(
        &mut self,
        scroll_position: i32,
    ) -> Option<(Vec<Message>, i32)>;
}
```

Key behaviors:
- Gated on `config.viewer_only == true` (remote session viewer)
- No concurrent loads (inflight guard)
- Max 10 chained pages on initial mount
- Scroll prefetch threshold: 40 rows from top
- SDK events converted to internal Message types

## Module Layout

```
assistant/
  mod.rs              — pub mod, re-exports
  history_client.rs   — HTTP API client with cursor pagination
  history_loader.rs   — viewport-aware loading + scroll trigger logic
```
