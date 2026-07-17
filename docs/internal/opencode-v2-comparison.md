# opencode v2 vs coco-rs Comparison

Date: 2026-06-28

This document compares `opencode` with `coco-rs` across product surface,
architecture, and UI design. It focuses on what `coco-rs` should adopt, adapt,
defer, or reject from `opencode`, especially the v2 session architecture.

Source labels intentionally use repository-relative names such as
`opencode: specs/v2/session.md`. They do not include local checkout paths.

## Executive Summary

`opencode` v2 is most valuable as a design reference for durable session
admission, reconnect-safe event/history APIs, context epochs, scoped registries,
and UI composition around session timelines. It should not be treated as a
drop-in blueprint for `coco-rs`.

The highest-ROI ideas for `coco-rs` are:

- Separate durable prompt admission from model execution.
- Add explicit per-session execution coordination.
- Introduce Context Epoch-style persistence for model-visible system context.
- Strengthen tool materialization with turn-scoped registry identity and stale
  tool-call rejection.
- Add reconnect-friendly session history/event APIs without abandoning the
  existing `MessageHistory` authority too early.
- Borrow selected UI patterns from opencode's session timeline, composer, and
  permission/question surfaces.

`coco-rs` is already stronger in several areas:

- Provider runtime breadth and multi-provider abstraction.
- Permission hardening, including fail-secure defaults and classifier-backed
  policy.
- Compaction strategy maturity.
- Ratatui TEA discipline and the `tui-ui` domain-free presentation boundary.
- Three-layer `CoreEvent` design and unified transcript authority.

The main caution is that `opencode` v2 is still incomplete. Its v2 specs are
strong architectural material, but current public docs and current implementation
do not always match the v2 target. In particular, the TUI extraction target says
the TUI should depend only on SDK-shaped boundaries, while the current package
still depends on core/plugin packages.

## Product And Feature Comparison

| Area | opencode | coco-rs | Assessment |
| --- | --- | --- | --- |
| Primary UX | Terminal UI, desktop/web app, IDE-oriented surfaces, server/client split | CLI/TUI centered Rust agent with app/session/query architecture | opencode has broader app surface; coco-rs has a tighter terminal-first execution model. |
| Agents | Built-in build/plan agents, subagents, hidden utility agents, child-session navigation | Build/plan-style modes, subagents, task/fork/worktree paths under active parity work | opencode UI around child sessions is worth studying; coco-rs runtime ownership appears more explicit. |
| Prompt steering | Durable `session_input` with `steer` and `queue` delivery | `CommandQueue` scoped to `SessionRuntime`, drained at turn boundaries | opencode has a stronger durability model; coco-rs has a simpler runtime-local queue. |
| Session history | Durable event projection into session messages and finite history APIs | JSONL transcript-as-truth, `MessageHistory` as authority, pluggable backend | opencode is stronger for reconnect/multi-client; coco-rs is simpler and easier to reason about locally. |
| Events | `EventV2` durable aggregate sequence plus live subscriptions | Three-layer `CoreEvent`: Protocol, Stream, Tui | opencode offers replay mechanics; coco-rs has cleaner semantic event layering. |
| Context | Context Epochs, System Context Registry, mid-conversation system messages | Context assembly pipeline with memory/instructions/attachments and prompt building | opencode's epoch model is a strong reference for persistence and cache stability. |
| Tools | Effect schema codecs, opaque definitions, materialized registry, output bounding | Tool trait, validated input newtype, safe/unsafe execution, callback handles | Both have strong ideas; combine opencode materialization with coco-rs validation discipline. |
| Permissions | Location/agent-scoped permission rules and pending approvals | Fail-secure permissions, classifier modes, source precedence, sandbox integration | coco-rs should not weaken its current permission posture to match opencode defaults. |
| Providers | v2 runner currently supports a narrow provider route set | Multi-provider Vercel AI based provider crates and runtime roles | opencode's catalog/policy split is useful; provider runtime should stay coco-rs-led. |
| Plugins | Scoped transforms for catalog/agents/skills/references/integrations | Plugin manifest/contribution system with skills/hooks/MCP/agents/commands | opencode's scoped transform lifecycle can improve hot reload semantics. |
| Config | Current docs show v1/v1.1 config; v2 specs redesign plural domains and policy separation | Layered settings/env/runtime overrides with hot reload and model roles | Use opencode v2 config as design input, not as parity target. |
| Compaction | v2 automatic and overflow-triggered compaction design | Mature multi-strategy compaction, reactive PTL, API-native Anthropic handling | coco-rs is stronger; only borrow durable checkpoint/event clarity. |
| UI | OpenTUI/Solid TUI plus web/desktop session timeline and composer | Ratatui TEA, transcript cells, native scrollback, pure `tui-ui` primitives | Borrow UI patterns, not package boundaries. |

