# coco-commands

Slash command registry and built-in implementations (help, config, clear, compact, model, session, mcp, plugin, diff, commit, pr, review, doctor, ...).

## Key Types
- `CommandHandler` trait — primary method `execute_command(&self, args: &str) -> Result<CommandResult>`; `execute(args) -> Result<String>` is a legacy default-error shim only
- `RegisteredCommand` — metadata (`CommandBase` from coco-types) + optional handler + `is_enabled` feature-flag gate
- `CommandRegistry` — name-keyed map with alias lookup; `execute` / `execute_command` dispatch; filter views: `visible()`, `client_visible()` (strips `is_sensitive`), `safe_for(CommandSafety)`
- `BuiltinCommand` / `AsyncBuiltinCommand` — sync and async built-in handler wrappers
- `builtin_base()`, `builtin_base_ext()` — construct default `CommandBase` with safety + argument-hint options
- `register_builtins()` (lib.rs) + `register_extended_builtins[_with_cwd]()` (implementations)

## Modules
- `handlers/` — richer command handlers that need app state
- `implementations/` — extended builtin registrations and shared `names` constants

## Deliberately Not Ported

**Audits should skip the commands listed below — these are conscious omissions,
not gaps.** If a future change re-introduces one, remove the row and add the
command to the registry.

### Group A — Provider / account-specific (Anthropic-only flows)

No single sign-in / billing / account surface applies across providers.

| Command | Reason |
|---|---|
| `/bug` | Anthropic `/feedback` alias. Intentionally unregistered so prompts and docs point at `/feedback` + the `collab-ai-dev/cocode` issue tracker. |
| `/fast` | Claude.ai/console-only fast-mode picker; coco-rs exposes fast-mode via `FastModeState` + Ctrl+Shift+F keybind only. |
| `/release-notes` | Fetches Anthropic-hosted changelog; CLI subcommand only in coco-rs, not slash-invoked. |
| `/privacy-settings` | `isConsumerSubscriber()`-gated; calls Anthropic Grove API. |
| `/rate-limit-options` | Claude.ai-only, hidden internal. |
| `/reset-limits` (+ non-interactive) | Upstream source is a literal `isEnabled: () => false` stub. |
| `/install-github-app` | `claude-ai`/`console` availability + Anthropic GitHub App OAuth. |
| `/install-slack-app` | `claude-ai` availability + Anthropic Slack App marketplace. |
| `/chrome` | `claude-ai` availability; Chrome-extension-only settings UI. |
| `/mobile` (aliases `/ios`, `/android`) | claude.ai mobile-app QR flow. |
| `/desktop` (alias `/app`) | `claude-ai` + macOS/win32 only; Anthropic desktop client install. |
| `/passes` | claude.ai referral / Passes program. |
| `/terminal-setup` | NOT actually provider-specific (corrected): generic Shift+Enter newline keybinding installer for VS Code / Apple Terminal. Deferred as a low-priority generic port — port if users ask. |
| `/extra-usage` (+ non-interactive) | Anthropic admin-overage request flow. |
| `/think-back` / `/thinkback-play` | Statsig-gated experimental Anthropic feature. |
| `/stickers` | Anthropic sticker-merch order flow (claude.ai-account-only). |

### Group B — Anthropic-internal stubs / first-party-only

Upstream source is a literal stub, or depends on Anthropic-internal
infrastructure (KAIROS, CCR, advisor API beta) coco-rs does not ship.

