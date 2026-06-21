# Round 5 — Build, Dev-Loop, and Phase-1 DoD

> Status: **final design round.** After this, implementation can start.
> Depends on: `01..04`.
> Round 4 settled the *what*; round 5 settles the *how to build and how
> to know it's done*. No new product surface introduced here.

This document is the source of truth for:

1. Justfile recipes (build modes, dev-loop)
2. Tailwind v4 CLI policy
3. Workspace structure (members, `default-members`)
4. Static asset version pins
5. Implementation prerequisites (layer rules, EnvKey, StatusCode, workspace declaration)
6. Phase-1 Definition of Done checklist

---

## 1. Decisions inherited

| From | Decision | Where it lands here |
|------|----------|--------------------|
| Round 4 §10.3 | `default = []`, opt-in via `--features serve-hub` | §2 |
| Round 4 §12.2 A | `coco-with-hub` not CI-gated; manual recipe only | §2 |
| Round 4 §12.2 B | Tailwind v4 CLI is contributor-installed | §3 |
| Round 4 §12.2 C | Dev-loop = `cargo watch` + `tailwindcss --watch` + `--dev-assets` flag | §4 |
| Round 4 §12.2 D | `default-members = ["app/cli"]` | §5 |
| Round 4 §12.2 E | Smoke-test list as proposed | §6 |
| Round 4 §12.2 F | Asset versions pinned, committed under `hub/server/web/static/`, bumped via PR | §7 |

Auth, FTS5, control plane stay deferred per their own rounds.

---

## 2. Build modes & `justfile` recipes

These three recipes get appended to `coco-rs/justfile`:

```just
# === Event-hub build targets ===

# Default coco binary — NO embedded hub server.
# This is the only build verified by `just pre-commit`.
coco:
    cargo build -p coco-cli

# coco with the embedded hub server (all-in-one mode).
# Requires `tailwindcss` v4 CLI in $PATH (or COCO_TAILWIND_CLI env var).
# Quality NOT gated by pre-commit — run this manually after touching hub crates.
coco-with-hub:
    cargo build -p coco-cli --features serve-hub

# Standalone hub-server binary.
hub-server:
    cargo build -p coco-hub-server
```

Existing recipes (`fmt`, `quick-check`, `pre-commit`, `test`,
`test-crate`, `fix`, `clippy`, `check`, `help`) are **untouched**.

### 2.1 Build matrix

| Recipe | Cargo invocation | `coco-hub-server` linked? | Tailwind CLI required? | Pre-commit covers it? |
|--------|------------------|---------------------------|------------------------|------------------------|
| `just coco` | `cargo build -p coco-cli` | no | no | **yes** |
| `just coco-with-hub` | `cargo build -p coco-cli --features serve-hub` | yes | yes | no — manual |
| `just hub-server` | `cargo build -p coco-hub-server` | n/a (is the hub) | yes | no — manual |

### 2.2 The CLI flag `--serve-hub`

- Always present in `coco --help`.
- Behavior when the `serve-hub` feature is **off**:
  ```
  $ coco --serve-hub
  Error: This `coco` build was not compiled with the `serve-hub` feature.
         Rebuild with `just coco-with-hub` or `cargo build -p coco-cli
         --features serve-hub`. Alternatively, run a separate
         `coco-hub-server` process and point `event_hub_url` at it.
  ```
- Behavior when the feature is **on**: spawns the hub server in the
  same Tokio runtime, points `event_hub_url` at `ws://127.0.0.1:<hub_port>/v1/connect`.

---

## 3. Tailwind v4 CLI policy

### 3.1 Sourcing

Contributors install `tailwindcss` v4 themselves; the project does
**not** vendor binaries and does **not** download at build time.

| Platform | Recommended source |
|----------|---------------------|
| macOS | `brew install tailwindcss` or download from https://tailwindcss.com/blog/standalone-cli |
| Linux | `curl -sLO https://github.com/tailwindlabs/tailwindcss/releases/latest/download/tailwindcss-linux-x64 && chmod +x tailwindcss-linux-x64 && sudo mv tailwindcss-linux-x64 /usr/local/bin/tailwindcss` |
| Windows | `winget install Tailwindcss.Tailwindcss` |

