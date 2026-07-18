# Sidechat architecture

Status: implemented and adversarially re-reviewed on 2026-07-18.

`/btw` is a local TUI feature that opens one ephemeral, multi-turn child
session. The child has independent admission, history, cancellation, UI state,
and lifecycle. It inherits bounded committed context from the parent, permits
only structurally read-only model tools, and never participates in persistence,
public session APIs, durable event sequencing, or Hub egress.

Backward compatibility with the former sentinel and one-turn fork is not a
goal. Those paths must not be reintroduced.

## 1. Review conclusion

The original refactor had the right product model—a real child session—but the
first implementation left several guarantees advisory or split across
incoherent owners. The following findings were reproduced in code and were not
false positives:

| Finding | Why it was real | Resolution |
|---|---|---|
| Child construction preceded registry reservation | Two starts could build expensive runtimes before ownership was decided; a parent transition could race promotion | `spawn_child_load` reserves `Loading + Child/Internal/LocalOnly + parent→child` atomically before the factory future is polled |
| Parent lifecycle did not own the child | Close/replace could strand or promote a child after the parent transitioned | Registry parent transitions block admission and transition the child first; AppServer owner tasks await child completion before parent close/replace |
| Local-only events used the durable envelope path | Child events could allocate sequence numbers or enter retention/Hub paths | The outbound router consults immutable slot policy and emits an ephemeral `SessionEnvelope` with no sequence or watermark |
| Hook restriction was advisory | Runtime, compaction, permission, task, MCP, and file-change dispatch sites could bypass local checks | `HookExecutionPolicy` is applied at every dispatch boundary; sidechat permits only PreToolUse, PostToolUse, and PostToolUseFailure |
| Context capture was unbounded and used syntactic user boundaries | Oversized fragments and synthetic user messages could split or distort inherited turns | Model-aware aggregate budgeting, an 8,192-token per-message cap, semantic user-origin checks, whole-turn suffix capture, and an explicit omission marker |
| Child construction re-folded configuration | History and runtime resources could describe different parent states | `SideChatSeed` owns the already-resolved engine, permission, model-runtime, tool, command, skill, and project-service inputs used by the child; no child disk re-fold or config watcher is installed |
| Parent and child shared cache-detector identity | Interleaved calls on a shared model runtime could compare unrelated baselines | Detector keys are a typed tuple of normalized query source, session cache scope, and agent id; compact requests carry the same session scope |
| Session identity stopped at the bridge | Untagged parent events could fold into the active child view | Every bridge event is wrapped in a session-scoped TUI delivery; lifecycle carries explicit parent and child ids |
| Only `SessionState` was swapped in the TUI | Inactive parent events could still overwrite child streaming, turn clocks, scroll, collapsed tools, or denial state | Those fields now move with the session projection; permission prompts and process-level dialogs remain deliberately global |
| `/btw` sentinel could reach remote/model input | A private control string was part of the prompt surface | The sentinel is deleted; TUI parses `/btw` directly, remote catalogs omit it, and raw `/btw` or legacy sentinel input is rejected |
| Close errors could leave stale bridge handles | The registry terminally removes a failed close, while the bridge previously retained a dead child surface | Bridge authority is retained only before commit and cleared whenever the registry slot is gone; the detached lifecycle pump delivers the terminal view transition |
| `ConversationKind` duplicated topology without owning behavior | It was unused while registry policy already controlled topology and runtime profile controlled capabilities | The unused enum was removed; ownership is now singular and explicit |

The remediation plan is therefore accepted with two corrections:

1. Do not add a parent-cache-prefix bootstrap. Correct bounded history is the
   contract; the child owns a new cache lineage. Detector isolation prevents
   false comparisons without coupling the child to mutable parent cache state.
2. Do not migrate the whole TUI into a second application-state graph. Keep the
   existing active fields for call-site stability, but swap the complete
   conversation projection and route every event by session id.

## 2. Authoritative owners

Each concern has exactly one source of truth.