| Command | Reason |
|---|---|
| `/advisor` | Server-side Anthropic API beta `advisor-tool-2026-03-01`, first-party-only. |
| `/ultraplan` | `feature('ULTRAPLAN')`; depends on Claude-Code-on-Web ("CCR") session backend. |
| `/ultrareview` | CCR-backed multi-agent review with no local execution path. |
| `/bughunter`, `/autofix-pr`, `/issue`, `/ant-trace` | Upstream sources are literal stubs (`/ant-trace` was an Anthropic-only OTel trace toggle). |
| `/onboarding`, `/share`, `/teleport`, `/backfill-sessions`, `/break-cache`, `/mock-limits`, `/good-claude`, `/perf-issue`, `/oauth-refresh` | Literal `isEnabled: () => false` stubs in `INTERNAL_ONLY_COMMANDS`. |
| `/heapdump` | Node.js V8 heap snapshot; no Rust runtime equivalent. |
| `/ctx_viz` | Anthropic-internal context probe; in `INTERNAL_ONLY_COMMANDS`. |
| `/brief` | KAIROS-only (`feature('KAIROS_BRIEF')`); depends on Anthropic-internal `BriefTool`. |
| `/bridge-kick` | Real but `USER_TYPE==='ant'`-gated bridge-failure-injection diagnostic; `INTERNAL_ONLY_COMMANDS`. |
| `/init-verifiers` | `type:'prompt'`, `INTERNAL_ONLY_COMMANDS` (ant-only); generates Verify-agent verifier skills. |

### Group C — Feature-gated upstream optionals (compiled out of the public build)

Gated behind GrowthBook/`feature(...)` flags off in the public bundle
(dead-code-eliminated) or claude.ai-only subscriber/policy checks; coco-rs
ships no equivalent backend.

| Command | Upstream gate |
|---|---|
| `/fork` | `feature('FORK_SUBAGENT')` (off). Distinct from `/branch` alias `fork`. |
| `/web` (`web-setup`) | `availability:['claude-ai']` + GrowthBook `tengu_cobalt_lantern`; GitHub-connect for Claude-Code-on-Web. |
| `/buddy` | `feature('BUDDY')` (off) — companion-sprite UI. |
| `/proactive` | `feature('PROACTIVE')‖feature('KAIROS')` (off). |
| `/assistant` | `feature('KAIROS')` (off). |
| `/remote-env` | `isClaudeAISubscriber() && isPolicyAllowed('allow_remote_sessions')` — teleport remote-env config. |
| `/remote-control` (alias `/rc`) | `feature('BRIDGE_MODE') && isBridgeEnabled()`. coco ships `coco-bridge`; wire if/when bridge UX is finalized. |
| `/peers` | `feature('UDS_INBOX')` (off) — agent-to-agent UDS inbox. |
| `/torch` | `feature('TORCH')` (off). |

### Re-introducing one of these

Treat it as a feature add, not a bug fix: remove the row, implement in
`implementations.rs` or `handlers/`, and hide Anthropic-only infrastructure
behind a `Feature` gate so non-Anthropic providers stay clean.

## Deferred (registered but thinned)

These commands ARE registered and respond, but the body is intentionally
simpler than the full feature. Don't flag them as missing — DO update this
table when a gap closes.

| Command | Rust state | Gap |
|---|---|---|
| `/insights` | `register_static_prompt` body in `prompts/insights.txt` | Full behavior (Opus facet extraction, remote-session SCP, JSONL parsing) delegated to the agent via prompt. |
| `/workflow` (alias `/workflows`) | Prompt command (`prompts/workflow.txt`, `allowed_tools=["Workflow"]`); bare form opens the picker, `<name>` launches via the Workflow tool as a `local_workflow` background task | No workflow editor/creation UI yet. |
| `/ide` | Static text stub in `ide_handler` | Not wired to `coco-bridge` (IDE detect / auto-connect / MCP cache invalidation). Wire when bridge UX is finalized. |
| `/help` | Hardcoded `CATEGORIES` in `handlers/help.rs` | User skills, plugin contributions, MCP tools don't appear. Needs handler-side registry access (`execute_command` doesn't carry one). |
| `/color` | `dispatch_color` writes only live `app_state.agent_color` | Ephemeral; should persist to settings/session metadata. |
| `/diff` | Plain form renders uncommitted git diff; TUI intercepts `/diff session` and `/diff turn <message-id>` for file-history snapshot diffs | SDK/headless expose only the git-diff text handler. |
| `/tasks` (alias `bashes`) | No-arg opens the background-tasks modal; `list` / `detail <id>` / `cancel <id>` use the live `TaskRuntime` | Full interactive output scrolling is TUI-side follow-up. |
| `/mcp` | Async overlay for list/add/remove/enable/disable | Interactive wizard UX (xaa IDP, add-server) thinned. |
| `/hooks` | Async overlay shows hook configs; `reload` reloads the live registry | Read-oriented; no interactive editing. |
| `/sandbox` (file `sandbox-toggle`) | Sync handler writes canonical modes; supports `exclusions`, `exclude`, `unexclude` | Per-platform availability panel text still thin. |
| `/doctor` | Async health-check text report | Install-method/auto-updater status not applicable to coco's distribution. |
| `/status` | Sentinel → live `runtime.status_report()`; TUI opens read-only panel, SDK/headless text | Panel jump actions to model/settings/permissions/sandbox not implemented. |