Verify with `tailwindcss --version` — must report `v4.x.x` (v3 will
fail because the hub's `style.css` uses v4 `@plugin` / `@source`
syntax).

### 3.2 `build.rs` detection sequence

```rust
// coco-rs/hub/server/build.rs (sketch)
fn locate_tailwind() -> Result<PathBuf, BuildError> {
    if let Ok(p) = std::env::var("COCO_TAILWIND_CLI") {
        return Ok(PathBuf::from(p));
    }
    if let Ok(p) = which::which("tailwindcss") {
        return Ok(p);
    }
    Err(BuildError::TailwindMissing)
}
```

When `locate_tailwind` fails, `build.rs` prints:

```
error: tailwindcss v4 CLI not found.

The `coco-hub-server` crate uses Tailwind v4 to compile its CSS. Install
the standalone CLI:

  macOS:    brew install tailwindcss
  Linux:    download from https://tailwindcss.com/blog/standalone-cli
  Windows:  winget install Tailwindcss.Tailwindcss

Or set the COCO_TAILWIND_CLI environment variable to a binary path.

(This requirement applies only when building with --features serve-hub
 or when building `coco-hub-server` directly. Plain `just coco` does
 not need Tailwind.)
```

### 3.3 What `build.rs` does once it finds the CLI

```
tailwindcss -i coco-rs/hub/server/web/style.css \
            -o $OUT_DIR/style.css \
            --minify
```

Then `coco-hub-server/src/web/static_assets.rs` does:

```rust
const STYLE_CSS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/style.css"));
```

Everything else (htmx, flowbite, prism, prism-*.min.js, prism-tomorrow.min.css)
is committed verbatim under `web/static/` and pulled in via `include_dir!`.

---

## 4. Dev-loop for hub UI work

Two-terminal pattern. Both run from `coco-rs/`.

**Terminal 1 — Rust rebuild on source change:**

```
cargo watch -x 'run -p coco-hub-server -- serve --dev-assets coco-rs/hub/server/web --port 8731'
```

**Terminal 2 — Tailwind rebuild on template/style.css change:**

```
tailwindcss -i coco-rs/hub/server/web/style.css \
            -o coco-rs/hub/server/web/static/dev-style.css \
            --watch
```

### 4.1 The `--dev-assets <dir>` flag

The hub server binary gains a flag:

```
coco-hub-server serve --dev-assets <path/to/web>
```

When set, the server reads CSS/JS/templates **from disk** at the
given path instead of from the embedded copy. Defaults to embedded
when unset (production behavior).

In dev mode:
- `*.html` templates → still compiled-in (Askama is compile-time);
  any HTML change still needs a Rust rebuild (`cargo watch` covers).
- `style.css` → read from `<dev-assets>/static/dev-style.css` at
  request time, so Tailwind's `--watch` output is hot.
- `static/*.js` and `static/*.css` → read from `<dev-assets>/static/`
  at request time.

### 4.2 Documentation

The above is duplicated into `coco-rs/hub/server/CLAUDE.md` so future
contributors learn it without reading this round-5 doc.

---

## 5. Workspace structure

### 5.1 Members & default-members

Add to `coco-rs/Cargo.toml`:

```toml
[workspace]
members = [
    # ... existing crates unchanged ...
    "hub/protocol",
    "hub/connector",
    "hub/server",
]
default-members = ["app/cli"]
```

Behavior:

| Command | Builds |
|---------|--------|
| `cargo build` (from `coco-rs/`) | only `coco-cli` (because of `default-members`) |
| `cargo build --workspace` | all members including hub crates |
| `cargo build -p coco-hub-server` | just the standalone hub binary |
| `cargo test` | only `coco-cli`'s tests |
| `cargo test --workspace` | all crates including hub |

`just pre-commit` (per CLAUDE.md) runs `cargo nextest` which respects
`default-members` unless `--workspace` is passed. We **do not** add
`--workspace` to `just pre-commit` — hub crates are exercised only
when `coco-with-hub` is built manually (per §12.2 A round 4 decision).

### 5.2 Layer placement in CLAUDE.md hierarchy

Following CLAUDE.md's L0–L5 dependency rules:

| Crate | Layer | Justification |
|-------|-------|---------------|
| `coco-hub-protocol` | **L1** | Depends only on `coco-types` re-exports + `serde` / `chrono` / `uuid`. No service or app deps. Same layer as `coco-types` adjacent crates. |
| `coco-hub-connector` | **L3** | Depends on protocol + `coco-types` + `utils/secret-redact` + `common/otel` + `tokio-tungstenite`. Same layer as `coco-tools` / `coco-shell`. |
| `coco-hub-server` | **L5** | Depends on protocol + heavy deps (axum, rusqlite, askama, …). Same layer as `coco-cli` / `coco-tui` / `coco-session`. **Not** depended on by any L0–L4 crate. |

These placements are declarative; the actual `Cargo.toml`
`[dependencies]` lists must respect them. `just check-error-policy`
already enforces error-handling hygiene; layer rules are enforced by
review.

---

## 6. Phase-1 Definition of Done

V1 ships when **all of A passes**, and **B–G pass under manual test**.

### A. Default build (CI-gated)

- [ ] `just coco` succeeds on macOS (Apple Silicon + Intel) and Linux x86_64.
- [ ] `just pre-commit` passes on the default workspace.
- [ ] `coco --serve-hub` (with the feature off) prints the diagnostic message from §2.2 and exits non-zero.

### B. `with-hub` build (manual)

- [ ] `just coco-with-hub` succeeds.
- [ ] `cargo test -p coco-hub-protocol -p coco-hub-connector -p coco-hub-server` succeeds.
- [ ] Binary size delta between `coco` and `coco-with-hub` is within
      the expected envelope (~+10–20 MiB; flag if larger).

### C. Single-machine all-in-one smoke test

- [ ] `coco --serve-hub --hub-port 8731` runs without panic for ≥ 5 min idle.
- [ ] Opening `http://localhost:8731/` in a desktop browser renders the dashboard within 1 s.
- [ ] Sending a prompt in the TUI causes the running session to appear in the web UI within 1 s.
- [ ] Events stream live via SSE — new events appear without a page refresh.
- [ ] Tool calls render with `tool_name`, input preview, output preview.
- [ ] The same URL on a 375×667 viewport (iPhone SE) renders as a single column with the filter drawer collapsed.
- [ ] Prism syntax highlighting works in the event-detail modal for at least JSON, Bash, and Diff.

### D. Multi-instance smoke test

- [ ] Start `coco-hub-server serve --port 8731` standalone in terminal 1.
- [ ] Start `COCO_EVENT_HUB_URL=ws://localhost:8731/v1/connect coco` in directory A (terminal 2).
- [ ] Start same agent in directory B (terminal 3).
- [ ] The hub web UI lists two instances, each with its own `cwd` and `started_at`.
- [ ] Filter by `tool=Shell` returns events from both instances correctly.
- [ ] Filter by `instance=<id-A>` returns only instance A's events.

### E. Resilience

- [ ] Kill the hub mid-session (Ctrl-C in terminal 1); agent TUI keeps responding to new prompts, no panic.
- [ ] Restart the hub; agent reconnects on its next batch flush.
- [ ] After enough offline time to overflow the ring (10 000-event default), an `EventsDropped` marker shows up in the web UI's event list for that session as a visually distinct row.
- [ ] `/clear` in the agent's TUI rotates `session_id`; the web UI shows a new session row under the same `instance_id`.

### F. Retention (manual sanity, not exhaustive)

- [ ] With `hub_retention_sweep_interval_secs` lowered to 30 and a backdated `received_at`, the sweep deletes the expected rows and `incremental_vacuum` runs.
- [ ] Default values (3 days / 3 GiB) do not delete anything during this smoke test (no surprise data loss for the operator running these checks).

### G. Search API surface

- [ ] `GET /v1/search?tool=Shell` returns only Shell tool events.
- [ ] `GET /v1/search?error=1` returns only failures.
- [ ] `GET /v1/search?q=foo` returns HTTP 400 with the documented `free_text_not_supported` body.
- [ ] `GET /v1/instances`, `/v1/instances/<id>/sessions`, `/v1/instances/<id>/sessions/<sid>/events` all return paged results with stable cursors.

### Out of scope for Phase-1 DoD