## Architecture Comparison

### Session Execution

`opencode` v2 separates prompt admission from execution. `sessions.prompt`
records one durable inbox row, returns an admission receipt, and only then wakes
execution unless `resume: false` is requested. A prompt is not model-visible
until the runner publishes `Prompted`, which atomically projects it into session
history.

`coco-rs` currently drives execution through `QueryEngine`: user input becomes
conversation context, message history, a provider stream, tool execution, hook
processing, continuation checks, compaction, and command-queue drain. Its
steering model is runtime-local rather than durable.

The `opencode` model is better for reconnect, multi-client UX, and explicit
admission idempotency. The `coco-rs` model is simpler and already well aligned
with the current terminal agent loop. The best path is incremental: add a
durable prompt inbox around `coco-rs` session storage before adopting a full
event-sourced runner.

Sources:

- `opencode: specs/v2/session.md`
- `opencode: packages/core/src/session.ts`
- `opencode: packages/core/src/session/input.ts`
- `opencode: packages/core/src/session/run-coordinator.ts`
- `coco-rs: app/query/CLAUDE.md`
- `coco-rs: app/session/CLAUDE.md`
- `coco-rs: docs/internal/session-storage-backend-design.md`

### Context And Runtime Instructions

`opencode` v2 introduces Context Epochs. A session stores an immutable baseline
system context for a provider-cache epoch plus a hidden structured snapshot. At
safe provider-turn boundaries, changed context sources are rendered as durable
mid-conversation system messages. Completed compaction starts a fresh epoch.

`coco-rs` has a strong context assembly pipeline with memory file discovery,
attachments, mentions, plan mode, and prompt building, but its system context is
less explicitly modeled as a persisted baseline/snapshot pair.

This is one of the most valuable opencode ideas. It addresses stale runtime
context, provider cache stability, and restart safety without pushing context
changes asynchronously into active turns.

Sources:

- `opencode: CONTEXT.md`
- `opencode: specs/v2/session.md`
- `opencode: packages/core/src/session/context-epoch.ts`
- `opencode: packages/core/src/system-context/*`
- `coco-rs: core/context/CLAUDE.md`
- `coco-rs: docs/internal/prompt-cache-design.md`

### Events, History, And Replay

`opencode` v2 has durable aggregate events with per-session sequence cursors,
projectors, finite history reads, and replay-and-tail session streams. This is
a good fit for desktop/web clients and reconnectable SDK consumers.

`coco-rs` intentionally keeps `MessageHistory` as the transcript authority and
uses three-layer `CoreEvent` envelopes for protocol, stream, and TUI concerns.
That layering is a strength: it prevents a flat event model from leaking UI-only
or streaming-only details into durable protocol state.

The recommendation is not to replace `CoreEvent` with `EventV2`. Instead,
`coco-rs` should add replayable session event/history APIs above its existing
transcript authority, preserving the three-layer event taxonomy.

Sources:

