# Grok-Build 0.2.117–1.0.0 Full-Crate Audit

Status: complete point-in-time comparison, 2026-08-12.

This is the non-UI companion to
[`ui/grok-build-tui-port-plan.md`](ui/grok-build-tui-port-plan.md). It records
the complete `crates/codegen` review requested after the TUI analysis; it does
not redefine crate-owned types or stable architecture. The code and canonical
`crate-coco-*.md` documents remain authoritative.

## Scope and method

The audited upstream interval is:

```text
dd04f397b1d02f2272b092555669dfba1f01bc85
  ..b13fa526f5112c0b20dad5f1f2300d3d3b127895
```

It contains ten public-repository sync commits from 2026-07-31 through
2026-08-10, including grok-shell 0.2.117, 0.2.118, 0.2.119, 0.2.120, and
1.0.0. Under `crates/codegen` it changes 860 files across all 36 changed
top-level crates (+98,225 / -31,254).

The review used four gates for every upstream change:

1. Read the release notes and the changed crate, not just its name or diff
   size.
2. Find the corresponding coco owner and trace the live call path.
3. Require a reproducible defect or a missing product contract before porting.
4. Re-implement at coco's existing owner seam; do not add compatibility
   aliases, parallel state, or reference-project architecture that conflicts
   with coco's boundaries.

“No port” below therefore means one of: coco already has the behavior, the
change is specific to grok's remote/product infrastructure, or evidence is
insufficient to justify adding lifecycle complexity.

## Release-level result

| Release | Main upstream signal | coco result |
| --- | --- | --- |
| 0.2.117 | Custom CA, background-agent stop, task ACP status/wait fixes, plan approval, resize performance | CA and ACP product seams are not equivalent. Task lifecycle and plan behavior were checked; no matching defect. UI resize work remains in the TUI plan. |
| 0.2.118 | Session delete, shortcuts, tmux diagnostics, sidechat retry, compact cancellation, task limits/status, plan UI, context errors | UI/product items stay in the TUI plan. coco already has typed task limits, session-scoped sidechat, and compact cancellation ownership; no direct port. |
| 0.2.119 | Free deny globs, safer automation, Mermaid, picker fixes, bounded task logs, auth, startup work | Deny-glob enforcement had real fail-open and platform-semantic defects and was rebuilt. Task storage was already disk-backed and bounded, but a UTF-8 tail panic was found and fixed. Other items are UI or product-specific. |
| 0.2.120 | Model status, changes refresh, task log sizing, export message | UI items stay in the TUI plan. Existing task output accounting and export ownership were already adequate. |
| 1.0.0 + post-release | Table wrapping, full permission scripts, clean errors, terminal appearance, session-fork memory, MCP media, retry policy, shutdown/session-load work | UI corrections landed in `608f72cc`. Session fork memory and sandbox defects were confirmed and fixed. MCP media and generic retry gaps were disproved. Global forced exit and ACP load barriers conflict with coco's scoped lifecycle and were rejected. |

## Confirmed defects and remediation

### Landed in the UI commit

- Markdown fallback wrapping sliced by scalar/byte assumptions around complex
  Unicode. It now wraps extended grapheme clusters.
- Permission prompts could receive a shortened shell preview. The producer now
  sends the complete script; the existing pager owns scrolling.
- Provider diagnostics leaked into user-visible inference banners. The typed
  inference error owner now emits clean provider-neutral output while logs keep
  raw diagnostics.
- The proposed G5 rename was rejected: `ThemeSetting::Auto` already means
  “follow the terminal background” via OSC 11, then `COLORFGBG`, then dark.
  That is the correct default over SSH and tmux. `follow_terminal` would merely
  rename the same behavior, while `follow_system` would be a distinct optional
  desktop feature, not an `auto` replacement.

### Landed in the owner-crate remediation

- `coco-session`: session resumability and fork copying no longer materialize
  an entire JSONL transcript. Forking streams byte records through a bounded
  buffer, preserves malformed/torn records, rewrites valid `session_id`
  fields, rejects in-place overwrite, and atomically publishes the destination
  only after a successful copy.
