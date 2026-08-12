# Grok-Build Full Workspace Crate Audit

Status: complete point-in-time crate comparison, 2026-08-12.

This closes the release-diff/TUI scope gap. At grok-build
`b13fa526f5112c0b20dad5f1f2300d3d3b127895` it covers 81 workspace packages
plus the independently rooted, parent-excluded `xai-grok-markdown-fuzz`. The
root manifest is not a package, so the audited total is 82 crate manifests.

This is a crate and ownership audit, not a claim that every upstream source
line should have a coco analogue. For each crate the review identified its
runtime contract, traced the corresponding coco owner and live construction
path, and tried to disprove the proposed gap before accepting a port.

Companions: [release-diff audit](grok-build-release-crate-audit-2026-08.md)
and [TUI port plan](ui/grok-build-tui-port-plan.md).

## Decision rules

1. Port only a real missing correctness, security, or product contract.
2. Implement at coco's owner seam; do not reproduce upstream topology.
3. Prefer typed errors, bounded I/O, immutable config, and explicit lifecycle.
4. Reject duplicate queues, indexes, DTOs, editors, lifecycles, and storage.
5. Retain neither fail-open behavior nor ambiguous compatibility aliases.

## Net result

The full-workspace pass confirmed one additional shared capability gap beyond
the already-remediated release findings: coco-owned HTTP clients could not add
an enterprise TLS root while retaining webpki roots. Coco now owns this at the
leaf `coco-utils-extra-ca` seam:

```text
COCO_EXTRA_CA_BUNDLE
  -> capped one-time PEM read
  -> rustls X.509 validation
  -> version-neutral DER cache
     -> reqwest 0.12 async/blocking adapters
     -> rmcp-client's quarantined reqwest 0.13 adapter
```

The setting is additive and optional. Missing, oversized, malformed, and
zero-certificate bundles warn and preserve normal roots. All application-layer
reqwest client constructors use the policy; provider SDK crates remain
independent and production inference gives them the shared client through
`coco-inference`. Spawned teammates inherit the same
`COCO_EXTRA_CA_BUNDLE` value.

The other 45 members outside the prior 36-crate release diff exposed no
justified port: coco already owns the behavior, it is xAI-only, or it would add
a parallel authority.

## Cross-validation of the strongest candidates

| Candidate | Evidence in coco | Decision |
| --- | --- | --- |
| ACP cancellation | `NdjsonFrameReader` uses cancel-safe `fill_buf`, copies a complete frame, then synchronously consumes it; the legacy SDK reader task is not reused after teardown. | No wrapper port. |
| Hunk/journal stack | `FileHistoryState` already owns snapshots, transcript metadata, unified diffs, rewind, and external-change detection. | Actor attribution needs a product design; no competing actor/SQLite truth. |
| Memory index | `coco-memory` owns auto/session/team memory while `coco-retrieval` owns vector/BM25 code search. | Do not merge in a second FTS5/vector storage authority. |
| Circuit breaker / power | Failure policy is domain-owned; the sleep inhibitor keeps active turns awake, unlike grok's suspend-aware token-refresh contract. | No generic breaker or power-event refactor without shared semantics. |
| Crash reporting | Signal-context work is deliberately limited to async-signal-safe terminal restoration. | Persistent blobs/symbolization need privacy, retention, upload, and unsafe-platform contracts. |
| Tool hub / workflow / Mermaid | Coco already has tool-runtime/hub owners, bounded QuickJS workflows, and native-cell Mermaid rendering. | Do not import xAI wire protocols, a Rhai host, or the vendored SVG/raster graph stack. |

## All 82 crate manifests