| Concern | Owner | Type or operation |
|---|---|---|
| Runtime capabilities | `coco-agent-host` | `SessionExecutionProfile` |
| Hook families | `coco-hooks` | `HookExecutionPolicy` |
| Slot topology, visibility, egress | `coco-app-server` | `SessionRegistrationPolicy` |
| Parent→child admission | `LiveSessionRegistry` lock | `begin_child_load`, blocked-parent set, child index |
| Runtime construction inputs | `coco-agent-host` | `SideChatSeed`, `SessionRuntimeFactory::build_side_chat` |
| Inherited-history bounds | `coco-context` | `BoundedContext`, `capture_bounded_context` |
| Model tool boundary | `coco-query` | `SideChatReadOnlyHandle` |
| Active TUI identity | `coco-tui` | `ViewMode` |
| Session event routing | `coco-types` / TUI reducer | `TuiOnlyEvent::SessionScoped` |
| Local child authority | local AppServer bridge | child interactive surface and event pump |

`ProcessSessionKind` remains the PID/process-launch classification. It is not a
conversation-topology type.

## 3. Required invariants

### I-1: independent admission

Parent and child own different `SessionTurnCoordinator` instances. Starting,
interrupting, or finishing a child turn never reserves or cancels the parent.

### I-2: one child per parent

At most one Loading, Live, or Closing child exists for a parent. This is
enforced under the registry write lock, not by the bridge's capacity of two.

### I-3: structural read-only boundary

The child model may call only:

- builtin Read, Glob, and Grep; and
- builtin Bash when the complete command passes the existing read-only shell
  analyzer.

MCP, custom, unknown, malformed, and every other builtin tool are denied before
executor dispatch. The gate matches resolved `ToolId`, never a wire alias.

Passing the structural gate returns `Ask`, not `Allow`, so ordinary permission
rules, sensitive-path checks, and approval prompts remain authoritative.
`require_can_use_tool` prevents a hook auto-approval from bypassing the gate.

### I-4: ephemeral ownership

A sidechat owns no transcript, usage, file-history, goal, schedule, PID, title,
memory, skill-learning, SessionManager, durable sequence, retention, or Hub
artifact. It is not resumable.

### I-5: parent-owned lifetime

Parent clear, replace, close, and process shutdown transition and close the
child first. Child admission is blocked once the parent transition begins. A
loading child may finish construction only into the already-recorded closing
path; it cannot become independently Live.

### I-6: bounded committed inheritance

Only committed parent history is captured. The inherited budget is:

```text
min(32,768, model_context_window / 2, model_context_window - 8,192)
```

No inherited message may exceed 8,192 estimated tokens. When the full prefix
does not fit, capture keeps the newest complete semantic user-turn groups and
prepends a typed omission marker. If the newest required group cannot fit, the
operation returns `SideChatContextTooLarge` with a compact-first instruction.

Synthetic, virtual, transcript-only, and tool-child user messages do not create
turn boundaries. UTF-8 and tool-use/tool-result groups are never sliced.

### I-7: local-only visibility and egress

The child registration is `Child/Internal/LocalOnly`:

- absent from session list/read/turns/resume paths;
- rejected when remotely targeted even if its id is guessed;
- omitted from remote command metadata;
- routed only to attached local surfaces;
- assigned no durable `session_seq`; and
- never announced or enqueued to the Hub.

### I-8: session-scoped UI state

`ViewMode` is the sole active-view identity:

```rust
pub enum ViewMode {
    Primary,
    SideChat { parent_id: SessionId, child_id: SessionId },
}
```

The active and inactive projections contain conversation state plus streaming,
turn ephemera, transcript scroll, collapsed-tool state, and recent denials.
Tagged inactive-parent events fold into the hidden parent projection. Global
permission prompts, modal arbitration, connection state, theme, and process
toasts remain global by design.

Stale or unknown lifecycle events cannot switch a newer view because exit
requires both the expected parent id and child id.

## 4. Construction and first turn

```text
/btw <question>
  -> TUI parses BtwRequest directly
  -> bridge resolves live parent and generates child SessionId
  -> AppServer atomically reserves child under parent
  -> reserved owner task captures SideChatSeed
  -> factory builds SideChatReadOnly runtime using captured resources
  -> bounded context + enforced boundary are appended silently
  -> registry promotes child or honors close-after-load
  -> bridge attaches child interactive authority and tagged pump
  -> TUI receives SideChatEntered(parent, child)
  -> TUI switches projection
  -> ordinary turn/start targets the child surface
```

Reservation precedes capture and construction. The first turn starts only after
the TUI switch, so the first emitted child event cannot be folded into the
parent view.