- `opencode: packages/core/src/event.ts`
- `opencode: packages/core/src/session/history.ts`
- `opencode: packages/core/src/session/projector.ts`
- `opencode: specs/v2/schema-changelog.md`
- `coco-rs: common/types/src/event.rs`
- `coco-rs: docs/internal/event-system-design.md`
- `coco-rs: docs/internal/engine-tui-unified-transcript-plan.md`

### Tools

`opencode` v2 tools are created through typed codecs, hidden runtime metadata,
and materialized registries. Materialization captures the effective tool
definitions for a provider turn and can reject stale tool calls if the
registration changed before settlement. The registry also centralizes model
output bounding.

`coco-rs` has strong Rust-side tool safety: validated input newtypes prevent raw
free-form input from reaching execution, safe tools run concurrently, unsafe
tools are queued, and callback handles decouple tool execution from surrounding
systems.

The best combined design is: keep `coco-rs` validation and execution discipline,
then add turn-scoped materialization identity and centralized model-output
bounding.

Sources:

- `opencode: specs/v2/tools.md`
- `opencode: packages/core/src/tool/tool.ts`
- `opencode: packages/core/src/tool/registry.ts`
- `coco-rs: core/tool-runtime/CLAUDE.md`
- `coco-rs: docs/internal/tool-schema-validated-newtype-plan.md`
- `coco-rs: docs/internal/tool-result-budget-plan.md`

### Permissions

`opencode` v2 normalizes location/agent-scoped rules and pending permission
requests. Its current public docs still describe permissive defaults and
v1-style permission configuration.

`coco-rs` should keep its stricter posture: deny/passthrough defaults, explicit
source priority, classifier-backed auto/yolo modes, dangerous path detection,
and sandbox integration.

The useful opencode idea is binding permission state to the effective agent and
provider turn, so later agent switches cannot change policy for already-issued
tool calls.

Sources:

- `opencode: packages/core/src/permission.ts`
- `opencode: packages/web/src/content/docs/permissions.mdx`
- `coco-rs: core/permissions/CLAUDE.md`
- `coco-rs: docs/internal/permission-sandbox-hardening.md`

### Catalog, Providers, Config, And Plugins

`opencode` v2 separates provider/model catalog data from policy and uses scoped
replayable transforms for plugins and other contributors. That lifecycle is
attractive: disabling a plugin removes its transform and rebuilds the effective
catalog.

`coco-rs` already has a more complete provider runtime and a layered settings
model. It should not copy opencode's current narrow v2 provider routes. It
should adapt the catalog/policy split and scoped transform lifecycle where it
improves hot reload and provenance.

Sources:

- `opencode: specs/v2/config.md`
- `opencode: specs/v2/catalog-config-plugin-lifecycle.md`
- `opencode: specs/v2/provider-model.md`
- `opencode: specs/v2/provider-policy.md`
- `opencode: packages/core/src/catalog.ts`
- `opencode: packages/core/src/state.ts`
- `opencode: packages/core/src/plugin/host.ts`
- `coco-rs: common/config/CLAUDE.md`
- `coco-rs: plugins/CLAUDE.md`
- `coco-rs: docs/internal/multi-provider-plan.md`

### UI

`opencode` has useful session UI patterns:

- A timeline that separates assistant turns, reasoning, tool parts, diagnostics,
  diffs, and summaries.
- A bottom composer region that coordinates prompts, permission questions,
  follow-ups, reverts, and todos.
- Child-session navigation for subagent workflows.
- SDK-shaped event reducers for web/desktop state.

`coco-rs` should borrow these interaction patterns selectively. It should not
copy opencode's current TUI package boundary because the design target says the
TUI should be SDK-only, while the current package still imports core/plugin
dependencies. `coco-rs` already has a cleaner `tui-ui` seam for pure
presentation primitives.

Sources:

- `opencode: specs/tui-package.md`
- `opencode: packages/tui/package.json`
- `opencode: packages/tui/src/app.tsx`
- `opencode: packages/app/src/pages/session.tsx`
- `opencode: packages/app/src/pages/session/timeline/message-timeline.tsx`
- `opencode: packages/app/src/pages/session/composer/session-composer-region.tsx`
- `opencode: packages/session-ui/src/components/session-turn.tsx`
- `coco-rs: app/tui/CLAUDE.md`
- `coco-rs: tui-ui/CLAUDE.md`
- `coco-rs: docs/internal/ui/tui-v2-design.md`

## coco-rs Improvement Recommendations

| Improvement | Decision | Decision Reason | ROI | Priority | Risk | Implementation Notes |
| --- | --- | --- | --- | --- | --- | --- |
| Durable prompt admission inbox | Adapt | It directly improves prompt idempotency, reconnect UX, and multi-client semantics while fitting the existing session backend model. | High | P0 | Medium: must not destabilize `QueryEngine` turn ownership. | Add a session-level admitted-input record first; keep `CommandQueue` as runtime steering until durable promotion is proven. |
| Per-session execution coordinator | Adopt | Serializing execution per session prevents overlapping drains and makes interrupt/resume semantics explicit. | High | P0 | Low/Medium: coordination bugs can deadlock active turns. | Implement as process-local coordination first; do not introduce distributed ownership yet. |
| Context Epoch baseline/snapshot | Adapt | Persisting exact model-visible system context improves cache stability, restart behavior, and auditability. | High | P0 | Medium/High: context sources and compaction boundaries must be carefully defined. | Start with environment/date/instruction sources; add plugin-defined sources after plugin lifecycle is scoped. |
| Turn-scoped tool materialization identity | Adapt | Stale tool-call rejection closes a real consistency gap when tools/config/plugins change between provider turn and settlement. | High | P0 | Medium: requires registry versioning without overcomplicating tool execution. | Preserve `ValidatedInput` and safe/unsafe execution; add materialization IDs and settlement checks. |
| Centralized model-visible tool output bounding | Adopt | It complements the existing Tool Result Budget plan and prevents each tool from inventing incompatible truncation behavior. | High | P0 | Medium: provider-native tool results may require exact round trips. | Bound only core-executed model-visible output; keep provider-hosted payload handling provider-aware. |
| Replayable session history API | Adapt | Enables reconnect, desktop/web clients, and SDK consumers without replacing current transcript authority. | High | P1 | Medium: projection and cursor semantics can conflict with JSONL transcript assumptions. | Expose finite reads from `MessageHistory` first; add durable cursors before event sourcing. |
| Replay-and-tail session event stream | Adapt | Useful for app/server mode and remote clients, especially when combined with finite history. | Medium/High | P1 | Medium/High: live-only stream deltas must not be mistaken for durable replay. | Keep three-layer `CoreEvent`; define which protocol events are replayable. |
| Scoped catalog/plugin transforms | Adapt | Improves plugin hot reload, provenance, and cleanup when config or plugin state changes. | Medium/High | P1 | Medium: can interact with existing plugin contributions and settings watchers. | Add transform ownership and replay for catalog-like domains before expanding to every plugin contribution. |
| Provider policy separated from provider configuration | Adopt | Cleanly separates "what exists" from "what is allowed"; aligns with multi-provider administration. | Medium | P1 | Low/Medium: migration from existing config must be explicit. | Keep coco-rs provider runtime; introduce policy as a separate evaluation layer. |
| Agent-bound permission snapshot per provider turn | Adapt | Prevents later agent switches from changing authorization for already-issued tool calls. | Medium | P1 | Medium: must integrate with current permission source precedence. | Store effective agent/policy context on tool-use records or execution context. |
| Session UI timeline patterns | Adapt | Improves readability of reasoning, tool calls, diffs, diagnostics, and subagent activity. | Medium | P1 | Medium: terminal constraints differ from web layout. | Borrow interaction grouping; implement through existing transcript cells and `tui-ui` primitives. |
| Composer-side permission/question/follow-up panels | Adapt | Makes active blocking states clearer and reduces transcript clutter. | Medium | P1 | Medium: needs careful keyboard and scrollback behavior. | Model as UI-only state derived from core events; do not make presentation state transcript-authoritative. |
| Child-session navigation affordances | Defer | Valuable for subagents, but depends on stable subagent/fork/worktree runtime semantics. | Medium | P2 | Medium: premature UI could encode unstable runtime behavior. | Revisit after subagent parity work settles. |
| Full EventV2-style event sourcing | Reject for now | It would add projection, migration, and recovery complexity while `coco-rs` already has a strong transcript authority. | Low/Medium | P2 | High: could duplicate or undermine `MessageHistory`. | Borrow replay/cursor contracts instead of wholesale storage architecture. |
| opencode v2 provider runtime routes | Reject | The current v2 runner route set is narrower than coco-rs provider support. | Low | P2 | Medium: copying it would regress provider breadth. | Only borrow catalog/policy concepts. |
| opencode TUI package boundary | Reject as implemented | The target boundary is good, but the current package still depends on core/plugin packages. | Low | P2 | Medium: copying it would weaken coco-rs's cleaner `tui-ui` seam. | Keep `tui-ui` domain-free and enforce dependency boundaries. |