| # | grok-build crate | coco owner and decision |
| ---: | --- | --- |
| 1 | `xai-proto-build` | Build helper for xAI protobufs. Coco has no matching schema; do not import service codegen. |
| 2 | `ptyctl` | `coco-utils-pty` and TUI PTY tests own the applicable lifecycle; no missing behavior found. |
| 3 | `ptyctl-cli` | Reference/debug CLI for grok's PTY controller; no production coco contract. |
| 4 | `xai-acp-lib` | AppServer transport is already cancellation-safe at its frame boundary; no wrapper port. |
| 5 | `xai-agent-lifecycle` | Agent/task lifecycle is session-scoped in `coco-agent-host` and `coco-query`; no second manager. |
| 6 | `xai-chat-state` | Coco session runtime owns chat state and event ordering; release change was non-semantic. |
| 7 | `xai-grok-compaction` | `coco-compact` already owns transport-neutral compaction, cancellation, and budgets. |
| 8 | `xai-grok-sampling-types` | `coco-inference`/`coco-llm-types` own typed provider data and clean errors; adopted relevant error presentation. |
| 9 | `xai-circuit-breaker` | Per-domain breakers are intentional; no generic abstraction without shared semantics. |
| 10 | `xai-grok-tools` | `coco-tools` plus `coco-tool-runtime` cover dispatch, limits, media, and tasks; UTF-8 tail defect was fixed. |
| 11 | `xai-computer-hub-core` | Maps to `coco-tool-runtime` and `coco-hub-*`; importing another registry/resolver would duplicate authority. |
| 12 | `xai-tool-protocol` | xAI Computer Hub wire schema has no coco interoperability contract; no port. |
| 13 | `xai-tool-types` | Coco's tool/runtime and common types are canonical; no parallel tool-description model. |
| 14 | `xai-tool-runtime` | Coco already has a unified runtime/dispatch/error owner; retain coco traits and notifications. |
| 15 | `xai-grok-tools-api` | xAI protobuf API is product-specific; no matching endpoint. |
| 16 | `xai-computer-hub-sdk` | Reconnect/pool/server behavior is split across coco hub and exec owners; do not copy the SDK facade. |
| 17 | `xai-tracing` | `coco-otel`, session tracing, and wire dump own diagnostics; no second subscriber stack. |
| 18 | `xai-file-utils` | xAI upload/product-event collection is not a generic file utility; no port. |
| 19 | `prod-mc-cli-chat-proxy-types` | Private chat-proxy DTOs have no coco peer; no wire compatibility added. |
| 20 | `xai-grok-auth` | `coco-provider-auth` owns credential providers and refresh; no matching bearer-suffix seam. |
| 21 | `xai-grok-version` | Lockstep upstream release number only; no port. |
| 22 | `xai-test-utils` | Coco uses `coco-test-harness`, cassette, and live-test helpers; adopt test ideas only when exercising a coco contract. |
| 23 | `xai-grok-config` | `coco-config` owns layered JSON/env/CLI resolution; TOML precedence is not imported. |
| 24 | `xai-tty-utils` | Coco PTY/process owners cover detach and bounded cleanup; forced exit/D-state work remains evidence-gated. |
| 25 | `xai-grok-env` | xAI endpoint presets are product-specific; coco's typed provider config remains canonical. |
| 26 | `xai-grok-extra-ca` | Confirmed gap; absorbed as bounded `coco-utils-extra-ca` and wired to every coco-owned reqwest client. |
| 27 | `xai-grok-sandbox` | Confirmed gap; portable fail-closed deny-glob enforcement landed in `coco-sandbox`. |
| 28 | `xai-grok-workspace-types` | Remote xAI workspace DTOs have no coco service; no protocol port. |
| 29 | `xai-interjection-core` | Session command/steering queues already own mid-turn input; no second interjection buffer. |
| 30 | `xai-token-estimation` | Token accounting is already owned by inference/context/compaction; release delta was style-only. |
| 31 | `xai-grok-test-support` | Mock/SSE/ACP fixtures overlap coco harnesses; port scenarios, not the support crate. |
| 32 | `xai-codebase-graph` | `coco-retrieval` and symbol search own code indexing/graph ranking; no separate graph service. |
| 33 | `xai-grok-paths` | Coco has absolute path, URI, and project path utilities; no third path type family. |
| 34 | `xai-crash-handler` | Terminal restoration is covered; persistent crash capture requires a separate privacy/observability design. |
| 35 | `xai-fast-worktree` | Coco's git/worktree owner has no xAI dashboard registry contract; no SQLite/CoW facade port. |
| 36 | `xai-gix-status` | Coco git status uses its own bounded call paths; no reproduced RLIMIT worker abort. |
| 37 | `xai-sqlite-journal` | Only relevant to upstream SQLite owners; adding it would introduce storage that coco sessions do not use. |
| 38 | `xai-fsnotify` | `coco-file-watch` provides caller-scoped watches; whole-tree causal streams/nested-repo pruning are not universally valid. |
| 39 | `xai-tracing-macros` | Direct `tracing` instrumentation is sufficient; no timestamp/timing macro layer needed. |
| 40 | `xai-grok-agent` | Agent definitions/prompts are owned by coco host/query; actionable no-operator denial was adopted. |
| 41 | `xai-grok-hooks` | `coco-hooks` already owns discovery, execution, policy, and orchestration. |
| 42 | `xai-grok-announcements` | Remote announcement feed/persistence is a product feature with no coco contract. |
| 43 | `xai-grok-config-types` | Coco config/common types remain canonical; remote restore DTOs are inapplicable. |
| 44 | `xai-grok-mcp` | Coco already quarantines rmcp/reqwest 0.13 in `coco-rmcp-client` and owns MCP auth/lifecycle. |
| 45 | `xai-grok-telemetry` | Product analytics/Sentry expansion is policy work; `coco-otel` remains the observability owner. |
| 46 | `xai-grok-sampler` | Generic transient retry is already covered; xAI edge-specific 525/526 policy is not widened globally. |
| 47 | `xai-grok-secrets` | `coco-secret-redact` owns outbound/log sanitization; no second regex policy. |
| 48 | `xai-mixpanel` | No Mixpanel product analytics contract; do not add a vendor client. |
| 49 | `xai-grok-http` | Shared HTTP construction now exists at coco owner seams, including extra roots; xAI user-agent/telemetry stays upstream. |
| 50 | `xai-grok-workspace` | Coco host filesystem, git, execution, and discovery are independently owned; remote proxy behavior is not applicable. |
| 51 | `xai-computer-hub-mcp-adapter` | Coco MCP and hub adapters already meet at typed owners; no xAI hub bridge. |
| 52 | `xai-grok-workspace-client` | xAI `workspace.*` RPC client has no coco server peer; no port. |
| 53 | `xai-hunk-tracker` | Coco file history/diff/rewind is canonical; actor attribution requires a separate product design. |
| 54 | `xai-grok-markdown` | Relevant grapheme/table correctness was adopted; coco renderer architecture remains native-scrollback oriented. |
| 55 | `xai-grok-markdown-core` | `coco-tui-markdown` already owns parsing/render analysis; no headless parser duplicate. |
| 56 | `xai-ratatui-textarea` | Coco retains its existing editor/input owner; UI details stay in the TUI plan. |
| 57 | `xai-grok-memory` | `coco-memory` plus `coco-retrieval` cover separate memory/search contracts; no duplicate SQLite/vector index. |
| 58 | `xai-grok-mermaid` | Coco renders Mermaid as terminal cells; raster PNG engine is incompatible with the chosen UI boundary. |
| 59 | `mermaid-to-svg` | Vendored dependency of upstream raster Mermaid; no direct coco use. |
| 60 | `dagre_rust` | Vendored layout dependency; avoid a second graph/layout stack. |
| 61 | `graphlib_rust` | Vendored Dagre graph dependency; no direct coco contract. |
| 62 | `ordered_hashmap` | Vendored graph dependency; coco uses existing ordered collections where semantics require them. |
| 63 | `xai-grok-models` | `coco-model-card` and provider config own model metadata/defaults. |
| 64 | `xai-grok-pager` | Dedicated TUI audit applies; alt-screen/app-owned scrollback is not ported. |
| 65 | `xai-grok-pager-render` | Renderer-specific improvements are filtered through coco's native-scrollback invariants. |
| 66 | `xai-grok-shared` | Shared upstream UI/config glue has no independent coco owner gap. |
| 67 | `xai-ratatui-inline` | Coco TUI UI/terminal surface already owns inline/native-scrollback writing. |
| 68 | `xai-grok-plugin-marketplace` | `coco-plugins` owns marketplace discovery, install, caching, and policy. |
| 69 | `xai-hooks-plugins-types` | Coco hook/plugin types are internal canonical DTOs; ACP extension wire compatibility is not required. |
| 70 | `xai-grok-shell` | Aggregator only; its changes were routed to session, sandbox, permission, task, inference, and TUI owners. |
| 71 | `xai-grok-shell-base` | Environment/process helpers already exist in typed coco utility owners; no facade copy. |
| 72 | `xai-grok-shell-session-support` | MCP and file-access tracking already have separate coco service/session owners. |
| 73 | `xai-grok-subagent-resolution` | `coco-subagent`, coordinator, and host own definition/runtime/resume resolution. |
| 74 | `xai-prompt-queue` | Coco command/steering queues own typed session input; no cross-UI queue DTO duplicate. |
| 75 | `xai-system-power` | `coco-utils-sleep-inhibitor` owns active-turn sleep policy; suspend-aware token refresh is a different contract. |
| 76 | `xai-workflow` | `coco-workflow` plus bounded `coco-workflow-runtime` already provide the script/host boundary. |
| 77 | `xai-grok-update` | Coco deliberately has version checking but no in-process self-updater; no silent product expansion. |
| 78 | `xai-grok-voice` | `coco-voice` owns remote/local STT configuration and lifecycle. |
| 79 | `xai-grok-pager-pty-harness` | Scenario ideas feed TUI tests; alt-screen assertions cannot be copied. |
| 80 | `xai-grok-pager-bin` | Boot/runtime/forced-exit behavior is TUI-specific; global `_exit` remains rejected. |
| 81 | `xai-grok-pager-minimal` | Reference minimal UI binary only; no production owner. |
| 82 | `xai-grok-markdown-fuzz` (excluded workspace) | Fuzzes upstream render modes. Coco should fuzz its own parser/render invariants rather than depend on this harness. |

## Architecture conclusion

Grok frequently extracts DTO/adapter/client crates around a remote xAI
platform. Coco should keep product-independent policy in leaf crates and
inject it into domain owners. Extra CA follows that boundary: one bounded DER
source, thin transport-version adapters, and no global HTTP manager or SDK
dependency inversion.

No compatibility alias or legacy behavior was added. In particular:

- theme `auto` remains the correct “follow terminal” policy and was not
  renamed to an equally ambiguous compatibility surface;
- permission without an approval bridge remains explicit deny, not implicit
  allow;
- invalid sandbox deny syntax remains an error, not a skipped rule;
- optional bad CA input preserves built-in trust roots, while valid input is
  additive rather than replacing platform trust.

Persistent crash reports, hunk actor attribution, session summaries, and
desktop-system theme following remain explicit product proposals. Crate parity
alone is not authorization to add them.
