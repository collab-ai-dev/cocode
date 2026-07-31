# coco-rs core alignment review against Claude Code 2.1.220

## Executive verdict

`coco-rs` is not a thin or partial port of the Claude Code core. Its agent
loop, tool runtime, permission pipeline, compaction system, subagent runtime,
workflow engine, MCP lifecycle, hooks, skills, tasks, memory, headless protocol,
and TUI are already substantial native Rust implementations with clear crate
boundaries.

The review found five actionable correctness or architecture issues in the
shared core. All five are fixed in this change:

- missing monotone Agent and WebSearch session limits;
- an outdated subagent nesting default and missing operator override;
- race-prone in-process agent capacity and start transitions;
- incomplete production child-engine sandbox/tool-runtime inheritance;
- WebSearch domain filtering that accepted non-boundary suffixes.

The remaining differences are primarily product surfaces or explicit Rust
design choices, not defects in the central agent loop.

## Method and evidence

The comparison used three evidence levels, in descending order of authority:

1. Claude Code 2.1.220 extracted source at
   `/lyz/codespace/claude-code-bomb/versions/2.1.220`, especially the verified
   depth and session-limit paths in `extract/cli_inner_pretty.js`.
2. The feature-organized analysis under
   `claude_code_v_2.1.220/analyze`, treating its provenance registers as leads
   rather than conclusions, as its own overview requires.
3. The readable 2.1.88 source at `/lyz/codespace/3rd/claude-code`, used to
   distinguish mature architecture from later deltas when the 2.1.220 report
   was incomplete.

The Rust review followed behavior from the model-facing tool boundary through
`ToolUseContext`, `QueryEngine`, `SessionRuntime`, coordinator dispatch, and the
execution backends. This matters because several apparent gaps in one crate are
actually filled by the production engine factory or session wiring.

## Core module assessment

- **Agent loop, state, and LLM API: aligned.** `app/query` owns a typed,
  multi-turn engine with streaming tool execution, continuation reasons,
  recovery, usage accounting, prompt-cache controls, and fork isolation. The
  Rust split is more explicit than the bundled JavaScript and follows the
  repository's dependency direction.
- **Tools: aligned with provider-specific differences.** File, search, shell,
  web, Agent, task, plan, MCP, scheduling, worktree, and structured-output
  tools are present. The OpenAI WebSearch provider still falls back to
  DuckDuckGo rather than using a native backend; that is a declared capability
  gap, not an accidental path.
- **Plan mode and permissions: aligned.** Live permission state, plan/auto
  reconciliation, read-only floors, Bash classification, SDK/TUI approval
  bridges, and fail-closed non-interactive behavior are implemented. Upward
  routing for residual `PermissionMode::Bubble` prompts remains incomplete.
- **Compaction and context accounting: aligned.** Full, micro, reactive, and
  session-memory paths are integrated with usage and persistence. Existing
  tests and design docs cover retry/failure breakers and cache-safe forks.
