# Current Gap Fix Plan

Date: 2026-05-14

Scope: current open or stale gaps found by reviewing the existing planning
docs and probing the live `coco-rs/` code. This is a fix-ordering document,
not a replacement for the deeper source-of-truth docs. Once an item is fixed,
update the owning plan or `audit-gaps.md` and remove it from this list.

## Evidence Reviewed

- `docs/coco-rs/audit-gaps.md`
- `docs/coco-rs/parity-skills-commands-plugins.md`
- `docs/coco-rs/verification-report.md`
- `docs/coco-rs/event-system-design.md`
- Current code probes across `commands`, `skills`, `plugins`, `core/tools`,
  `app/query`, `app/cli`, `services/mcp`, and `exec/sandbox`.

Important freshness note: several old audit rows are stale. Current code shows
`PermissionChecker` preflight wiring for Read/Write/Edit, `RequiresAction`
emission, `permission_denials` accumulation, `/rewind`, `/init`, `/memory`,
`/commit-push-pr`, and the `/compact` manual entry-point already exist. Treat
older docs that say these are missing as cleanup work, not implementation work.

## Priority Model

- P0: release-blocking correctness or active security exposure.
- P1: user-visible broken behavior, protocol holes, or high-leverage runtime
  plumbing.
- P2: parity gaps with clear users but acceptable workarounds.
- P3: backlog, platform tail, or intentionally deferred parity.

No active P0 was confirmed in this review. The highest-priority live issues are
P1 because the old P0/P1 entries around sandbox preflight and SDK action state
have code and tests now.

## P1 Active Gaps

### P1.1 Plugin install and refresh lifecycle

Status: intentional parity (NOT an open gap for the install/refresh half).

Verified 2026-05-15 against TS reference:

- TS `utils/plugins/pluginInstallationHelpers.ts:588` ends a successful install
  with the same `"Run /reload-plugins to activate"` message — TS **also**
  defers to an explicit user action, not auto hot-reload.
- Rust `plugins/src/watcher.rs:9-11` documents the design choice explicitly:
  "The watcher is intentionally not hooked into the [`crate::PluginManager`]
  refresh path — that's the explicit `/reload-plugins` user action."
- `commands/src/handlers/plugin.rs:243` returns the matching message
  (`"Run /reload-plugins to activate"`).
- `app/cli/src/sdk_server/notifications/plugins.rs` already emits a
  `ServerNotification::PluginsChanged` so the SDK client sees the disk
  change.

Residual work (separate, smaller-scope gap, not P1.1):

- The standalone `coco plugin install` path in `app/cli/src/bin_handlers/plugin.rs`
  reports marketplace/URL installs as not implemented. This is a CLI parity
  surface item — track as P2 if user-visible.
- `app/cli/src/lib.rs` still documents URL-based plugin install as not yet
  implemented.

These remaining items do not warrant adding an auto-refresh path; matching TS
parity means keeping `/reload-plugins` as the activation surface.

### P1.2 SDK and MCP elicitation bridge

Status: implemented.

Verified 2026-05-15:

- `app/cli/src/sdk_server/handlers/mcp.rs:132` `build_send_elicitation` returns
  the closure attached at MCP connect time.
- `mcp.rs:195` `bridge_elicitation_to_sdk_client` allocates a request id, sends
  `mcp/requestElicitation` over the transport, awaits `ElicitationResolveParams`,
  and translates the SDK reply back into the rmcp protocol response.
- `app/cli/src/elicitation_hooks.rs:48-158` wraps the bridge with pre- and
  post-dialog hooks (matching TS `elicitationHandler.ts`).
- Failure cases: transport absent → error response (not silent drop); SDK
  client unresponsive → timeout error.
- Bridge is installed both on initial MCP connect (`mcp.rs:357`) and on
  reconnect (`mcp.rs:393`).

Residual risk: none for the elicitation path itself. Future enhancements
(retry policy, structured cancellation propagation) can build on the existing
bridge without re-architecting it.

### P1.3 MCP handle resource and auth methods

Status: implemented.

What changed:

- `app/cli/src/mcp_handle_adapter.rs` forwards `read_resource` through
  `McpConnectionManager::read_resource` and preserves every returned MCP
  content item instead of dropping to the first block.
- `McpConnectionManager::authenticate` is now the service-owned OAuth entry
  point. It mirrors the TS shape for HTTP/SSE servers: detect OAuth support,
  return a browser URL when login is required, wait for the callback in the
  background, and reconnect the server after credentials are available.
