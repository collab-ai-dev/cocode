# coco-rs Test Strategy

How to test coco-rs: which technique fits which problem, and the conventions
that keep the suite fast and deterministic. Commands live in the workspace
`coco-rs/CLAUDE.md` (`just quick-check` per iteration, `just pre-commit` once
before commit).

## Pick a technique (decision tree)

| You are testing… | Use | Where |
|---|---|---|
| Pure logic (parsers, classifiers, reducers) | a plain unit test in a companion `*.test.rs` | next to the source |
| A built-in tool / registry / permission flow | `coco-test-harness` builders (`ToolRegistryBuilder`, `PermissionBridgeBuilder`) | the consuming crate's tests |
| Provider request/response wire shape (no network) | a **cassette** replayed through the real codec | `vercel-ai/<provider>/tests/*_cassette_replay.rs` |
| The exact outbound request body (golden) | `assert_*_snapshot!` on `get_args(...)` | `vercel-ai/<provider>/tests/*_golden.rs` |
| The decoded response stream shape (golden) | replay a canned SSE, `assert_debug_snapshot!` the `Vec<…StreamPart>` | same file as the cassette test |
| TUI widget / view-model rendering | render into ratatui `TestBackend`, `assert_snapshot!` the buffer text | `tui-ui/src/widgets/*.test.rs` |
| The paint engine's **emitted ANSI** (cursor / SGR / scrollback) | `VT100Backend` — decode the real bytes through a `vt100` emulator, assert cells | `tui-ui` / `app/tui` |
| Protocol / wire-stable types | serde round-trip + `assert_snapshot!` of the JSON + a version guard | `hub/protocol/tests/wire_format.rs` |
| A real end-to-end call to a live provider | gated live test (`require_live!`) | `tests/live/tests/` |
| Time-dependent behavior | inject a clock (`MockClock` / `TestClock`); never read the wall clock in the asserted path | — |

Prefer the cheapest technique that catches the regression. A cassette or golden
beats a live test for everything except "does the real provider still behave
this way"; a `TestBackend` buffer beats a `VT100Backend` unless the bug lives in
the *emitted escape sequences* (cursor moves, SGR runs, scrollback clears).

## Integration-test aggregation (`tests/all.rs` + `suite/`)

Each top-level file in a crate's `tests/` directory compiles and **links** as a
separate test binary. Linking dominates test-build time, so a crate with five
`tests/*.rs` files pays five link steps. Aggregate them into **one** binary:

```
tests/
  all.rs            // `mod suite;`
  suite/
    mod.rs          // `mod foo; mod bar; …`
    foo.rs          // (moved from tests/foo.rs, unchanged)
    bar.rs
```

`exec/apply-patch`, `common/otel`, `services/compact`, `retrieval`, and
`app/tui` follow this pattern. Convert any crate that accrues ≥3 integration
files. Caveat: files that use **`insta` snapshots** can't be moved blindly — the
module path is part of the snapshot identifier, so relocating a file into
`suite::` orphans its `.snap` files. Aggregate snapshot-free integration files
freely; for snapshot tests, prefer co-located `src/**/*.test.rs` instead.

The Linux linker is already set to **mold** in `coco-rs/.cargo/config.toml`
(`-C link-arg=-fuse-ld=mold`; `apt install mold` once). Aggregation + mold are
the two levers on `pre-commit` link time.

## Record/replay cassettes (`coco-cassette`)

Hand-written `wiremock` fixtures match on **method + path only**, so the
outbound request is unverified and canned responses drift from reality. A
cassette records a request/response pair and replays it over a real loopback
server, exercising the genuine `reqwest` + SSE-decode path against true bytes —
no network, no key.

When to reach for which:

- **Cassette** — you want the real codec exercised *and* the outbound request
  body validated. `CassettePlayer::verify()` asserts every interaction was
  consumed and each request matched the recorded **method + path + body**
  (strict order by default; `Cassette::with_any_order()` for legitimately
  concurrent requests).
- **wiremock** — a quick canned response where request-shape fidelity doesn't
  matter (e.g. forcing a specific error status). Lower fidelity; prefer a
  cassette for anything contract-shaped.

Pattern (see `vercel-ai/anthropic/tests/messages_cassette_replay.rs` and
`vercel-ai/openai/tests/chat_cassette_replay.rs`): derive the recorded request
from the provider's own `get_args` (self-maintaining), pair it with a canned SSE
body, point the provider's `base_url` at `player.base_url()`, drive `do_stream`,
then `player.verify()`. Cassettes are credential-safe by construction —
`Cassette::save` re-scans with `coco-secret-redact` and refuses to write a file
that still contains a key.

## TUI testing

Three layers, increasing fidelity:

1. **`TestBackend` + `assert_snapshot!`** — render a widget/frame to ratatui's
   in-memory buffer and snapshot the cell text. Catches layout / composition
   regressions. Plain-text (symbols only) — styling is not captured.
2. **`VT100Backend`** (`coco-tui-ui`, `testing` feature) — a `SurfaceBackend`
   that funnels the paint engine's *real emitted ANSI* into a `vt100` emulator,
   so you can assert decoded cells (`screen.cell(r,c)` text + `.bold()` +
   `.fgcolor()`) and cursor position. Use this when the bug is in the *bytes*:
   malformed SGR, off-by-one cursor, scrollback framing (e.g. the `ESC[3J`
   theme-toggle wipe class). `TestBackend` is blind to those.
3. **`insta` snapshots** under co-located `snapshots/` dirs. Generate/update with
   `INSTA_UPDATE=always cargo test -p <crate> <filter>` (or `cargo insta
   review`); **read the generated `.snap` before committing** — a snapshot is
   only as good as the diff a human approved.

## Live tests

`tests/live/` holds tests that hit real providers. They are gated by the
`require_live!` macro (`tests/live/tests/common/mod.rs`), which resolves a target
from the available capabilities / providers / keys at runtime and returns early
when none is configured — strictly better than `#[ignore]`, which would hide the
test from `cargo test --list`. `tests/live/tests/common/` is the single shared
scaffolding module (env, fixtures, tmpdir, runtime). CI stays hermetic; live
tests run only where credentials exist.

## Determinism

- **Time** — inject a clock. `coco-async-utils::Clock` (`now` /
  `now_unix_millis`) for non-UI code, `coco-tui-ui::clock::Clock` (`now` /
  `now_ms`) for the TUI; tests substitute `TestClock` / `MockClock` and
  `advance` virtual time. Production already anchors turn timing to the injected
  clock — never read `Instant::now()` directly in a path you assert on. For
  retry/backoff *sleeps*, use tokio's `#[tokio::test(start_paused = true)]` +
  `tokio::time::advance`; the backoff *schedule* itself
  (`RetryConfig::delay_for_attempt`) is pure and tested directly.
- **Volatile fields** — before golden-snapshotting payloads with timestamps,
  UUIDs, or temp paths, run them through `coco-test-harness::normalize`
  (`normalize_str` / `normalize_json_value`) so snapshots stay portable across
  runs and machines. `coco-secret-redact` handles credentials; `normalize`
  handles the non-secret-but-nondeterministic remainder.
- **Session traces** — `coco-session-trace` captures execution semantics (tool
  lifecycle, compaction edges, turn boundaries) as a replayable bundle for
  post-mortem debugging and as golden-replay fixtures.

## Conventions

- Never inline `#[cfg(test)] mod tests { … }` — always a companion
  `#[path = "<name>.test.rs"] mod tests;`.
- `pretty_assertions::assert_eq`; compare whole objects over individual fields.
- One positional filter per `cargo test`; use a shared substring/module prefix
  or separate invocations.