- `coco-sandbox`: invalid globs, walk failures, depth truncation, and resource
  overruns no longer silently weaken a deny. Linux walks from literal prefixes
  with depth 64, 4,096-match and two-million-entry caps, and masks canonical
  symlink targets. macOS uses anchored runtime Seatbelt regexes, including
  canonical and `/private` aliases, so post-launch matches remain denied. One
  validator defines the portable syntax for both platforms. A cross-backend
  property test also exposed single-character wildcards and classes as
  UTF-8-byte versus Unicode-scalar divergent; those ambiguous forms are now
  rejected rather than interpreted differently.
- `coco-agent-host`: the bounded shell stderr tail no longer drains a `String`
  at an arbitrary byte offset, which could panic on emoji or CJK. It now keeps
  a suffix at a valid UTF-8 boundary.
- `coco-query`: non-interactive `AskUserQuestion` denial now tells the model
  that no operator exists and to continue with its best judgment. The
  permission boundary now denies every residual `Ask` when no approval bridge
  exists; the old implicit auto-approval fallback was intentionally removed.
- `coco-agent-host` tests: Tokio's `test-util` feature is now an explicit dev
  dependency. Single-crate tests no longer rely on workspace feature
  unification to compile liveness tests.

### Disproved gaps

- MCP large images are already represented as typed media blocks and converted
  to `FileData` before text offloading; they are not flattened into oversized
  text.
- Inference retry already covers 408, 409, 429, and every generic 5xx. grok's
  525/526 handling is provider/edge-specific and does not justify widening
  coco's generic policy without a matching provider contract.
- Background task output is already disk-backed, delta-addressed, and bounded;
  only the independent UTF-8 suffix bug required a change.
- Subagent concurrency/depth limits and dangerous broad auto-rule stripping
  already exist at typed owners.

## All 36 changed crates