- `McpManagerAdapter::authenticate` now calls the manager entry point, so
  `McpAuthTool` and any future generic `McpHandle` caller share the same
  path. Non-OAuth transports return a clear "does not use OAuth" message
  instead of an internal "not implemented" error.

Verification:

- `services/mcp/src/client.test.rs` covers the non-OAuth transport branch.
- `app/cli/src/mcp_handle_adapter.test.rs` covers adapter auth forwarding and
  multi-content resource conversion.
- `core/tools/src/tools/mcp_tools.test.rs` covers the tool-level generic handle
  path for MCP resource/auth tools.

### P1.4 Tool Result Budget TS parity

Status: implemented.

Evidence:

- `core/tool-runtime/src/tool_result_storage.rs` now contains shared constants,
  Level 1 persistence helpers, and Level 2 replacement state.
- `app/query/src/tool_outcome_builder.rs` calls the Level 1 helpers for tools
  that opt in via `Tool::max_result_size_bound()` returning
  `ResultSizeBound::Chars(_)` rather than `ResultSizeBound::Unbounded`.
- `app/query/src/engine_finalize_turn.rs` wires a Level 2 pass before
  microcompact when `compact.tool_result_budget.enabled` is true.
- Bash/PowerShell model-visible persistence now goes through the shared
  session-scoped `tool-results/` root instead of a temp-dir path.
- Level 2 persists selected fresh candidates, stores exact
  `<persisted-output>` replacement strings, and leaves canonical history
  intact.
- `coco-session` writes and reconstructs content-replacement records for
  resume/fork.
- TS threshold defaults are mirrored: most tools inherit `100_000`, tighter
  tools override, and `Read` opts out with `i64::MAX`.
- MCP binary output is persisted through the same session storage root.
- `docs/coco-rs/tool-result-budget-plan.md` assigns the real Level 1 and Level
  2 pipeline to `coco-tool-runtime`, `coco-query`, and `coco-session`.

Residual risk: future MCP content block variants that are not currently exposed
through `McpHandle` may need additional persistence handling. Session-expiry
cleanup for `tool-results/` is implemented through `TranscriptStore`
housekeeping.

Verification:

- Runtime tests cover Level 1 persistence with Bash and non-Bash singleton text
  results.
- Query tests cover TS-style Level 2 aggregate replacement.
- Session tests cover content-replacement record round-trips and resume
  reconstruction.
- Focused crate tests and `just quick-check` have been used as the verification
  gate for this refactor.

### P1.5 MCP-sourced skills

Evidence:

- `SkillSource::Mcp { server_name }` exists and `/skills` can label MCP skills.
- `docs/coco-rs/parity-skills-commands-plugins.md` says the builder registry
  and consumer are missing.
- Code search did not find a current MCP skill builder registration path.

Risk: MCP servers that publish skills cannot expose them through the same
SkillTool and slash-command bridge as file or plugin skills.

Fix plan:

1. Add a write-once `coco-skills::mcp_builders` registry.
2. Wire MCP skill-list notifications to build `SkillDefinition` values.
3. Register/unregister those definitions with `SkillManager` on server connect,
   reconnect, and disconnect.
4. Surface them through `/skills`, `SkillTool`, and command-source metadata.

Verification:

- Add `coco-skills` tests for MCP frontmatter parsing and source metadata.
- Add `coco-mcp` or CLI integration tests for connect/disconnect lifecycle.
- Run `just test-crate coco-skills`, `just test-crate coco-mcp`, and
  `just quick-check`.

## P2 Active Gaps

### P2.1 Config and sandbox hot reload subscriber wiring

Evidence:

- `app/cli/src/tui_runner.rs` starts `RuntimeReloader`, but comments say
  QueryEngine integration that rereads `tool_overrides` and `api_client` per
  turn from the publisher is deferred.
- The runner does subscribe for config-change hooks, display settings reload,
  and reload-error toasts.
- `audit-gaps.md` also notes that `SandboxState::update_config` exists, but no
  subscriber reruns sandbox adapter input construction and calls it.

Risk: users see partial hot reload behavior. UI/config-change hooks may update,
but model/tool runtime snapshots and sandbox rules can stay stale until restart.

Fix plan:

1. Thread `RuntimePublisher` or a runtime snapshot reader into `QueryEngine`.
2. Refresh per-turn `tool_overrides`, feature gates, and role clients at a
   single turn boundary.
3. Subscribe sandbox state to the same publisher and call
   `SandboxState::update_config` after rebuilding adapter inputs.
4. Preserve cache-break detector continuity when the provider/model spec does
   not change.

Verification:

- Add a query test that changes a tool override between turns.
- Add a TUI runner test or seam test for model-role refresh.
- Run `just test-crate coco-query`, `just test-crate coco-cli`, and
  `just quick-check`.

