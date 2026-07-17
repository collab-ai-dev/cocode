# Event Hub — Design

A `coco-rs` capability that captures the full agent event stream from
one or many `coco` processes, persists it for analysis, and serves a
desktop-first responsive web UI with structured-filter search.

## Authoritative spec

**Read [`spec.md`](spec.md).** Everything an engineer needs to
implement or onboard against the event hub lives there: scope,
architecture, identity model, wire protocol, storage schema,
`EventStore` trait, Web UI stack, configuration surface, build modes,
DoD checklist.

## Status

- ✅ Design complete (Phase 1).
- ⏳ Implementation pending — see `spec.md` §13 for the suggested
  first PR.

## Out of V1 scope

| Item | Where |
|------|-------|
| Auth (bearer / mTLS / OIDC) | Dedicated future round (`07-auth.md`, TBD) |
| FTS / free-text search | V2 — trait surface already exists |
| Phase-3 control plane (cancel / approve / inject) | `06-control-plane.md`, parked |

## Historical discussion

The five-round design discussion that produced `spec.md` is archived
under [`_discussion/`](_discussion/). It is **not** the source of
truth — read it only to understand *why* a decision was made, not
*what* the decision is.

| File | What it captured |
|------|-------------------|
| `_discussion/01-requirements-analysis.md` | Scope, codex-rs reference, gap analysis, open questions |
| `_discussion/02-decisions-round-2.md` | Three foundational decisions (no agent DB; opaque `instance_id`; hub = TUI-in-browser + search); per-turn aggregation rule; TUI chip deferred |
| `_discussion/03-wire-protocol-and-schema.md` | WS contract; frame kinds; aggregation state machine; close codes |
| `_discussion/04-hub-storage-and-ui.md` | SQLite schema; `EventStore` trait; Web UI stack; three-crate layout under `coco-rs/hub/` |
| `_discussion/05-build-and-dod.md` | Justfile recipes; Tailwind v4 policy; dev-loop; Phase-1 DoD |