## Recommended Roadmap

### P0: Session And Tool Correctness

1. Add process-local per-session execution coordination.
2. Add durable prompt admission records with idempotent admission receipts.
3. Add tool registry materialization identity and stale-call rejection.
4. Centralize core-executed model-visible tool output bounding.
5. Begin Context Epoch design with a minimal source set.

This set gives the highest architectural leverage because it tightens turn
ownership, restart behavior, and tool consistency without requiring a wholesale
storage migration.

### P1: Reconnectable APIs And Scoped State

1. Add finite session history reads over the existing transcript authority.
2. Define replayable protocol events while preserving the `CoreEvent` taxonomy.
3. Add replay-and-tail session event streams.
4. Introduce scoped catalog/plugin transforms for selected domains.
5. Separate provider policy evaluation from provider configuration.
6. Improve TUI state surfaces using opencode timeline/composer patterns.

This set supports app/server mode and richer clients, but it should follow the
P0 execution and session invariants.

### P2: Watch Or Defer

1. Revisit child-session navigation after subagent runtime semantics are stable.
2. Reassess EventV2-style projection only if JSONL transcript authority becomes
   a blocker for remote/session-server requirements.
3. Track opencode v2 provider and TUI boundary maturity, but do not copy current
   implementation boundaries.

## Non-Recommendations

Do not copy these opencode areas directly:

- The current opencode v2 provider runtime route set. It is less complete than
  coco-rs's provider architecture.
- The full EventV2 storage/projection model. Its replay contract is useful, but
  its storage model would be a high-cost shift.
- Current opencode TUI package dependencies. The extraction target is sound, but
  the implementation does not yet satisfy that target.
- Current public opencode config/permission docs as v2 truth. The v2 specs
  deliberately redesign several domains, so public docs and v2 architecture must
  be treated separately.
- Permissive permission defaults. coco-rs should keep its fail-secure posture.

## Open Questions For coco-rs

- Should durable prompt admission live inside the current session transcript
  backend, or as a parallel queue projected into `MessageHistory`?
- Which `CoreEvent` variants are stable enough to expose through a replayable
  session event stream?
- What is the minimum Context Epoch source set for an initial implementation:
  environment, date, instructions, selected agent guidance, or provider/model
  identity?
- Should provider policy be configured in the same file family as providers, or
  as a separate policy domain with its own source precedence?
- How much of the web-style session timeline should the terminal UI expose
  directly versus keeping as future desktop/web app guidance?

## Bottom Line

`opencode` v2 is a strong reference for durable session semantics and
client-oriented architecture. `coco-rs` should absorb the design principles that
improve correctness and reconnectability, especially prompt admission, execution
coordination, context epochs, and tool materialization. It should preserve its
own stronger provider, permission, compaction, transcript, and TUI boundaries.