### P2.2 MCPB completion and UI

Evidence:

- `plugins/src/mcpb.rs` now parses/extracts/cache-loads `.mcpb` and `.dxt`
  bundles and returns `NeedsConfig`.
- The same module still has a TODO for full JSONSchema validation.
- The earlier parity plan expected a TUI config overlay for first-time MCPB
  installs; current command paths still need confirmation against that UI.

Risk: bundled MCP server installs work only for the simplest config schema and
may not provide the expected first-time configuration UX.

Fix plan:

1. Finish the TS subset of schema validation: string, number, boolean, enum,
   default, required, and validation error text.
2. Confirm or implement the MCPB config overlay in TUI.
3. Wire slash-command and CLI install paths through the same MCPB loader.

Verification:

- Add schema validation table tests in `coco-plugins`.
- Add TUI overlay snapshot tests if UI is added or changed.
- Run `just test-crate coco-plugins`, `cargo test -p coco-tui` if snapshots
  change, and `just quick-check`.

### P2.3 Prompt file parts

Evidence:

- `CommandResult::Prompt { parts, .. }` supports file parts.
- `app/cli/src/tui_runner.rs` concatenates text parts but warns that
  `Prompt::File` parts are not yet rendered into engine input.

Risk: command handlers or plugin commands that return file attachments can lose
that context in TUI execution.

Fix plan:

1. Define the engine input representation for prompt file parts.
2. Teach TUI and SDK runners to pass file parts through the same attachment
   path used for user-provided files.
3. Add tests with mixed text/file prompt parts.

Verification:

- Add runner tests for text-only, file-only, and mixed prompt commands.
- Run `just test-crate coco-cli`, `just test-crate coco-query`, and
  `just quick-check`.

### P2.4 Hook Agent handler

Evidence:

- `app/query/src/hook_llm.rs` fully implements Prompt hooks.
- The Agent hook path is intentionally a stub that logs and returns
  `Cancelled`.
- Model-role routing for hook evaluations is already in place.

Risk: Agent-type hooks silently do not enforce custom policies unless users
only rely on Prompt hooks.

Fix plan:

1. Register `StructuredOutputTool` for hook-agent runs.
2. Fork a bounded `QueryEngine` with `max_turns = 50`.
3. Enforce "must call StructuredOutput before Stop".
4. Grant read access to the transcript path for the hook run only.

Verification:

- Add hook-agent tests for success, structured denial, timeout, and stop without
  structured output.
- Run `just test-crate coco-query`, `just test-crate coco-hooks`, and
  `just quick-check`.

### P2.5 Web preapproved host wiring

Evidence:

- `core/tools/src/tools/web.rs` contains `PREAPPROVED_WEB_HOSTS` and
  `is_preapproved_host`, but comments say they are not wired into the
  permission evaluator.

Risk: known safe documentation hosts still require approval, creating TS parity
and UX friction.

Fix plan:

1. Route WebFetch/WebSearch permission checks through the preapproved-host
   helper at the query-engine permission layer.
2. Keep exact-host and path-prefix semantics unchanged.
3. Add explicit tests for subdomain rejection and path segment boundaries.

Verification:

- Add permission-controller tests for preapproved and non-preapproved hosts.
- Run `just test-crate coco-query`, `just test-crate coco-tools`, and
  `just quick-check`.

### P2.6 Skills watcher parity

Evidence:

- The parity plan lists missing pieces in `skills/src/watcher.rs`: 1s stability
  threshold, ConfigChange hook integration, `.git/` ignore, additional-dir
  watched paths, and a reset-sent-skills analogue.

Risk: skill changes may reload at different times from TS, miss additional
directories, or bypass hooks that should gate reloads.

Fix plan:

1. Add watcher config for stability, debounce, and polling.
2. Filter `.git/` paths.
3. Watch configured additional dirs.
4. Execute ConfigChange hooks before cache clears.
5. Reset sent-skill tracking when reload succeeds.

Verification:

- Add watcher unit tests with a fake notify stream if available.
- Add a hook-blocking reload test.
- Run `just test-crate coco-skills`, `just test-crate coco-hooks`, and
  `just quick-check`.

### P2.7 Event-time reminder producers

Evidence:

- `audit-gaps.md` Round 13 defers six event-time reminder emitters.
- Current code has reminder adapters, but IDE bridge and swarm adapters still
  contain stub comments in `app/query/src/reminder_adapters.rs`.

Risk: context reminders can be incomplete around IDE/sibling/teammate state,
which affects model behavior more than protocol correctness.

Fix plan:

1. Inventory each deferred reminder producer against the current adapter set.
2. Implement missing producers at the subsystem boundary, not in prompt
   rendering.
3. Add snapshot tests for system-reminder assembly.

Verification:

- Run targeted `coco-system-reminder` or `coco-query` tests depending on final
  ownership, then `just quick-check`.

### P2.8 Managed-policy-only sandbox filters

Evidence:

- `audit-gaps.md` says TS supports `allow_managed_domains_only` and
  `allow_managed_read_paths_only`.
- Coco-rs has policy-source settings infrastructure, but sandbox adapter inputs
  currently carry flat permission-rule lists without source distinction.

Risk: enterprise policy cannot yet express "allow only managed-source domains
or read paths" with TS-equivalent enforcement.

Fix plan:

1. Preserve permission rule source metadata through config resolution.
2. Thread source-aware rules into sandbox adapter inputs.
3. Apply managed-only filters before building platform sandbox config.

Verification:

- Add config-resolution tests for rule source preservation.
- Add sandbox adapter tests for managed-only domains and read paths.
- Run `just test-crate coco-config`, `just test-crate coco-sandbox`, and
  `just quick-check`.

## P3 Active Gaps

- OTel span hierarchy and L3 application events remain broad implementation
  work. Keep them behind the existing `crate-coco-otel.md` ownership.
- GrowthBook remains a policy/architecture decision rather than a code-only
  gap.
- DirectConnect full session management and daemon mode are still long-tail
  remote/SDK work.
- IDE `/ide` command and IDE reminder adapters need bridge integration.
- TUI autocomplete still has TODOs for file search and LSP symbol sources.
- Agent overlay edit/delete says edits take effect next session.
- Provider tail: Bedrock / Vertex / Foundry construction is an **explicit non-goal**.
  `services/inference/src/auth.rs` retains env-based detection only for
  diagnostic clarity; `model_factory.rs` will never dispatch on these variants.
- Windows inner-stage sandbox remains platform-tail work.
- `relocateToolReferenceSiblings` remains a low-priority compact/message
  parity item.
- Deprecated or hidden slash-command stubs should remain hidden unless a real
  command body is ported.

## Stale Audit Items Cleaned Up

May 14 cleanup updated `audit-gaps.md`, `parity-skills-commands-plugins.md`,
and `verification-report.md` so these are no longer presented as open
implementation gaps. Keep this list as an audit trail for why the older rows
were changed:

- `PermissionChecker` has production preflight callers in Read, Write, and
  Edit through `core/tools/src/tools/sandbox_preflight.rs`.
- `RequiresAction` is emitted from `app/query/src/permission_controller.rs` and
  has query tests.
- `permission_denials` are accumulated in `app/query` and carried into SDK
  session results.
- `CommandSource` already has Skills, Plugin, Bundled, MCP, User, Project,
  Managed, Builtin, and deprecated command sources.
- `CommandResult` already has text, injected prompt, compact, prompt parts,
  open-dialog, and skip variants.
- `CommandRegistry` can build from `SkillManager` and `PluginManager`.
- `/rewind`, `/init`, `/memory`, and `/commit-push-pr` have current handlers
  and tests.
- `/compact` is no longer a plain text stub: the command emits a sentinel and
  TUI/SDK runners dispatch to `QueryEngine::run_manual_compact`.
- `MCPB` is no longer zero: a loader/cache/config-needs path exists, but schema
  validation and install UI parity remain active.
- The old verification-report rows around SDK `RequiresAction` and
  `permission_denials` are superseded by current `event-system-design.md` and
  code tests.

## Fix Sequence

1. Documentation cleanup: **done May 14**. `audit-gaps.md`,
   `parity-skills-commands-plugins.md`, and `verification-report.md` now mark
   the stale open rows as resolved, partial, accepted, or superseded.
2. Runtime plumbing: plugin live refresh, SDK elicitation bridge, generic MCP
   resource/auth forwarding.
3. Context pressure: Tool Result Budget shared pipeline.
4. Extensibility parity: MCP-sourced skills, MCPB UI/schema, Prompt file parts.
5. UX and reminder parity: web preapproval, skills watcher, reminder producers,
   TUI autocomplete, IDE integration.
6. Long-tail systems: hook Agent path, OTel, DirectConnect/daemon,
   Windows sandbox.

## Completion Gates

For any code patch from this plan:

1. Run the targeted crate tests listed on the item.
2. Run `just quick-check` from `coco-rs/`.
3. Run `just pre-commit` exactly once at the end before a commit.
4. For docs-only cleanup, `git diff --check` is sufficient unless code or
   generated artifacts changed.