- Auth (deferred).
- FTS / free-text `q` (deferred to V2).
- Phase-3 control plane (deferred).
- Performance benchmarks (no targets set this round).
- Cross-host distributed deployment (works mechanically but unverified at scale).
- Tailwind dev-loop verified (developer ergonomics, not user surface).

---

## 7. Static asset version pins

| Asset | Version | Source | Path in repo |
|-------|---------|--------|--------------|
| Tailwind CSS | **v4.0.x** (latest 4.0 patch) | https://tailwindcss.com/blog/standalone-cli (CLI, not embedded) | not committed; contributor installs |
| Flowbite (CSS + JS) | **v2.x** (latest 2.x) | https://flowbite.com/docs/getting-started/quickstart/ | `coco-rs/hub/server/web/static/flowbite.min.js` |
| HTMX | **v1.9.x** (latest 1.9 patch) | https://htmx.org/ | `coco-rs/hub/server/web/static/htmx.min.js` |
| HTMX SSE extension | matching HTMX | https://htmx.org/extensions/server-sent-events/ | `coco-rs/hub/server/web/static/htmx-ext-sse.min.js` |
| Prism.js core | **v1.29.x** (latest 1.29 patch) | https://prismjs.com/download.html | `coco-rs/hub/server/web/static/prism.min.js` |
| Prism languages | v1.29.x matching core | bundled download | `coco-rs/hub/server/web/static/prism-<lang>.min.js` × 11 |
| Prism theme | `prism-tomorrow` v1.29.x | bundled download | `coco-rs/hub/server/web/static/prism-tomorrow.min.css` |

### 7.1 Update process

Manual bump PR. Steps:

1. Download new minified files from the upstream source.
2. Replace the committed copy under `coco-rs/hub/server/web/static/`.
3. Run `just coco-with-hub` to verify it still compiles.
4. Run the §6 smoke checks A + C at minimum.
5. Open a PR titled `chore(hub): bump <asset> to <version>`.
6. Include a one-line "what changed upstream" note in the PR body.

No `package.json`, no npm, no automated updater in V1. Manual bumps
happen rarely (a few times a year at most).

### 7.2 Vendor file integrity

Each committed asset's SHA-256 lives in
`coco-rs/hub/server/web/static/.checksums` so a future automated
update script (post-V1) can verify the bundle was untouched on each
build. V1 just commits and trusts git.

---

## 8. Implementation prerequisites

These are mechanical decisions to clear the runway. None of them
warrant their own round but they need to be settled before
implementation starts.

### 8.1 `EnvKey` registrations

Add to `coco-rs/common/config/src/env_key.rs`:

```rust
// Connector-side (always available, used when event_hub_url is set):
COCO_EVENT_HUB_URL                       → event_hub_url
COCO_EVENT_HUB_BEARER_TOKEN              → event_hub_bearer_token   // reserved (V1 ignored)
COCO_EVENT_HUB_RING_BUFFER_SIZE          → event_hub_ring_buffer_size
COCO_EVENT_HUB_BATCH_MAX_EVENTS          → event_hub_batch_max_events
COCO_EVENT_HUB_BATCH_MAX_BYTES           → event_hub_batch_max_bytes
COCO_EVENT_HUB_BATCH_MAX_INTERVAL_MS     → event_hub_batch_max_interval_ms

// Hub-side (used only when serve-hub feature is on, or coco-hub-server runs):
COCO_HUB_DATA_DIR                        → hub_data_dir
COCO_HUB_BIND_ADDR                       → hub_bind_addr
COCO_HUB_PORT                            → hub_port
COCO_HUB_MAX_FRAME_BYTES                 → hub_max_frame_bytes
COCO_HUB_MAX_TOOL_OUTPUT_INLINE          → hub_max_tool_output_inline
COCO_HUB_RETENTION_DAYS                  → hub_retention_days
COCO_HUB_RETENTION_MAX_BYTES             → hub_retention_max_bytes
COCO_HUB_RETENTION_SWEEP_INTERVAL_SECS   → hub_retention_sweep_interval_secs
COCO_HUB_VACUUM_THRESHOLD_BYTES          → hub_vacuum_threshold_bytes
COCO_HUB_LOG_LEVEL                       → hub_log_level

// Build-time only (read by hub/server/build.rs, not at runtime):
COCO_TAILWIND_CLI                        → (not in EnvKey — build.rs reads via std::env::var)
```