- **Subagents, teams, and background tasks: aligned in the local runtime.**
  Typed spawn requests, model-role resolution, worktree isolation, summaries,
  task registry integration, handoff review, mailboxes, and teammate lifecycle
  exist. The Rust default live-agent ceiling is deliberately tighter (`8`
  rather than upstream's `20`) and remains configurable.
- **Workflow: aligned.** The QuickJS sandbox, deterministic globals, source
  precedence, metadata validation, concurrency, resume journal, nested-run
  state, progress reduction, dispatch screening, and completion diagnostics are
  documented in `workflow-alignment-2.1.220.md`.
- **Sandbox and shell: aligned on supported operating systems.** The main and
  production child engines now share the live sandbox enforcement object and
  session shell services. Claude Code's Windows user-sandbox product path is
  not reproduced as an equivalent platform feature.
- **MCP, hooks, skills, tasks, and memory: broadly aligned.** These are native
  services rather than tool-local implementations, which improves ownership
  and testability. Some Claude-hosted marketplace and managed-policy behavior
  necessarily differs.
- **Models, authentication, and API reliability: intentionally broader.**
  `coco-rs` supports multiple providers and therefore cannot copy Claude-only
  catalog, entitlement, and transport assumptions literally. Retry, watchdog,
  OAuth, role selection, and cache metadata live behind provider-neutral
  seams.
- **Headless and UI: core aligned, product surfaces partial.** The SDK protocol,
  event taxonomy, stream output, TUI, accessibility state, and session browser
  are implemented. Claude remote control, first-party Chrome integration, and
  a complete IDE bridge are separate product surfaces; the IDE reminder adapter
  is still explicitly a stub.

## Session-owned atomic loop breakers

**What it does:** Enforces the upstream-compatible defaults of 200 Agent
dispatches and 200 WebSearch calls per conversation session, with validated
`COCO_MAX_SUBAGENTS_PER_SESSION` and
`COCO_MAX_WEB_SEARCHES_PER_SESSION` overrides.

**How it works:**

1. `SessionUsageLimits` stores each monotone count in an `AtomicI32` and each
   resolved maximum as immutable session configuration.
2. `fetch_update` performs comparison and increment as one atomic operation;
   concurrent tool batches cannot all pass a separate load and overshoot.
3. `SessionRuntime` owns one `Arc<SessionUsageLimits>` and installs it on every
   per-turn, fork, and child engine through the existing `wire_engine` funnel.
4. `ToolUseContext::clone_for_concurrent` clones the `Arc`, not the counters, so
   parallel calls charge one shared budget.
5. A newly constructed session runtime, including the runtime created by
   `/clear`, receives fresh counters. Rebuilding `QueryEngine` for another user
   message does not reset them.
6. Invalid and non-positive environment values fall back to 200. A limit can
   never be accidentally disabled by malformed configuration.

Edge cases: validation failures before dispatch are not charged. Agent runtime
failures after the dispatch boundary remain charged. WebSearch cache hits are
charged because the upstream contract counts tool calls, not backend requests.
WebSearch exhaustion returns model-visible guidance instead of a generic tool
error, preventing an automatic retry loop.

**Why this approach:**

- Session ownership matches the upstream reset boundary and avoids coupling a
  safety budget to UI state or active-task lifetime.
- Atomics are sufficient because the operation is a two-field-independent
  monotone counter; an async mutex would add scheduling and poisoning concerns
  without protecting a richer invariant.
- Reusing the background task registry was considered, but completed agents are
  removed there while the session budget must never decrement.
- Persisting the counters was rejected: upstream resets on `/clear`, and a
  process/session restart is already a stronger boundary.

**Key insight:** A session cap is a monotone historical budget, while
concurrency is a live leased gauge. Treating them as the same state is the
architectural error that causes counters to reset when tasks finish.

## Subagent depth policy

**What it does:** Aligns the default maximum spawn depth to 3 and supports the
validated `COCO_MAX_SUBAGENT_SPAWN_DEPTH` override.

**How it works:**

1. `subagent_depth_limit()` resolves an integer greater than or equal to one;
   otherwise it returns the default constant `3`.
2. The Agent tool resolves the limit once per call and rejects a caller whose
   `query_depth` is already at the ceiling.
3. Foreground and background tool-filter planners use the same resolver, so a
   child beyond the ceiling cannot retain the Agent tool through a different
   dispatch path.
4. Forks count because the child depth is derived at the spawn boundary and
   installed on the child engine.

Edge cases: the leaf at the exact ceiling may still see Agent in a legacy tool
snapshot, but the execution guard rejects it deterministically. Invalid
overrides do not produce a zero-depth session.

**Why this approach:**

- A policy function keeps the default and override in the pure subagent crate,
  shared by filtering and execution.
- Adding the value to every spawn DTO was considered, but it would duplicate a
  process-level operator policy across serialization boundaries.
- Reading the environment at the policy boundary is a negligible cold-path
  cost and avoids a global cache that makes embedded tests order-dependent.

**Key insight:** The important invariant is not merely a constant; every path
that controls Agent visibility or execution must resolve the same ceiling.

## Atomic runner lifecycle

**What it does:** Prevents concurrent registration from exceeding capacity and
prevents rejected execution handles from running without a tracking entry.

**How it works:**

1. `register_agent` acquires the map's write lock before checking capacity and
   duplicate identity.
2. It constructs and inserts the entry while holding that same guard, making
   check-and-reserve one transaction.
3. Each entry records an explicit `started` flag; a second start cannot replace
   the first completion receiver.
4. `start_agent` validates the entry before spawning its forwarder.
5. Unknown or duplicate starts call `JoinHandle::abort()` before returning.
6. Constructor input is clamped to at least one, eliminating negative-to-`usize`
   casts that otherwise create an effectively unbounded runner.

Edge cases: cancellation can remove an entry between registration and start;
the subsequent start now aborts the already-created child task. Taking the
completion receiver no longer makes a running entry appear startable again,
because `started` is independent of receiver ownership.

**Why this approach:**

- A write-locked transaction is clearer than an atomic count plus rollback,
  because duplicate identity and map insertion are part of the same invariant.
- Holding the guard is cheap: no I/O or `.await` occurs after acquisition.
- An owned task wrapper was considered, but would add lifecycle machinery when
  the existing map is already the authority.

**Key insight:** Dropping a Tokio `JoinHandle` detaches the task. A `false`
return is not sufficient cleanup unless the rejected handle is explicitly
aborted.

## Child runtime inheritance

**What it does:** Ensures production subagents execute with the parent
session's live sandbox and resolved tool services instead of default or inert
standalone values.

**How it works:**

1. `QueryEngineAdapter` continues to build a portable child config with safe
   standalone defaults.
2. The production factory in `agent_handle_factory` overlays resolved compact,
   tool, sandbox, memory, shell, web, LSP, and plan settings.
3. It now also installs the session's shared `SandboxState`; shell commands in
   child engines therefore use the same hot-reloadable enforcement object.
4. The parent's `ShellProvider` and output rewriter are cloned into the child,
   preserving shell snapshots, session environment hooks, `/env`, prefixes,
   and output compression.
5. `cwd_override` remains child-specific, so sharing shell services does not
   collapse worktree isolation.
6. The standard `SessionRuntime::wire_engine` path still installs shared
   handles, observers, the task registry, and the new session limits.

Edge cases: standalone tests and embeddings without `SessionRuntime` retain
safe defaults. Runtime-only `Arc<dyn Trait>` values do not enter serialized
coordinator DTOs. Child session CWD is not shared with the parent.

**Why this approach:**

- The production factory is already the documented overlay boundary; extending
  it preserves layering and minimizes the public configuration surface.
- Adding sandbox and shell handles to `AgentQueryConfig` was rejected because
  that DTO also crosses process/serialization boundaries.
- Constructing a second `SandboxState` per child was rejected because it would
  miss live updates and duplicate enforcement resources.

**Key insight:** Configuration parity needs both descriptive settings and the
live enforcement object. Copying `SandboxSettings` while leaving
`sandbox_state = None` describes a sandboxed child that actually runs
unsandboxed.

## DNS-boundary domain filtering

**What it does:** Makes WebSearch allow/block lists match an exact host or a
real subdomain, never an arbitrary string suffix.

**How it works:**

1. Configured domains are trimmed and normalized for optional leading/trailing
   dots.
2. Exact host equality is accepted.
3. A suffix is accepted only when the remaining prefix ends in `.`, which is a
   DNS label boundary.
4. Empty configured domains never match.

Edge cases: `github.com` matches `github.com` and `gist.github.com`; it rejects
`evilgithub.com` and `github.com.evil.test`.

**Why this approach:**

- The predicate is allocation-light and sufficient because URL parsing has
  already extracted the host.
- Plain `ends_with` was simpler but unsafe for domain policy.
- Public-suffix-list matching was considered unnecessary: the contract is
  caller-supplied host ancestry, not registrable-domain ownership.

**Key insight:** Host suffix matching is only valid when the suffix begins at a
DNS label boundary.

## Remaining optimization backlog

Priority reflects correctness and architecture impact, not feature-count
parity.

1. **P1: implement `PermissionMode::Bubble` routing for child engines.** The
   type exists, but the adapter still documents that residual child prompts
   cannot be routed upward. Until then, fail-closed is correct but less capable
   than upstream interactive subagent approval.
2. **P2: decide whether the live-agent default should remain 8.** Upstream's
   concurrent-subagent default is 20. Rust's tighter configurable ceiling
   reduces memory and open streams but makes wide workflow/Agent fan-out less
   compatible. This should be benchmarked, not changed by literal parity.
3. **P2: replace the OpenAI WebSearch fallback with a native provider path or
   reject the configuration explicitly.** Silent provider substitution is
   operationally surprising even though it is logged once.
4. **P2: complete or remove the IDE reminder stub.** The event and reminder
   types exist, but `app/query/src/reminder_adapters.rs` explicitly supplies a
   no-integration placeholder.
5. **P3/product: remote control, first-party Chrome, Windows user sandbox, and
   Claude-hosted marketplace/managed-policy behavior.** These require product
   transports and platform commitments outside the core agent architecture.

## Files changed by this review

- `core/tool-runtime/src/session_usage.rs`: session-owned atomic limits.
- `core/tool-runtime/src/context.rs`: shared limits on every tool context.
- `app/query` and `app/agent-host`: engine/session wiring and production child
  runtime inheritance.
- `core/tools/src/tools/agent/agent_tool.rs`: Agent session cap and depth gate.
- `core/tools/src/tools/web.rs`: WebSearch session cap and domain predicate.
- `core/subagent/src/filter.rs`: default depth and operator resolver.
- `coordinator/src/runner.rs`: atomic registration and safe start lifecycle.
- `common/config/src/env.rs`: typed `COCO_` environment keys.
- Focused tests cover counter races, registration races, rejected task abort,
  duplicate starts, tool-limit short circuits, and DNS-boundary matching.
