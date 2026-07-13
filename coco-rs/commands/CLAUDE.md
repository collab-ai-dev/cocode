# coco-commands

Slash command registry and built-in implementations (help, config, clear, compact, model, session, mcp, plugin, diff, commit, pr, review, doctor, ...). ~96 commands across v1/v2/v3.

## Key Types
- `CommandHandler` trait — `execute(args: &str) -> Result<String>`
- `RegisteredCommand` — metadata (`CommandBase` from coco-types) + optional handler + `is_enabled` feature-flag gate
- `CommandRegistry` — name-keyed map with alias lookup; filter views: `visible()`, `client_visible()` (strips `is_sensitive`), `safe_for(CommandSafety)`
- `BuiltinCommand` / `AsyncBuiltinCommand` — sync and async built-in handler wrappers
- `builtin_base()`, `builtin_base_ext()` — construct default `CommandBase` with safety + argument-hint options
- `register_builtins()` — registers the starter ~25; `register_extended_builtins` in `implementations::`

## Modules
- `handlers/` — richer command handlers that need app state
- `implementations/` — extended builtin registrations and shared `names` constants

## Deliberately Not Ported

**Audits should skip the commands listed below — these are conscious omissions,
not gaps.** If a future change re-introduces one of these, remove the
corresponding row from this table and add it to the registry.

### Group A — Provider / account-specific (Anthropic-only flows)

Skipped because the multi-provider scope means no single sign-in / billing /
account-management surface applies across providers.

| Command | Reason |
|---|---|
| `/bug` | Anthropic `/feedback` alias. coco-rs intentionally does not register it so prompts and docs point users at the explicit `/feedback` command and the `collab-ai-dev/cocode` issue tracker instead of upstream Anthropic routes. |
| `/fast` | Claude.ai/console-only fast-mode picker; coco-rs exposes fast-mode via `FastModeState` + Ctrl+Shift+F keybind only. |
| `/release-notes` | Fetches Anthropic-hosted changelog; not slash-invoked in coco-rs (CLI subcommand only). |
| `/privacy-settings` | `isConsumerSubscriber()`-gated; calls Anthropic Grove API. |
| `/rate-limit-options` | Claude.ai-only, hidden internal. |
| `/reset-limits` (+ non-interactive) | Upstream source is a literal `isEnabled: () => false` stub. |
| `/install-github-app` | `claude-ai`/`console` availability + Anthropic GitHub App OAuth. |
| `/install-slack-app` | `claude-ai` availability + Anthropic Slack App marketplace. |
| `/chrome` | `claude-ai` availability; Chrome-extension-only settings UI. |
| `/mobile` (aliases `/ios`, `/android`) | claude.ai mobile-app QR flow. |
| `/desktop` (alias `/app`) | `claude-ai` + macOS/win32 only; Anthropic desktop client install. |
| `/passes` | claude.ai referral / Passes program. |
| `/terminal-setup` | NOT actually provider-specific (corrected): upstream is a generic Shift+Enter newline keybinding installer for VS Code / Apple Terminal. Deferred as a low-priority generic port, not a provider/account concern — port if users ask for it. |
| `/extra-usage` (+ non-interactive) | Anthropic admin-overage request flow. |
| `/think-back` / `/thinkback-play` | Statsig-gated experimental Anthropic feature. |
| `/stickers` | Anthropic Code sticker-merch order flow (ungated/user-facing upstream, but claude.ai-account-only merch). |

### Group B — Anthropic-internal stubs / first-party-only

Skipped because the upstream source is already a literal stub placeholder, or
the feature depends on Anthropic-internal infrastructure (KAIROS, CCR, advisor
API beta) that coco-rs does not ship.