Total: 16 runtime keys (6 connector + 10 server).
`hub_auth_token` was removed in round 4 (auth deferred).

### 8.2 `StatusCode` allocations

Reserve category `EventHub = 14` in `coco-rs/common/error/`:

| Code | Variant | Meaning |
|------|---------|---------|
| 14_001 | `EventHub::ConnectorSendFailed` | Connector failed to POST a batch (transient, retriable) |
| 14_002 | `EventHub::ConnectorBufferOverflow` | Ring buffer overflowed; events dropped |
| 14_003 | `EventHub::HubProtocolMismatch` | WS subprotocol negotiation failed (4001 close) |
| 14_004 | `EventHub::HubFrameTooLarge` | Frame above `hub_max_frame_bytes` (4013 close) |
| 14_005 | `EventHub::HubInvalidAnnounce` | Malformed `announce` frame (4000 close) |
| 14_006 | `EventHub::StoreError` | Underlying `EventStore` returned an error |
| 14_007 | `EventHub::StoreNotSupported` | `EventStore::search` with `q` set under V1 SQLite impl |
| 14_008 | `EventHub::TailwindCliMissing` | `build.rs` could not find `tailwindcss` (compile-time error) |

Document in `coco-rs/common/error/README.md` under existing
StatusCode categories.

### 8.3 Workspace member declaration

Final form of the `[workspace]` block in `coco-rs/Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = [
    # ... existing crates unchanged ...
    "hub/protocol",
    "hub/connector",
    "hub/server",
]
default-members = ["app/cli"]
```

### 8.4 Per-crate `CLAUDE.md`

Each new crate ships its own `CLAUDE.md` per project convention:

- `coco-rs/hub/protocol/CLAUDE.md` — wire types, schema-version policy
- `coco-rs/hub/connector/CLAUDE.md` — aggregator state machine, dev-loop reminders
- `coco-rs/hub/server/CLAUDE.md` — `EventStore` trait, store impl features, dev-loop (§4), Tailwind install reminder, asset update process (§7.1)

The root `coco-rs/CLAUDE.md` (the project's main one) gains:

- A new row in the "Crate Guide" table under a new "Hub" group, parallel to the existing "Common" / "Services" / "Core" / "Exec" / "Root" / "App" / "Standalone" / "Utils" groups, with three entries.
- A reference to this design directory (`docs/coco-rs/event-hub/`) under "Specialized Documentation".

---

## 9. What's still deferred (final tally)

| Item | Status | When |
|------|--------|------|
| Auth (bearer / mTLS / OIDC) | deferred | Dedicated future round |
| FTS5 / free-text search (`q` param) | deferred | V2 |
| Phase-3 control plane (cancel / approve / inject) | parked | Phase-3 doc when motivated |
| OTLP export | post-V1 | When real users ask |
| Multi-hub federation | post-V1 | When real users ask |
| Per-instance retention overrides | post-V1 | When real users ask |
| Blob storage for huge tool outputs | post-V1 | When inline-truncation pain shows |
| TUI hub-status chip | post-V1 | When silent-loss UX complaints emerge |
| `coco-with-hub` CI gate | post-V1 | After hub stabilizes — early phase accepts rot risk |
| Automated asset-version bumps | post-V1 | When manual bumps become annoying |
| Performance benchmarks for ingest / search | post-V1 | When real load hits |
| Cross-host TLS / cert story | post-V1 | Paired with auth round |

---

## 10. Implementation start signal

After this round, the following can begin in parallel:

1. **`coco-hub-protocol`** — pure wire types, no external blockers.
2. **`coco-hub-connector`** — depends on protocol; can be developed against a mock server.
3. **`coco-hub-server`** — depends on protocol; SQLite + Axum + Web UI work
   can be split into sub-tasks.
4. **`app/cli` wiring** — `--event-hub-url` / `--serve-hub` flag plumbing
   + feature-gated `coco-hub-server` dep; can land before the server is
   even functional.
5. **`SessionRegistration.instance_id`** — one-line addition to
   `app/session/src/concurrent_sessions.rs`; lowest-risk first step.

Suggested first PR: items 4 + 5, which establish the integration
points without requiring any new crate to be functional yet.