| # | Upstream crate | Material change reviewed | Decision for coco |
| ---: | --- | --- | --- |
| 1 | `ptyctl` | PTY lifecycle comments/small cleanup | No behavior gap. Coco's PTY lifecycle is separately owned and tested. |
| 2 | `xai-chat-state` | Narrow match-guard/style cleanup | No semantic delta worth porting. |
| 3 | `xai-codebase-graph` | Idiomatic reverse-key sorting | Style-only; no architectural benefit. |
| 4 | `xai-fast-worktree` | SQLite dashboard registry, read-only/busy robustness | No equivalent remote worktree registry. Do not add SQLite to local worktree ownership. |
| 5 | `xai-file-utils` | xAI artifact upload and edge retry details | Product-specific. Generic coco retries were checked separately and already cover their contract. |
| 6 | `xai-fsnotify` | Prune nested checkouts from a whole-tree watcher | Coco has no single always-on whole-workspace watcher. Generic caller-scoped watchers must not silently exclude nested repositories. |
| 7 | `xai-grok-agent` | Headless/no-operator behavior | Adopted at coco's permission-controller boundary with actionable `AskUserQuestion` denial. |
| 8 | `xai-grok-auth` | Central bearer suffix attribution | No corresponding cross-process suffix seam; no port. |
| 9 | `xai-grok-config-types` | Remote turn-summary, concurrency, and restore flags | Coco already has local typed limits. Remote restore DTOs are not applicable. |
| 10 | `xai-grok-http` | OS errno extraction for telemetry | Diagnostic enhancement only; not a retry/correctness gap. Keep with the broader telemetry backlog. |
| 11 | `xai-grok-markdown` | Narrow-table and Unicode wrapping | Relevant correctness technique landed in `608f72cc`; renderer architecture was not copied. |
| 12 | `xai-grok-pager` | Main app-owned scrollback/UI release work | Fully handled by the dedicated TUI plan. Native scrollback remains a coco invariant. |
| 13 | `xai-grok-pager-bin` | Boot policy, runtime cap, forced-exit paths | No reproduced coco hang. A global `_exit` watchdog would bypass scoped cleanup, so it is rejected absent evidence. |
| 14 | `xai-grok-pager-minimal` | Minimal UI surface | TUI-only; no non-UI port. |
| 15 | `xai-grok-pager-pty-harness` | Reference PTY scenarios | Test ideas belong to the TUI plan; alt-screen assertions cannot be copied into native-scrollback tests. |
| 16 | `xai-grok-pager-render` | Frame diff, selection, link/render work | TUI-only. Relevant Unicode correctness was re-implemented at coco's renderer seam. |
| 17 | `xai-grok-sampler` | Retry classification | Coco already retries generic transient HTTP classes. Do not import provider-specific 525/526 globally. |
| 18 | `xai-grok-sampling-types` | Typed/clean sampling errors | Coco's typed inference error owner now supplies clean user output. |
| 19 | `xai-grok-sandbox` | Portable deny globs, caps, fail-closed errors, macOS runtime regexes | Confirmed gap; fully adopted at `coco-sandbox`, with platform-native enforcement and shared validation. |
| 20 | `xai-grok-shared` | Shared UI/config details | TUI settings only; no independent runtime gap. |
| 21 | `xai-grok-shell` | Release aggregation, session fork, task/UI/lifecycle work | Session-fork memory was adopted by `coco-session`; other changes were routed to their actual owners and reviewed separately. |
| 22 | `xai-grok-shell-base` | Metadata/base plumbing | No behavior gap. |
| 23 | `xai-grok-shell-session-support` | Managed MCP extraction/move | Coco already separates MCP lifecycle into `services/mcp`; copying the split would duplicate ownership. |
| 24 | `xai-grok-telemetry` | External OTLP and startup/permission analytics | Observability expansion, not a release correctness defect. Evaluate under `coco-otel`, not opportunistically here. |
| 25 | `xai-grok-test-support` | Reference-only fixtures/helpers | No production port. |
| 26 | `xai-grok-tools` | MCP image handling, task logs, subagent limits, headless copy | Images/limits/storage already covered. Actionable headless copy and the independently found UTF-8 tail fix were adopted at their owners. |
| 27 | `xai-grok-tools-api` | Metadata marker/API plumbing | No matching contract gap. |
| 28 | `xai-grok-update` | Binary self-updater | Cocode has no in-process binary updater; out of scope and not silently added. |
| 29 | `xai-grok-version` | Release version bump | No code port. |
| 30 | `xai-grok-workspace` | Remote restore/workspace service and permission work | Remote code restore is not applicable. Existing coco permission gates and rule narrowing were verified. |
| 31 | `xai-grok-workspace-client` | New remote RPC client | No equivalent service; no port. |
| 32 | `xai-grok-workspace-types` | Remote workspace DTOs | No equivalent service; no port. |
| 33 | `xai-ratatui-textarea` | Input/UI fixes | Covered by the TUI comparison; coco retains one existing editor owner. |
| 34 | `xai-sqlite-journal` | SQLite journaling | Coco sessions/worktrees do not use this storage model. Adding it would create a second authority. |
| 35 | `xai-token-estimation` | Small idiomatic checked-division cleanup | No correctness delta. |
| 36 | `xai-tty-utils` | Process gauges, runtime cap, bounded reap, git-lock variants | Memory tracing already exists. Runtime caps and D-state reap remain evidence-gated lifecycle candidates, not confirmed coco defects. |

## Architecture assessment

The remediation plan is sound because each change has one owner:

```text
settings/rules
    -> coco-sandbox adapter
    -> one validated deny-glob policy
       -> Linux bounded expansion -> bwrap bind-over
       -> macOS anchored regex    -> Seatbelt profile

session catalog -> coco-session streaming/atomic fork
task process     -> coco-agent-host UTF-8-safe bounded tail
permission Ask  -> coco-query explicit bridge-or-deny policy
```

No compatibility aliases were retained. Unsupported glob syntax now errors;
the old silent-skip behavior is intentionally gone. No new global manager,
cross-crate state bag, or duplicate lifecycle authority was introduced.

## Evidence and remaining candidates

Targeted `test-crate` verification covers `coco-sandbox`, `coco-session`,
`coco-agent-host`, and `coco-query`, including integration suites and the pure
macOS profile generator on Linux CI. The sandbox suite also cross-products the
accepted rule grammar against both globset and the generated regex backend.
The final commit gates are the workspace `quick-check` and repository
`pre-commit` workflow.

The following are not hidden TODOs from this remediation; they require a new
product contract or a reproduced failure before implementation:

- process-wide runtime cap / forced exit;
- bounded D-state process reaping;
- remote workspace restore and its RPC/SQLite registry;
- external OTLP/permission analytics expansion;
- desktop-system theme following as a separate explicit mode.

They should not be bundled into this release absorption merely because grok
ships them.