| Command | Reason |
|---|---|
| `/advisor` | Server-side Anthropic API beta `advisor-tool-2026-03-01`, first-party-only. |
| `/ultraplan` | `feature('ULTRAPLAN')`; depends on Claude-Code-on-Web ("CCR") session backend. |
| `/ultrareview` | CCR-backed multi-agent review with no local execution path. |
| `/bughunter` | Upstream source is a literal `isEnabled: () => false` stub. |
| `/autofix-pr` | Upstream source is a literal stub. |
| `/issue` | Upstream source is a literal stub. |
| `/onboarding` | Upstream source is a literal stub; in `INTERNAL_ONLY_COMMANDS`. |
| `/share` | Upstream source is a literal stub; in `INTERNAL_ONLY_COMMANDS`. |
| `/teleport` | Upstream source is a literal stub; in `INTERNAL_ONLY_COMMANDS`. |
| `/heapdump` | Node.js V8 heap snapshot; no Rust runtime equivalent. |
| `/ctx_viz` | Anthropic-internal context probe; in `INTERNAL_ONLY_COMMANDS`. |
| `/ant-trace` | Upstream source is a literal stub; original feature was an Anthropic-only OTel trace toggle. |
| `/brief` | KAIROS-only (`feature('KAIROS_BRIEF')`); depends on Anthropic-internal `BriefTool`. |
| `/backfill-sessions` | Literal `isEnabled:()=>false` stub in `INTERNAL_ONLY_COMMANDS`. |
| `/break-cache` | Literal `isEnabled:()=>false` stub in `INTERNAL_ONLY_COMMANDS` (prompt-cache debug). |
| `/mock-limits` | Literal `isEnabled:()=>false` stub in `INTERNAL_ONLY_COMMANDS` (rate-limit mocking). |
| `/good-claude` | Literal `isEnabled:()=>false` stub in `INTERNAL_ONLY_COMMANDS`. |
| `/perf-issue` | Literal `isEnabled:()=>false` stub in `INTERNAL_ONLY_COMMANDS`. |
| `/oauth-refresh` | Literal `isEnabled:()=>false` stub in `INTERNAL_ONLY_COMMANDS` (provider-internal). |
| `/bridge-kick` | Real but `USER_TYPE==='ant'`-gated bridge-failure-injection diagnostic; `INTERNAL_ONLY_COMMANDS`. |
| `/init-verifiers` | `type:'prompt'`, `INTERNAL_ONLY_COMMANDS` (ant-only); generates Verify-agent verifier skills. |

### Group C — Feature-gated upstream optionals (compiled out of the public build)

Skipped because upstream gates each behind a GrowthBook/`feature(...)` flag
that is **off** in the public bundle (the command module is dead-code-eliminated),
or behind claude.ai-only subscriber/policy checks. coco-rs ships no equivalent
backend, so the slash command is intentionally absent.

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

If a downstream consumer needs a skipped command, treat it as a feature add,
not a bug fix:
1. Remove the row from the table above.
2. Implement the command in `implementations.rs` or `handlers/`.
3. If the command depends on Anthropic-only infrastructure, hide it behind a
   `Feature` gate so non-Anthropic providers stay clean.

## Deferred (registered but thinned)

These commands ARE registered and respond, but the body is intentionally
simpler than the full feature pending follow-up work. Don't flag them as
missing — they are stubs by design — but DO update this table when the gap
closes.

| Command | Rust state | Gap |
|---|---|---|
| `/insights` | `register_static_prompt` with 12-line body in `prompts/insights.txt` | Full behavior: Opus-driven facet extraction + SCP-from-Coder for remote sessions + JSONL log parsing. Rust delegates the work to the agent via prompt. P3. |
| `/workflow` (alias `/workflows`) | Prompt command in `prompts/workflow.txt` with `allowed_tools=["Workflow"]` | Bare `/workflow` opens the workflow picker; `/workflow <name>` launches through the Workflow tool. Launched workflows run as `local_workflow` background tasks and the TUI background-task/detail surfaces render workflow progress notifications. Gap: no workflow editor/creation UI yet. P2. |
| `/ide` | Static text stub in `ide_handler` | Full behavior: `detectRunningIDEs`, JetBrains/VS Code auto-connect dialogs, MCP cache invalidation. Rust ships the `coco-bridge` crate but the slash command is not wired to it. P2 — wire when bridge UX is finalized. |
| `/help` | Hardcoded `CATEGORIES` in `handlers/help.rs` | User-installed skills, plugin contributions, and MCP-bridged tools won't appear in `/help` output. P1 — refactor to iterate the live `CommandRegistry`; needs handler-side registry access (currently `CommandHandler::execute_command(&self, args: &str)` doesn't carry one). |
| `/color` | `dispatch_color` writes only to live `app_state.agent_color` | Choice should persist in the session transcript so it survives restarts. Currently ephemeral. P3 — wire to settings.json or session metadata. |
| `/diff` | Plain `/diff` renders the uncommitted git diff; TUI intercepts `/diff session` and `/diff turn <message-id>` to render file-history snapshot diffs | SDK/headless still expose only the git-diff text handler. P3. |
| `/tasks` (alias `bashes`) | No-arg opens the background-tasks modal; `list`, `detail <id>`, and `cancel <id>` use the live `TaskRuntime` | Full interactive output scrolling remains TUI-side follow-up. P3. |
| `/mcp` | Async overlay for list/add/remove/enable/disable | Core ops work; interactive wizard UX (xaa IDP, add-server) thinned. P2. |
| `/hooks` | Async overlay shows hook configs; `/hooks reload` reloads the live registry | Read-oriented; interactive editing not yet available. P3. |
| `/sandbox` (file `sandbox-toggle`) | Sync handler writes canonical modes and supports `exclusions`, `exclude <pattern>`, `unexclude <pattern>` | Per-platform availability panel text is still thin. P3. |
| `/doctor` | Async health-check text report | Install-method + auto-updater status not applicable to coco's distribution. Text report is sufficient. P3. |
| `/status` | Sentinel → live `runtime.status_report()`; TUI opens a read-only status panel, SDK/headless keep text output | Panel jump actions to model/settings/permissions/sandbox surfaces are not implemented. P3. |