## Interactive-only commands (TUI; no SDK/headless path)

`/export`, `/branch` (alias `/fork`), and `/btw` do their real work in the TUI
slash dispatch (`app/cli/src/tui/slash_execution.rs` / `slash_resolution.rs`),
not the registry sync handler — they don't run meaningfully in headless `-p`
mode. Registry handlers return honest usage guidance for non-interactive
surfaces; `/btw` additionally has an SDK `turn/start` handler path (shared
`coco_agent_host::side_question`).

- `/export <filename>` writes the conversation under cwd; format inferred from
  extension (`.md`/`.json`/else text); no-arg opens the format picker.
  Clipboard export is `/copy` (coco split).
- `/branch` forks the on-disk transcript (`recovery::fork_conversation`,
  relabeling `session_id`) + live-switches via the `/resume` hydration path.
- `/btw` answer is model-invisible but transcript-visible (TS modal is fully
  ephemeral) — see `handlers/btw.rs`.

## Always-Enabled General-Purpose Commands

Plain Rust features with no gating. **Do not introduce `is_enabled` for
these** — intentionally available to every user.

| Command | What it does in coco-rs |
|---|---|
| `/version` | Prints `cocode v{CARGO_PKG_VERSION}`. |
| `/feedback` | Prefilled `collab-ai-dev/cocode` GitHub issue URL (version, commit, build time, OS, arch, timestamp). Logs excluded by default; `--with-logs` adds a redacted best-effort tail with a review reminder. No `/bug` alias. |
| `/tag` | Toggles a searchable tag on the current session via `SessionManager::toggle_tag` (sentinel-based dispatch). |
| `/files` | Lists `git ls-files` grouped by top-level directory with rough context-size estimate. |

## Rewind / Resume Naming

- **`/rewind`** — in-session TUI checkpoint picker (`openMessageSelector`
  semantics); operates on file-history snapshots, touches no transcript-on-disk.
- **`/resume`** — load a prior transcript and continue (CLI `--resume` / `-r`);
  reads JSONL, rebuilds chain via `coco_session::recovery`.

**Canonical names only.** Aliases (`checkpoint`, `continue`) are intentionally
dropped: single dispatch arm per command, no `matches!(...)` fan-out, no
entries in `RegisteredCommand.base.aliases`. Audits reintroducing an alias must
justify the divergence. The historical `/restore` / `--restore` names from an
earlier draft are likewise off the table.

## Permission/persistence gaps below the slash-command layer

NOT command-handler bugs, but they manifest as "the command doesn't seem to
do anything" in audits:

- `DialogSpec::PluginPicker` / `McpbConfig` / `Confirm`: registered, but the
  TUI slash dispatch (`app/cli/src/tui/slash_execution.rs`) emits
  `SlashCommandStatusKind::DialogPending` instead of opening a real overlay.
  Dialog data is plumbed; the TUI consumer is not. Track in
  `coco-tui::overlays`, not here.
- `/permissions allow|deny|reset`: mutates `engine_config` for the session
  only (`PermissionUpdateDestination::Session`); never writes settings.json.
  No fix needed.