The sidechat boundary says only what code enforces: inherited messages are
reference material, the parent conversation is not mutated, model-directed
tools are read-only, and normal permissions may still ask.

## 5. Follow-up, interrupt, and close

While the child is open:

- plain input starts another child turn;
- a second `/btw` is rejected;
- only `/help` and `/btw --close` are accepted slash commands;
- Ctrl+C during a child turn interrupts that child turn; and
- Ctrl+C with an empty idle composer, or `/btw --close`, closes the child.

Close is owned by the AppServer lifecycle task. The bridge does not discard its
surface before the registry transition. Once terminal removal commits, bridge
authority is cleared even if runtime teardown reported an error. The child pump
is detached long enough to forward `SessionEnded`, sends
`SideChatExited(parent, child)`, and terminates.

Parent close and replace use the same close callback for child and parent and
wait on the child's completion before closing the parent runtime.

## 6. Hook and permission order

```text
model tool call
  -> permitted PreToolUse hook
  -> mandatory sidechat structural gate
  -> ordinary builtin permission evaluator
  -> optional child-scoped approval request
  -> executor
  -> permitted PostToolUse or PostToolUseFailure hook
```

Every other hook family is denied by policy, including SessionStart,
SessionEnd, Setup, Stop, compact hooks, permission hooks, task hooks,
elicitation, file/config/cwd changes, and notification hooks. The policy is
checked in both session-runtime and query-engine dispatch paths so adding a new
hook family defaults to denied for sidechat.

User-authored tool lifecycle hooks can themselves have side effects. They are
explicit policy automation and are outside the model-directed capability
claim.

## 7. Cache behavior

The child does not borrow mutable parent detector or bootstrap state. It starts
with the bounded parent messages and then owns ordinary child history and
provider cache writes.

Multiple sessions may share a `ModelRuntime`, so detector state is keyed by:

```text
(normalized query source, session cache scope, optional agent id)
```

`compact` normalizes to the main-thread source but retains both the session
scope and agent id. Compaction and cache-deletion notifications use the same
tuple. This preserves diagnostics without conflating parent, child, or
concurrent agent baselines.

`ContextFidelity::FullPrefix` means all committed parent messages fit verbatim;
it does not promise reuse of a parent provider-cache entry.

## 8. Failure handling

- Factory failure removes the child reservation and parent index.
- Attach failure closes the registered child through the ordinary lifecycle.
- A second concurrent start fails before its factory is polled.
- Parent transition closes child admission under the same registry lock.
- Close callback failure is terminal: waiters receive the error, surfaces and
  slot are removed, lifecycle is delivered, and no stale bridge handle remains.
- Late events for a closed child are ignored by session-id routing.
- Remote raw `/btw` and the legacy sentinel return INVALID_REQUEST and never
  become model input.

## 9. Verification map

The implementation is covered at the owner boundaries:

| Contract | Tests |
|---|---|
| Context budgets, semantic boundaries, omission, oversized fragments | `coco-context` side_chat tests |
| Read-only tool matrix and mandatory permission fallthrough | `coco-query` side_chat tool-gate tests |
| Runtime capability and hook allowlist | agent-host execution-profile tests |
| Atomic child reservation and parent-close loading race | AppServer registry and owner-task tests |
| Internal/public registration policy | AppServer registration-policy tests |
| Local-only sequence and retention behavior | AppServer routing and agent-host local-bridge tests |
| Parent/child cache detector isolation, including shared agent id | inference cache-detection tests |
| Tagged projection folding and stale exit rejection | TUI state tests |
| Idle Ctrl+C close routing | TUI update tests |
| Direct `/btw` parsing and non-TUI fallback | command tests |
| Remote catalog omission and internal-target rejection | command and agent-host bridge tests |
| Parent turn remains undisturbed | CLI `/btw` integration test |

Required validation for changes touching this architecture is the focused owner
tests plus workspace formatting, quick-check, and pre-commit validation.

## 10. Non-goals

- compatibility with the old one-turn fork or sentinel;
- remote SDK/headless sidechat support;
- persistence or resume of sidechat children;
- more than one child per parent;
- model-directed mutation under any permission mode;
- merging child content into the parent transcript; or
- a Hub schema for child conversations.