## Interactive-only commands (TUI; no SDK/headless path)

`/export`, `/branch` (alias `/fork`), and `/btw` do their real work in the TUI
runner (`app/cli/tui_runner.rs` dispatch interceptors), not the registry sync
handler. These are interactive-only (TUI) commands — they don't run meaningfully in headless `-p` mode. The registry
handlers (`branch_handler`, `export_handler`) return honest usage guidance for
the non-interactive surface; `/btw` additionally has an SDK `turn/start`
handler fork path (shared `coco_agent_host::side_question`). Behavior notes:

- `/export <filename>` writes the conversation (incl. tool activity) under cwd;
  format inferred from extension (`.md`/`.json`/else text). No-arg opens the
  format picker, which writes a timestamped default. Clipboard export is `/copy`
  (coco split; TS bundles clipboard into the export dialog).
- `/branch` forks the on-disk transcript (`recovery::fork_conversation`,
  relabeling `session_id`) + live-switches via the `/resume` hydration path.
- `/btw` answer is model-invisible but transcript-visible (TS modal is fully
  ephemeral) — see `handlers/btw.rs`.

## Always-Enabled General-Purpose Commands

These commands are plain Rust features with no gating in coco-rs. **Do not
introduce `is_enabled` for these** — they are intentionally available to
every user.

| Command | What it does in coco-rs |
|---|---|
| `/version` | Prints `cocode v{CARGO_PKG_VERSION}`. |
| `/feedback` | Generates a prefilled `collab-ai-dev/cocode` GitHub issue URL with version, commit, build time, OS, arch, and timestamp. Logs are excluded by default; `--with-logs` includes only a best-effort redacted tail of the current coco log and tells the user to review before submitting. No `/bug` alias. |
| `/tag` | Toggles a searchable tag on the current session via `SessionManager::toggle_tag` (sentinel-based dispatch). |
| `/files` | Lists `git ls-files` grouped by top-level directory with rough context-size estimate. (Description: "List git-tracked files in this repository".) |

## Rewind / Resume Naming

Two distinct features:

- **`/rewind`** — in-session TUI checkpoint picker (`openMessageSelector`
  semantics). Operates on file-history snapshots; touches no
  transcript-on-disk.
- **`/resume`** — load a prior transcript and continue. CLI form: `--resume`
  / `-r`. Reads JSONL; rebuilds chain via `coco_session::recovery`.

**Canonical names only.** Aliases (`/rewind` → `[checkpoint]`,
`/resume` → `[continue]`) are intentionally dropped. Single dispatch
arm per command — no `matches!(name, "rewind" | "checkpoint" | "undo")`
fan-out, no alias entries in `RegisteredCommand.base.aliases`. Audits that
reintroduce an alias must first justify why the divergence from this rule is
worth carrying. The historical `/restore` and `--restore` names from an
earlier coco-rs draft are likewise off the table.

## Permission/persistence gaps below the slash-command layer

These items are NOT command-handler bugs but show up in audits because
they manifest as "the command doesn't seem to do anything". They're
tracked here so audits can cross-reference.

- `DialogSpec::PluginPicker`, `DialogSpec::McpbConfig`, `DialogSpec::Confirm`:
  registered but `tui_runner::dispatch_slash_command` emits
  `SlashCommandStatusKind::DialogPending` instead of opening a real
  overlay. The dialog data is plumbed; the TUI consumer is not. Track
  in `coco-tui::overlays`, not here.
- `/permissions allow|deny|reset`: mutates `engine_config` for the
  session but does not write to settings.json. Behavior is session-only
  (`PermissionUpdateDestination::Session`). No fix needed.
