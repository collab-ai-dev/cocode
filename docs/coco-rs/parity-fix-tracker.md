# coco-rs TS→Rust Parity — Fix Tracker

> **Dedicated tracking file** for the remaining TS→Rust parity work. Tick `[ ]→[x]` as items land; update the progress counts below.
> Baseline established **2026-06-05** by re-validating the 267-finding audit against live **HEAD `748580242`** (`feat/review`) — live-code reads + a 58-agent verification pass. Each open item is self-contained: TS spec ref + current Rust location + fix sketch.
> No P0 work is open (every security/correctness/main-loop gap is already fixed — see **Closed**). Remaining work is medium/low parity polish, mostly *wiring ported-but-unwired code*.

## Progress

Update these as boxes get ticked.

| Bucket | Total | Done |
|---|---:|---:|
| **P1** (wiring; small) | 18 | 17 |
| **P2** (parity tail) | 78 | 75 |
| **P3** (cosmetic/edge) | 58 | 19 (+2 partial) |
| **Open total** | 155 | 111 |

> **2026-06-07 — P3 implementation pass (16 fixes landed + tested, `just pre-commit` green: 11041 tests pass).**
> After the two verification workflows, implemented the Tier-0 (real bugs) + Tier-1 (quick wins) +
> cheap Tier-2 (finish-the-partial) set. **Newly `[x]` (16):**
> - **Behavioral bugs (Tier 0):** tool-runtime#13 (`COCO_MAX_TOOL_USE_CONCURRENCY=0` deadlock →
>   `>0` filter), sandbox#175 (`allow_pty` default flipped to true, both serde + Default impl),
>   tool-runtime#15 (pre-execute abort now emits `PreExecutionCancelled` + `CANCEL_MESSAGE`, no
>   PostToolUseFailure), hooks#187 (failed-hook stderr no longer injected as success-context +
>   missing `hook_non_blocking_error` now emitted).
> - **MEDIUM:** tools-file#23 (empty/offset reads emit `<system-reminder>` warnings at render).
> - **Quick wins (Tier 1):** messages#88 (cost `>0.5` threshold), memory#226 (verbatim freshness
>   text + caller spacing), commands#211 (`/status` no alias, `/tasks`→`bashes`), plugins#237
>   (canonical `(+ N)` suffix, dead `format_dep_note` deleted), tools-web-mcp#63 (`required:[url,prompt]`),
>   system-reminder#108 (trailing-newline parity for todo+task), tools-exec#35 (no max-timeout
>   enforcement; dead `max_timeout_ms` helper deleted).
> - **Finished partials (Tier 2):** tools-exec#42 (pwsh hint-strip), tools-web-mcp#60 (teammate-durable
>   guard via `is_in_process_teammate`), mcp#156 (Unicode `…` marker + server-instructions truncated +
>   2 dead dup impls deleted), context#102 (`COCO_DISABLE_GIT_INSTRUCTIONS` EnvKey + `gitsettings`
>   helper + `get_environment_info` gate threaded through all callers).
>
> **system-reminder#104 → REFUTED** (removed from open set): the prior audit analyzed a DEAD path —
> `QueuedCommandGenerator` is not registered; the live drain (`queued_command_to_attachment`) renders
> human commands correctly (visibility is `AttachmentKind`-derived, not per-message `is_meta`). No bug.
>
> **Still deferred (2 — largest, lowest-impact, most cascade risk):**
> - **tool-runtime#11** `[~]` — phrasing already matches TS; remaining = `<tool_use_error>` wrapper +
>   `Bash(cmd)` descriptor. Needs `ToolCallErrorKind::SiblingCancelled` + an `errored_tool_descriptor`
>   field threaded onto `UnstampedToolCallOutcome` (≈6 construction sites) + delete dead
>   `SyntheticToolError::SiblingError`. Spec validated (workflow `waksbnfdu`).
> - **tools-exec#40** `[ ]` — auto-detach mislabeled `backgroundedByUser`. Needs a `DetachSource`
>   enum threaded through `signal_detach` (signature change) across running.rs/controller.rs/agent.rs/
>   the `TaskHandle` trait + bash.rs. Note: `assistantAutoBackgrounded` is KAIROS-only — NOT a coco
>   target; the fix is the generic message, not that flag.
>
> The broader **Tier-3 parity tail** (hooks#186/#188, system-reminder#106/#107, coordinator#259/261/262,
> config#245/249, messages#85/86/87, mcp#149/150, query#5/6/7, sandbox#174/176, plugins#236/238,
> tools-exec#37/39, context#99, tasks#215, shell#161/165, skills#199, tools-file#27, tools-web-mcp#58/61)
> remains open — medium-effort, batched-by-subsystem follow-ups.

> **2026-06-07 — P3 verification pass (18-agent workflow `w2c61u25v`, 586 tool-uses, live HEAD `f79861273f` + `claude-code-kim` TS mirror).**
> The "P3 done = 0" line was stale. Re-verified every one of the 58 by re-locating
> symbols live (the audit's line numbers are off the old `748580242` baseline):
> - **3 fully fixed → `[x]`:** **tools-web-mcp#59** (CronList recurring/durable — closed by the recent
>   cron commits `c258d88..f798612`), **config#252** (no `get_fast_mode_model`; fast mode is a
>   same-model `speed=fast` capability flag by design — already_fixed-by-redesign), **tasks#218**
>   (premise refuted — Rust `truncate_output` is head-only + 8MB tail-read = TS parity; the middle-elide
>   was removed with #34).
> - **7 partials → `[~]`** (one half already landed): **tools-exec#42** (bash hint-strip done, pwsh
>   missing), **tools-exec#43** (prod TaskRuntime path already merges stdout+stderr; only byte-exact
>   interleave + legacy fallback residual — candidate `wont_fix`), **tools-web-mcp#60** (next-run
>   reachability done; teammate-durable guard missing), **config#250** (file managed-settings done;
>   OS-MDM missing [large], remote-sync is a non-goal), **mcp#156** (truncation is char-safe + wired;
>   suffix char + untruncated server-instructions + 2 dead dup impls remain), **tool-runtime#11**
>   (phrasing already matches TS; `<tool_use_error>` wrapper + `(cmd)` descriptor missing — and #8 is
>   actually WIRED, so this is no longer "dead code"), **context#102** (2k truncation done;
>   `include_git_instructions` gate is a no-op with zero consumers).
> - **48 confirmed open.** Several LOW-labelled items are genuine behavioral bugs, not cosmetics
>   (see the prioritized fix plan handed back this session): **tool-runtime#13** (concurrency=0
>   deadlock), **sandbox#175** (allow_pty default silently denies PTY), **tool-runtime#15** (pre-exec
>   cancel wrongly fires PostToolUseFailure), **system-reminder#104** (mid-turn human input vanishes
>   from transcript), **hooks#187** (non-zero hook stderr injected as success context), **tools-exec#40**
>   (auto-detach mislabeled "backgrounded by user").
> - **Sanctioned reframes applied:** every `CLAUDE_CODE_*` env finding (config#245, system-reminder#107,
>   context#102, skills#199, tools-exec#37) is retargeted to its `COCO_*` equivalent, never the
>   CLAUDE_ name. **tools-file#23 is the only MEDIUM** in P3.

> **2026-06-07 — adversarial re-audit of the 12 'remaining' P2 items (12-agent workflow `w53jmyhgi` + independent greps on live HEAD).**
> The tracker **undercounted done**. **6 of the 12 were stale-mislabeled** as open/partial and are
> actually fully wired — verified by tracing live call chains, not trusting the (stale) RUST POINTER
> line numbers: **tool-runtime#9, query#2, permissions#70, hooks#184, hooks#185, commands#208** → all
> flipped `[x]` below with evidence. **P2 done 57 → 72.** Only **4 genuine gaps remain** (+2 large defers).
>
> The two plugin items previously stamped **"effectively complete" were NOT** — both are unreachable on
> the headless + SDK surfaces:
> - **plugins#235** — `run_marketplace_startup` has exactly ONE caller (`tui_runner.rs:454`); the
>   seed→reconcile→**delist** sweep silently never runs for `coco --print`, piped, `coco chat`,
>   `coco review`, or SDK NDJSON (TS runs it headless: `print.ts:1721`). Genuine **do-now** gap.
> - **plugins#239** — SDK `handle_plugin_reload` (`sdk_server/handlers/runtime.rs:323`) is a `_ctx`
>   no-op stub returning empty vecs; an SDK client's `plugin/reload` reloads nothing. Genuine **do-now** gap.
>
> **UPDATE 2026-06-07 (same day): the two do-now items are now FIXED** — plugins#235 (marketplace startup
> wired into headless + SDK via `session_bootstrap::spawn_marketplace_startup`) and plugins#239 (SDK
> `handle_plugin_reload` now runs the real reload chain). `just quick-check` + `just test-crate coco-cli`
> green; uncommitted. **P2 done 72 → 74.** Remaining: tool-runtime#8 (own PR), config#247 (capability
> gating), tools-web-mcp#54 + plugins#234 (large defers).
>
> **Original remaining-work plan (now partly done):** do-now = ~~plugins#235 + plugins#239~~ (FIXED above).
> Defer-own-PR = **tool-runtime#8** (sibling-abort missing on the **default streaming** path — see
> its entry for the defer rationale; it is sequenced after the plugin wiring, not dropped). Partial =
> **config#247** (now also a **multi-provider correctness bug** — the model-support gate hardcodes
> `contains("opus-4-6")` instead of the capability-driven, provider-agnostic
> `capabilities.contains(Capability::FastMode)`; see its entry).
> Large defer = tools-web-mcp#54, plugins#234.
>
> **2026-06-06 — P2 verification + tracker reconciliation on `feat/review`.**
> The P2 work landed across **Wave 1–5** commits (`a8110cb`, `4b3da2f`,
> `46472336`, `adafd60` + the earlier first-pass), but the checkboxes below were
> never flipped — this pass reconciles the doc to the code and **adversarially
> verifies** each wave fix against the `claude-code-kim` TS mirror (5-agent
> sweep). `just quick-check` GREEN at HEAD.
>
> **P2 done is now 57** (was stated 38; the doc lagged the waves by 19).
>
> **Wave fixes reconciled to `[x]` (25):** Wave 1 — tool-runtime#10/12,
> permissions#75/78, hooks#190, skills#196, plugins#240, commands#209,
> coordinator#265, context#101, mcp#151. Wave 2 — skills#193, coordinator#258,
> config#253, inference#133/137, context#96/97, sandbox#177, messages#80. Wave 3
> — shell#162/164/167. Wave 5 — context#98, commands#207. (These join the ~32
> first-pass `[x]` items already recorded.)
>
> **Adversarial verification verdicts:** 18 CONFIRMED faithful (inference#133;
> context#96/97/98; permissions#75/78; tool-runtime#10/12; shell#162/164/167;
> skills#193/196; coordinator#258/265; config#253; commands#209; mcp#151).
> Two findings:
> - **inference#137** was PARTIAL (interruptible sleep present, but no
>   top-of-attempt cancel check) → **now COMPLETE**: added the top-of-loop
>   `cancel.is_cancelled()` guard to both the blocking and streaming retry loops
>   (TS `withRetry.ts:190`) + companion test
>   (`test_precancelled_token_short_circuits_before_request`).
> - **context#101** flagged PARTIAL by an agent (processed-vs-raw stored
>   content) was a **false alarm**: `detect_changed_files` decides changed-ness
>   by **mtime** (`changed_files.rs:32`), never by content diff, and the stored
>   `mtime_ms` is the real disk mtime. No fix needed — CONFIRMED.
>
> **Follow-up flagged (not a regression):** the Wave-3 Bash `check_permissions`
> curated routing admits only 3 risk ids (jq-system, dangerous-vars, IFS);
> broad code-exec/exfil patterns collapse to `DANGEROUS_PATTERNS_GENERAL` and
> pass at *that* seam (downstream rule pipeline + auto-mode classifier still
> gate them). Matches the deliberate Wave-3 over-prompt-avoidance design; a
> git-commit-scoped + safe-substitution carve-out is the eventual completion.
>
> **Partial:** tools-web-mcp#52 and inference#134 are now complete.
> Current partials are config#247, tools-web-mcp#54, plus the three Stage-4
> items below (plugins#234/#235/#239).
>
> **Stage 4 — plugin lifecycle progress (2026-06-06).** Implemented the chosen
> "named target" slice (the parts not blocked on threading `McpConnectionManager`
> into `SessionRuntime`). plugins#234/#235/#239 moved `[ ]`→`[~]`:
> - **#234** — full `validateUserConfig` type/range port + `${user_config.X}`/
>   `${__dirname}` substitution (4 tests). Remaining: sensitive→keyring (needs a
>   live install caller; module still caller-less).
> - **#235** — the **delisting sweep** is live + wired at startup + tested,
>   driving all 4 formerly-dead delisting fns. Remaining: seed marketplaces
>   (env-name decision) + reconcile-on-startup + headless-specific entry.
> - **#239** — `/reload-plugins` now chains agent-catalog + hook reloads.
>   Remaining: MCP/LSP re-register + reconnectKey (the `McpConnectionManager`
>   blocker). Also: builtin-plugin scaffold (`init_builtin_plugins` + skill merge)
>   wired at bootstrap/reload (clears the "builtins-dormant" follow-up).
>
> **Genuinely open — grouped for the fix plan.** ⚠️ **SUPERSEDED by the 2026-06-07 audit above** —
> most of the items below were verified RESOLVED (tool-runtime#9, query#2, permissions#70, commands#208,
> hooks#184, hooks#185). Kept for history; the live remaining set is in the 2026-06-07 note.
> - ~~**Wave 4 — interrupt/abort fabric:**~~ only **tool-runtime#8** (streaming sibling-abort) remains;
>   tool-runtime#9 / query#2 / permissions#70 are RESOLVED (were never actually open).
> - ~~**Interactive TUI overlays:** commands#208~~ — RESOLVED (real `PluginDialogState` overlay).
> - ~~**Hooks async fabric:** hooks#184 / hooks#185~~ — RESOLVED (only hooks#185 `forceSyncExecution`
>   residual remains).
> - **Partial misc (still open):** config#247 (fast-mode — now also a multi-provider gating bug),
>   tools-web-mcp#54 (real cron wake loop).
>
> Run `just pre-commit` before committing.

> **2026-06-06 — misc-deferred implementation pass (Wave 4 still deferred).**
> Completed in this pass: tools-web-mcp#52, inference#134, config#243,
> mcp#144, skills#194, messages#81, and system-reminder#103/#109/#110.
> Partial / bounded fixes: config#247 (fast-mode model gate + live toggle,
> org prefetch state machine still not fully ported) and tools-web-mcp#54
> (in-memory schedule CRUD backend; no real cron wake loop). Still open outside
> Wave 4: commands#208 and hooks#184/#185.

> **2026-06-05 — all 18 P1 items resolved on `feat/review`** (17 fixed + tested;
> `skills#201` deliberately skipped — coco keeps the `.coco/` convention, not
> `.claude/`). `just pre-commit` green; full-workspace clippy clean. Two items
> landed **partial** with documented follow-ups: `skills#195` (legacy
> `.coco/commands` dir + realpath dedup done; setting-source gates + `--add-dir`
> deferred — needs `build_session_skill_manager` config plumbing) and
> `skills#192` (dual `!`-pattern parsing + MCP-skip done; per-command
> permission-evaluator wiring deferred). Implementation notes per item below.

## P1 — finish the half-wired subsystems (small effort, mostly wiring)

### [x] mcp · short_request_id ✅ FIXED (feat/review)
`● genuinely_open` · **HIGH** · effort **small**

- **Gap:** RE-AUDIT MISS. `services/mcp/src/naming.rs` still returns `tool_use_id[..8]` (plain truncation). The FNV-1a→base-25 letters-only (excl `l`) 5-letter algorithm was never ported.
- **TS:** channelPermissions.ts:112-152 — FNV-1a (offset 0x811c9dc5, prime 0x01000193) → 25-letter alphabet → exactly 5 letters; PERMISSION_REPLY_RE `/^\s*(y|yes|n|no)\s+([a-km-z]{5})\s*$/i`.
- **Fix:** Implement FNV-1a base-25 5-letter encoder (+profanity re-hash) in `naming.rs::short_request_id`; channel reply parser must accept the 5-letter id.

### [x] shell · security validators ✅ FIXED (feat/review)
`● genuinely_open` · **HIGH** · effort **medium**

- **Gap:** RE-AUDIT MISS. `exec/shell/src/security.rs::check_security` registers only 5 crude substring checks; the 29-analyzer quote-aware suite (`utils/shell-parser` `default_analyzers`) has zero production consumers. (Genuinely-dangerous IFS/eval ARE blocked — this is breadth.)
- **TS:** tools/BashTool/bashSecurity.ts — ~22 quote/heredoc-aware validators (obfuscated `$'\x2d\x2d'` flags, backslash-escaped operators, brace expansion, comment/quote desync, CR tokenization).
- **Fix:** Wire `coco_shell_parser::default_analyzers()`/`analyze()` into the bash security gate (or port the missing validators into `check_security`).

### [x] plugins · hint-recommendation ✅ FIXED (feat/review — full pipeline + TUI dialog)
`● genuinely_open` · **HIGH** · effort **medium**

- **Gap:** RE-AUDIT MISS (niche UX). `PluginRecommendation` is a bare struct with no logic; `<claude-code-hint type=plugin/>` from CLI/SDK stderr is never surfaced, no show-once / dont-show-again state.
- **TS:** utils/plugins/hintRecommendation.ts — parse `<claude-code-hint>` from stderr → "install this plugin?" with show-once state.
- **Fix:** Add stderr hint parser + record/resolve/mark-shown/disable around `PluginRecommendation`, surfaced via the permission/notification path.

### [x] coordinator#256 — cleanup_team_directories skips worktree destruction and tasks-dir cleanup ✅ FIXED (feat/review)
`● genuinely_open` · **HIGH** · effort **small** · fix-sketch *sound*

- **Gap:** cleanup_team_directories reads worktrees and removes tasks dir but does not notify TaskListStore change listeners, unlike TS notifyTasksUpdated()
- **TS:** teamHelpers.ts:641-683 cleanupTeamDirectories reads the team file FIRST, collects each member.worktreePath, runs destroyWorktree (git worktree remove --force, fallback rm -rf) on each, THEN rm's the team dir, THEN rm's getTasksDir(sanitizedName) and calls notifyTasksUpdated().
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/coordinator/src/team_file.rs:150-196 (cleanup_team_directories reads team file and members.worktree_path at 153-165, calls destroy_worktree at 156, removes tasks dir at 179-193, but returns Ok(()) at 195 without notifying); TS source /lyz/codespace/3rd/claude-code/src/utils/swarm/teamHelpers.ts:641-683 calls notifyTasksUpdated()…
- **Fix:** In cleanup_team_directories: read_team_file first, for each member with Some(worktree_path) call destroy_worktree, then remove the team dir, then remove the team's tasks dir (sanitize_name(team)) and fire a tasks-updated notification.

### [x] mcp#145 — MCP resources and prompts never fetched at connect (always empty) ✅ FIXED (feat/review — SDK path now fetches; command registration still Phase-2)
`◑ partially_fixed` · **HIGH** · effort **small** · fix-sketch *sound_with_caveats*

- **Gap:** rmcp path partially fixed (resources/prompts fetched), SDK path still broken (hardcoded empty), prompts never registered as mcp__server__prompt commands
- **TS:** client.ts:2000 fetchResourcesForClient sends resources/list and :2033 fetchCommandsForClient sends prompts/list at discovery (gated on capabilities.resources/.prompts), converting prompts to mcp__<server>__<prompt> slash Commands (client.ts:2054-2073).
- **Rust (HEAD):** coco-rs/services/mcp/src/client.rs:343-351 (rmcp path calls fetch_resources/fetch_prompts), 475-482 (SDK path still hardcodes Vec::new()), discovery.rs:197-207 (reads empty server.resources if not populated), naming.rs:37-38 (mcp__<server>__<tool> convention documented but prompts never registered)
- **Fix:** In do_connect/do_connect_sdk, when caps.resources/.prompts, call list_resources / a new list_prompts and populate ConnectedMcpServer.resources/commands; register prompts into the command registry as CommandSource::Mcp prompt commands.

### [x] plugins#231 — Enterprise plugin policy never read from managed settings; no per-plugin blockl… ✅ FIXED (feat/review — enable_plugin policy gate)
`◑ partially_fixed` · **HIGH** · effort **small** · fix-sketch *sound_with_caveats*

- **Gap:** Per-plugin blocklist implemented for install pipeline; enable/disable handlers lack policy gate
- **TS:** pluginPolicy.ts isPluginBlockedByPolicy(pluginId) reads getSettingsForSource('policySettings').enabledPlugins[pluginId] === false — a per-pluginId gate that is the single source of truth across install/enable/UI.
- **Rust (HEAD):** plugins/src/security.rs:182-289 (EnterprisePolicy struct with blocked_plugins field, from_managed_settings/from_policy_settings constructors, check_policy function with PolicyVerdict::BlockedPlugin); install.rs:182,220 (policy gates at root and dependency level); commands/src/handlers/plugin.rs:280-300 (enable_plugin function lacks policy check); security.t…
- **Fix:** Add a per-plugin blocklist field to EnterprisePolicy and a constructor that reads it (plus marketplace fields) from the Policy-scope settings via RuntimeConfig; wire both install sites to build the policy from managed settings instead of ::default().

### [x] skills#195 — App loads only 2 flat skill dirs - managed/legacy-commands/project-up-to-home/d… ✅ FIXED (feat/review — legacy `.coco/commands` + realpath dedup + git-root boundary + full `--setting-sources`/plugin-only gates + `--add-dir`)
`◑ partially_fixed` · **HIGH** · effort **small** · fix-sketch *sound_with_caveats*

- **Gap:** Partially fixed: load_scoped/dedup/managed/project loaded, but legacy .claude/commands skipped, setting-source gates unimplemented, --add-dir not wired.
- **TS:** loadSkillsDir.ts:638-803 getSkillDirCommands walks managed (getManagedFilePath/.claude/skills) -> user -> project (getProjectDirsUpToHome) -> --add-dir -> legacy loadSkillsFromCommandsDir, then dedups by realpath file identity (728-769) and applies isSettingSourceEnabled/isRestrictedToPluginOnly gates (650-713).
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/skills/src/lib.rs:1208-1225 (build_session_skill_manager now uses load_scoped with managed/user/project scopes); skills/src/lib.rs:609-625 (load_scoped implements three-scope loading); skills/src/lib.rs:857-862 (dedup by canonical realpath implemented); session_runtime.rs:2938 (reload_plugins_with calls build_session_skill_manag…
- **Fix:** Replace the two flat load_from_dirs calls with load_scoped over a SkillScopes built from managed + ~/.coco/skills + project-dirs-up-to-home + commands dirs, with realpath dedup and source-enable/lock gates (reuse the existing handlers/skills.rs build pattern).

### [x] tools-agent-task#50 — EnterWorktree does not chdir into the worktree or restore session state ✅ FIXED (feat/review — ExitWorktreeOutput parity fields; cache-clear is a no-op in coco-rs, see note)
`● genuinely_open` · **HIGH** · effort **small** · fix-sketch *sound_with_caveats*

- **Gap:** EnterWorktree changes process CWD but omits TS's system-prompt and memory-file cache clearing; no query-engine consumer wires cwd_override from tool output, so subsequent tools see stale context despite correct session_cwd.
- **TS:** EnterWorktreeTool.ts:94-102 — after creating the worktree it process.chdir(worktreePath) + setCwd + setOriginalCwd(getCwd()) + saveWorktreeState + clearSystemPromptSections + clearMemoryFileCaches + clears the plans-dir cache, so all subsequent file/shell ops operate inside the worktree.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/worktree.rs:87-198 (EnterWorktreeTool::execute does std::env::set_current_dir + session_cwd update at lines 161-179 but omits clearSystemPromptSections/clearMemoryFileCaches/getPlansDirectory.cache.clear); no consumer in app/query to set cwd_override from EnterWorktreeOutput. ExitWorktreeTool (line 352) call…
- **Fix:** After creating the worktree, restore the session into it: at minimum std::env::set_current_dir(worktree_path), and surface originalCwd / cache-clear targets to the query-engine cleanup hook (mirroring the ExitWorktreeRestoration block) so system-prompt/memory caches and originalCwd are updated.

### [x] commands#204 — /help is a hardcoded category list advertising nonexistent commands and wrong a… ✅ FIXED (feat/review)
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Help.rs advertises 5 phantom commands (fast, privacy-settings, feedback+bug, pr+pr-create, resume+continue) that are NOT in the live registry and will throw NotFound
- **TS:** commands/help/help.tsx passes the live commands registry to <HelpV2 commands={commands}/> (dynamic, source-annotated, including skills/plugins/MCP commands).
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/commands/src/handlers/help.rs:127 (fast), :139 (privacy-settings), :316 (feedback/bug), :228 (pr/pr-create), :168 (resume/continue). These entries are hardcoded in CATEGORIES but NOT registered in implementations.rs (grep of implementations.rs finds no FAST, PRIVACY, FEEDBACK, PR_CREATE, or CONTINUE constants).
- **Fix:** Minimal: delete the unregistered entries (fast, privacy-settings, feedback/bug) and the bogus aliases (continue, pr-create, pr) from CATEGORIES. Proper: iterate the live CommandRegistry snapshot (needs handler-side registry access) so skills/plugins/MCP also appear.

### [x] commands#206 — /clear missing reset/new aliases; /config alias is 'configuration' not TS 'sett… ✅ FIXED (feat/review)
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Missing TS aliases reset,new for /clear; wrong alias 'configuration' instead of 'settings' for /config; muscle-memory commands fall through
- **TS:** commands/clear/index.ts:14 aliases ['reset','new']; commands/config/index.ts:4 aliases ['settings'].
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/commands/src/implementations.rs:527-534 /clear uses &[] (no aliases; TS has reset,new); :268-275 /config uses &["configuration"] (TS has settings); tui_runner.rs:2538 intercepts only exact name=="clear", not aliases
- **Fix:** Register /clear with &["reset","new"] and /config with &["settings"] (dropping 'configuration'); ensure tui_runner's clear interception matches on the aliases too (matches!(name, "clear"|"reset"|"new")).

### [x] context#93 — Per-turn currentDate context ('Today's date is X') is not injected ✅ FIXED (feat/review — UserContextGenerator, Core tier; forks excluded for cache parity)
`● genuinely_open` · **MEDI** · effort **trivial** · fix-sketch *sound*

- **Gap:** No per-turn baseline currentDate injection (Today's date is X); only mid-session rollover emits a date string. KAIROS instruction broken by missing context.
- **TS:** context.ts:155-189 getUserContext always returns currentDate = `Today's date is ${getLocalISODate()}.`; utils/api.ts:449-474 prependUserContext injects it (plus claudeMd) as an isMeta <system-reminder> user message on every turn (except NODE_ENV==='test'). TS env block (prompts.ts:640-648 computeEnvInfo) intentionally has no date — the date comes only from …
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/system-reminder/src/generators/date_change.rs:50-65 emits date only on rollover; /lyz/codespace/codex/coco-rs/memory/src/prompt/builders.rs:218 instruction references never-injected currentDate; no per-turn prependUserContext equivalent in engine_prompt.rs
- **Fix:** Add a baseline currentDate injection on every turn (a UserPrompt/Core tier reminder or a per-turn meta user message) carrying `Today's date is <local ISO date>.`, independent of the rollover latch — mirroring prependUserContext.

### [x] memory#224 — SessionMemory first (init) extraction skips the tool-call/natural-break gate TS… ✅ FIXED (feat/review)
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Rust init-branch skips tool-call/natural-break gate that TS applies to every extraction (first or subsequent); Rust tests only cover tool_calls=5 case where gate would pass anyway
- **TS:** sessionMemory.ts:134-181 shouldExtractMemory: after the init-threshold check + markSessionMemoryInitialized() (138-143), it ALWAYS evaluates `hasMetTokenThreshold && hasMetToolCallThreshold) || (hasMetTokenThreshold && !hasToolCallsInLastTurn)` (168-170). On the first extraction hasMetUpdateThreshold is trivially true (tokensAtLastExtraction=0, so current>=…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/memory/src/service/session.rs:356-371 — the `if !state.initialized` branch (356-359) checks ONLY `current_tokens < session_memory_init_tokens` then falls through to try_claim+run_fork (lines 374-375). The tool-call gate (`tool_calls_since_last_extraction >= session_memory_tool_calls`) and natural-break (`!had_tool_calls_in_last_…
- **Fix:** In the `!state.initialized` branch, after passing the init-token check, also apply the tool_call_gate / natural_break disjunction (same as the else branch) before claiming, so the first extraction respects the natural-break gate.

### [x] shell#163 — Risky patterns hard-DENY in Rust where TS routes them through 'ask' approval ✅ FIXED (feat/review — eval/IFS/source→Ask; control-char + /proc/environ stay Deny)
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound*

- **Gap:** Hard-Deny IFS/eval/dangerous patterns in Rust have no user-approval path; TS routes same patterns through ask behavior for user control
- **TS:** bashSecurity.ts:2257-2413 — every validator returns behavior:'ask' or 'passthrough'; bashCommandIsSafe_DEPRECATED never returns 'deny'. eval, IFS=, backtick substitution etc. are 'ask' results the user can approve through the normal permission flow.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/exec/shell/src/security.rs:64-70 returns SecuritySeverity::Deny for eval; /lyz/codespace/codex/coco-rs/core/tools/src/tools/bash.rs:670-683 rejects all Deny results with no Ask route; /lyz/codespace/3rd/claude-code/src/tools/BashTool/bashSecurity.ts:2257-2413 never returns deny, only ask/passthrough
- **Fix:** Demote the Deny severities to Ask (route through the permission Ask flow) so the user can approve, matching TS — or at minimum allow override under bypass/yolo.

### [x] skills#192 — In-prompt shell execution uses wrong syntax and skips permission gating ✅ FIXED (feat/review — dual `!`-pattern + MCP-skip + per-command permission via the real Bash tool, abort-on-deny/failure)
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound_with_caveats*

- **Gap:** Shell expansion in skill prompts uses wrong syntax ($(...) vs TS !`...` and ```!...```) and has no permission gating; dead code exists but is never wired to disk/plugin skills
- **TS:** utils/promptShellExecution.ts:48-141: BLOCK_PATTERN=```!\n...``` and INLINE_PATTERN=!`...`. Each matched command is checked via hasPermissionsToUseTool against BashTool/PowerShellTool (throws MalformedCommandError on denial), executed through the real tool .call(), persisted via processToolResultBlock, and stdout+stderr formatted. Called from loadSkillsDir.…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/skills/src/shell_exec.rs:24-110 (execute_shell_in_prompt handles only $(...) syntax); /lyz/codespace/codex/coco-rs/skills/src/prompt_render.rs:68 (calls shell_exec with allow_shell gate, no permission evaluator); /lyz/codespace/codex/coco-rs/skills/src/lib.rs:1-50 (module public interface exposes shell_exec but no permission int…
- **Fix:** Wire render_skill_prompt (or fold shell-exec into expand_skill_prompt) into both live paths; replace `$(...)` scanning with TS INLINE/BLOCK patterns; route each command through the permission evaluator + Bash tool, skip when source is Mcp.

### [x] skills#198 — No resetSentSkillNames analogue - reloaded skills with same names are never re-… ✅ FIXED (feat/review — reset_announcements + watcher/clear wiring)
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** SkillManager has no reset_announcements method; /clear command wipes transcript but not announcements, so model never re-receives skill_listing after clearing conversation
- **TS:** skillChangeDetector.ts:276 calls resetSentSkillNames() on every debounced reload; attachments.ts:2607-2613 resetSentSkillNames clears the per-agent sentSkillNames set so the regenerated catalog (including same-named edited skills) is re-announced.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/skills/src/lib.rs:345-364 (take_unannounced_skills only inserts, never clears); lib.rs:416-424 (reload_disk_skills clears disk/disk_conditional but NOT announcements); /lyz/codespace/codex/coco-rs/app/cli/src/session_runtime.rs:3035-3127 (clear_conversation resets 12+ caches but never touches skill_manager.announcements)
- **Fix:** Add SkillManager::reset_announcements() clearing the announcements map; call it from reload_disk_skills (or from the reload driver) so the regenerated listing re-announces same-named skills.

### [x] skills#200 — Manual /reload-plugins never updates the model-facing SkillManager - only the /… ✅ FIXED (feat/review — reload_plugins_with folds into live skill_manager)
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound_with_caveats*

- **Gap:** Manual /reload-plugins creates ephemeral SkillManager for slash-command rebuild only; wired SkillManager (for model catalog and dispatch) never gets reloaded
- **TS:** skillChangeDetector.ts:85-141 + clearSkillCaches re-emit the live catalog so the model's skill_listing sees updated skills after a reload.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/app/cli/src/session_runtime.rs:2671-2698 (reload_plugins_with creates throwaway SkillManager at line 2676, uses it only for command_registry, drops it; self.skill_manager never reloaded); commands/src/lib.rs:517 (command dispatch wired to self.skill_manager); session_runtime.rs:1985 (model catalog wired to self.skill_manager)
- **Fix:** In reload_plugins_with, call self.skill_manager.reload_disk_skills(fresh) (and reset_announcements) instead of discarding a throwaway manager, then rebuild the command registry against self.skill_manager.

### [~] skills#201 — Bootstrap loads .coco/skills but not .claude/skills for the model - TS-compat p… ⏭️ SKIPPED (feat/review — deliberate: coco keeps the `.coco/` convention, and an explicit test asserts `.claude/skills` is ignored. WON'T FIX unless coco adopts `.claude/` compat.)
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Bootstrap loads ~/.coco/skills and .coco/skills upward but not .claude/skills; TS loads .claude/skills via getProjectDirsUpToHome; /skills dialog loads both .claude and .coco but model never sees .claude skills
- **TS:** loadSkillsDir.ts:642,692-698 getSkillDirCommands loads project skills from getProjectDirsUpToHome('skills', cwd), i.e. <cwd>/.claude/skills (and ancestors).
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/skills/src/lib.rs:1208-1225 (build_session_skill_manager loads managed + user_skills + project_skill_dirs_up_to_home); lib.rs:1241-1259 (project_skill_dirs_up_to_home walks only .coco/skills, NOT .claude/skills)
- **Fix:** Add cwd/.claude/skills (via SkillScopes.project_skills / load_scoped) to the engine-bootstrap skill load so the model catalog matches what /skills lists.

### [x] memory#225 — SessionMemory 'initialized' latch flips on successful fork, not on threshold cr… ✅ FIXED (feat/review)
`● genuinely_open` · **LOW** · effort **trivial** · fix-sketch *sound*

- **Gap:** Rust flips initialized flag only on fork success; TS flips it at threshold crossing before fork, independent of fork outcome
- **TS:** sessionMemory.ts:138-143 -- markSessionMemoryInitialized() (sessionMemoryUtils.ts:165-167) is called inside shouldExtractMemory the moment hasMetInitializationThreshold passes, BEFORE and independent of whether the fork fires or succeeds. So after a failed/skipped first extraction, TS is already initialized and uses the UPDATE branch (token-growth + tool-ca…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/memory/src/service/session.rs:716 — `state.initialized = true` is written ONLY inside `run_fork`'s `Ok(resp)` success branch (line 716); the `Err(e)` arm (735-743) returns Failed without setting it. grep confirms `initialized` writes are exactly: default(false), line 307 (clear_after_compact → false), line 716 (→ true). TS verif…
- **Fix:** Set state.initialized = true at the gate-pass point (right after the init-threshold check passes, before run_fork), independent of fork success -- mirroring markSessionMemoryInitialized() in the TS gate.

### P1 follow-ups (opened by the feat/review pass) — ✅ all resolved

These fell out of the two **partial** items and the adversarial review and have
since been implemented on `feat/review`:

- **[x] skills#195-b — full `--setting-sources` enforcement + `--add-dir` + plugin-only gates.**
  `StrictPluginOnlyCustomization` bool→enum (`true|array<surface>`),
  `RuntimeConfig.enabled_setting_sources` resolved from `--setting-sources`
  (and applied in `load_settings_with`), `SkillLoadGates` threaded into
  `build_session_skill_manager`, `COCO_DISABLE_POLICY_SKILLS`, `--add-dir`
  loading. Mirrors TS `getSkillDirCommands` gating.
- **[x] skills#192-b — per-command permission via the real Bash tool.** In-prompt
  shell routes through a `BashToolHandle` (`SessionBashToolHandle`) that runs
  `hasPermissionsToUseTool`-equivalent (`evaluate_with_tool_check` +
  `BashTool::check_permissions`) with the skill `allowed-tools` as always-allow,
  then `BashTool::execute`; deny **or** failure aborts the expansion (TS
  `MalformedCommandError`). MCP-skip retained.
- **[x] worktree base-branch — mirror TS.** `EnterWorktree` now resolves the
  default branch (`coco_git::get_default_branch`: `origin/HEAD` symref →
  main/master → fetch → `HEAD` fallback) and creates with `-B <branch> <path>
  <baseBranch>`, capturing the base SHA as `original_head_commit` so
  `discardedCommits` is measured against the default-branch baseline (TS
  `getOrCreateWorktree`). PR-mode + sparse-checkout remain out of the
  model-tool path (not exposed by TS `EnterWorktreeTool` either).

## P2 — parity tail (no safety/correctness risk; small/medium)

### tools-file — input-validation parity (9)

### [x] tools-file#17 — Read tool has no 25K-token output cap (MaxFileReadTokenExceededError)
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Read tool has 256KB byte cap but no 25K-token output cap; TS throws MaxFileReadTokenExceededError
- **TS:** FileReadTool.ts:1030 calls `validateContentTokens(content, ext, maxTokens)`; :755-772 estimates tokens (rough estimate then API count) and throws MaxFileReadTokenExceededError when content exceeds maxTokens (DEFAULT_MAX_OUTPUT_TOKENS=25000, env-overridable). This is the hardcoded base behavior, not GrowthBook-gated.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/read.rs:235-250 (validate_input checks only file_path/offset/limit, zero token validation); :545-575 (read path applies byte cap only, no token cap); grep for MaxFileReadTokenExceededError returns zero matches across entire crate
- **Fix:** After building `output`, estimate tokens (e.g. chars/4 heuristic, ext-aware) and if it exceeds a configurable maxTokens (default 25000), return a corrective InvalidInput/ExecutionFailed error instructing offset/limit instead of returning content.

### [x] tools-file#19 — NotebookEdit drops replace->insert-past-end conversion and cell_type change on …
`◑ partially_fixed` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Replace rejects one-past-end (good), but silently drops input.cell_type changes (gap remains)
- **TS:** NotebookEditTool.ts:371-377 — replace at one-past-end (`cellIndex === notebook.cells.length`) auto-converts to insert (defaulting cell_type to code). :425-427 — `if (cell_type && cell_type !== targetCell.cell_type) targetCell.cell_type = cell_type` applies a cell_type switch on replace.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/notebook_edit.rs:353-361 (Replace arm rejects one-past-end at line 353: cell_index >= cells.len() returns InvalidInput); but :371-378 (Replace arm mutates source/execution_count/outputs, zero handling of input.cell_type)
- **Fix:** In the Replace arm: if cell_index == cells.len(), fall through to the Insert path (default cell_type to Code). Otherwise, after mutating source, if input.cell_type is Some and differs from the cell's current cell_type, set cells[cell_index]["cell_type"].

### [x] tools-file#21 — Edit cannot create a new file via empty old_string
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Edit rejects nonexistent files; cannot create new file via empty old_string (TS allows it)
- **TS:** FileEditTool.ts:224-246 — when the file does not exist and old_string==='' it returns `{ result: true }` (valid new-file creation); FileEditTool.ts:248-264 — old_string==='' on an existing empty file is content insertion; the call path (getPatchForEdits, utils.ts:313-322) writes new_string as the whole file when old_string==''.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/edit.rs:162-168 unconditionally rejects nonexistent files before inspecting old_string; TS FileEditTool.ts:224-246 allows nonexistent file creation when old_string===''
- **Fix:** Before the existence check, if old_string is empty: allow nonexistent path (write new_string as the file, creating parent dirs); for an existing empty file allow replacing empty content with new_string. Otherwise keep the not-found error.

### [x] tools-file#22 — Edit deletion (empty new_string) does not strip the trailing newline
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Edit deletion silently leaves orphan newline; TS applies trailing-newline strip logic
- **TS:** FileEditTool/utils.ts:206-228 `applyEditToFile`: when new_string==='' and old_string lacks a trailing '\n' but `originalContent.includes(oldString + '\n')`, it removes `old_string + '\n'`. This is on the production single-edit path (utils.ts:313-322 → applyEditToFile).
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/edit.rs:296-342 calls replacen(old_string, new_string, 1) directly; zero invocation of edit_utils::apply_edit_to_file which holds the trailing-newline-strip logic (edit_utils.rs:195-214 unused on production path)
- **Fix:** Route the single-edit deletion case through `edit_utils::apply_edit_to_file` (or inline the same target = old+"\n" selection) in execute when new_string is empty, instead of the plain replacen/replace.

### [x] tools-file#20 — NotebookEdit insert silently defaults cell_type instead of erroring (errorCode …
`● genuinely_open` · **LOW** · effort **trivial** · fix-sketch *sound*

- **Gap:** NotebookEdit insert defaults cell_type to Code instead of requiring it (errorCode 5)
- **TS:** NotebookEditTool.ts:210-216 — `if (edit_mode === 'insert' && !cell_type)` returns errorCode 5 'Cell type is required when using edit_mode=insert.'
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/notebook_edit.rs:381 unwrap_or(NotebookCellType::Code) silently defaults insert when cell_type is None; TS NotebookEditTool.ts:210-216 returns errorCode 5 validation failure
- **Fix:** Add a validate_input (or early execute check): when edit_mode==Insert and input.cell_type is None, return InvalidInput 'Cell type is required when using edit_mode=insert' rather than defaulting to Code.

### [x] tools-file#24 — Read does not validate pages param up-front; open-ended and over-limit ranges d…
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** Read pages validation absent; malformed ranges silently read all pages instead of errorCode 7/8
- **TS:** FileReadTool.ts:418-440 validateInput rejects malformed pages with errorCode 7 and rejects ranges > PDF_MAX_PAGES_PER_READ(20) with errorCode 8 BEFORE any I/O. pdfUtils.ts:16-50 parsePDFPageRange supports open-ended 'N-' → lastPage Infinity.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/read.rs:235-250 (validate_input never inspects pages field); :1393-1409 (parse_page_range returns None on parse error, falls back to all-pages silently via :1306 unwrap_or); zero errorCode 7/8 paths exist
- **Fix:** Add pages validation to validate_input (reject malformed with a clear error; reject range>20). In parse_page_range, support the 'N-' open-ended form (right side empty → end=total) like TS.

### [x] tools-file#25 — Read byte cap truncates full reads instead of throwing FileTooLargeError
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** Read byte cap truncates instead of throwing FileTooLargeError; applied to all reads not just full reads
- **TS:** FileReadTool.ts:1019-1028 calls readFileInRange with maxBytes = (limit===undefined ? maxSizeBytes : undefined) and no truncateOnByteLimit (defaults false). readFileInRange.ts:95-102 — on a full read where file size > maxSizeBytes it throws FileTooLargeError (the ~100-byte error). limits.ts:1-14 documents that truncation was deliberately tried and reverted. …
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/read.rs:546-575 truncates at DEFAULT_BYTE_LIMIT (256_000) with footer; TS FileReadTool.ts:1019-1028 throws FileTooLargeError for full reads exceeding maxSizeBytes (262144, only when limit===undefined)
- **Fix:** For full reads (no limit arg) where total file size exceeds the byte cap, return a FileTooLargeError-equivalent error instructing offset/limit instead of truncating; align the threshold to 262144 and only apply the cap when limit is unset.

### [x] tools-file#26 — Edit does not reject .ipynb files (TS errorCode 5 routes to NotebookEdit)
`● genuinely_open` · **LOW** · effort **trivial** · fix-sketch *sound*

- **Gap:** Edit accepts .ipynb files, corrupts notebook JSON; TS redirects to NotebookEdit (errorCode 5)
- **TS:** FileEditTool.ts:266-273 — `if (fullFilePath.endsWith('.ipynb'))` returns errorCode 5 'File is a Jupyter Notebook. Use the NotebookEdit tool to edit this file.'
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/edit.rs:104-116 (validate_input checks only file_path/old_string equality, zero ipynb check); no .ipynb extension guard anywhere in edit.rs; TS FileEditTool.ts:266-273 returns errorCode 5 'use NotebookEdit tool'
- **Fix:** In Edit validate_input, if file_path ends with '.ipynb' return an invalid result telling the model to use the NotebookEdit tool.

### [x] tools-file#29 — NotebookEdit validate_input never runs; .ipynb extension/cell-not-found checks …
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** NotebookEdit has no validate_input; .ipynb extension guard missing, parse failures on non-notebooks
- **TS:** NotebookEditTool.ts:189-196 validateInput rejects a non-.ipynb path with errorCode 2 'File must be a Jupyter notebook (.ipynb file). For editing other file types, use the FileEdit tool.'; :270-289 pre-checks the resolved cell index exists (errorCode 7/8).
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/notebook_edit.rs:158-473 (impl Tool has no validate_input override; inherits no-op default from coco-tool-runtime:674-676); no .ipynb extension check anywhere; TS NotebookEditTool.ts:189-196 validates extension in validateInput
- **Fix:** Add an extension check (validate_input or early execute): if the path does not end with '.ipynb', return an error redirecting the model to the FileEdit tool, matching TS errorCode 2.

### system-reminder — turn-counter / gating (4)

### [x] system-reminder#103 — Todo/Task reminder-gap counter uses human turns, not assistant turns
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound*

- **Gap:** Todo/task reminder cadence uses human turns, not assistant turns; reminder counter frozen across tool-loop rounds within one human prompt
- **TS:** attachments.ts:3212-3264 getTodoReminderTurnCounts and 3319-3373 getTaskReminderTurnCounts compute turnsSinceLastReminder by scanning history backward, counting non-thinking ASSISTANT messages until the most recent todo_reminder/task_reminder attachment. getTodoReminderAttachments (3300-3303) fires when turnsSinceLastTodoWrite>=10 AND turnsSinceLastReminder…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/system-reminder/src/turn_counting.rs:99-109 (count_human_turns), turn_runner.rs:287-295 (throttle_gap using turn_number which is human turns), engine_turn_reminders.rs:212 (reminder_human_turn_number = count_human_turns), generators/todo_reminders.rs:83-84 (gates on ctx.turns_since_last_todo_reminder >= 10)
- **Fix:** Add count_assistant_turns_since_attachment(messages, AttachmentKind::TodoReminder/TaskReminder) in turn_counting.rs and feed those into turns_since_last_todo_reminder/turns_since_last_task_reminder; for these two types make the orchestrator gate on assistant turns (or set their ThrottleConfig.min_turns_between=0 and rely solely on the generator's assistant-…

### [x] system-reminder#105 — Diagnostics reminder lacks the Bash-tool presence gate
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound_with_caveats*

- **Gap:** Diagnostics reminder lacks TS Bash-tool presence gate; read-only subagents receive un-actionable reminders and unconditional take_dirty drain consumes diagnostics
- **TS:** attachments.ts:2854-2862 getDiagnosticAttachments and 2883-2891 getLSPDiagnosticAttachments both early-return [] when the Bash tool is absent from toolUseContext.options.tools ('diagnostics are only useful if the agent has the Bash tool to act on them').
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/system-reminder/src/generators/diagnostics.rs:35-49 (DiagnosticsGenerator::generate only checks ctx.diagnostics.is_empty(), never checks ctx.tools for ToolName::Bash). No Bash-tool gate anywhere in the diagnostics path.
- **Fix:** In diagnostics.rs::generate, return Ok(None) unless ctx.tools contains ToolName::Bash (mirror TS). Suppress before the take_dirty drain so a no-Bash agent doesn't consume diagnostics — i.e. gate at the generate() call or skip the DiagnosticsSource materialize when Bash is absent.

### [x] system-reminder#109 — Queued commands may double-inject: live drain path + reminder generator both re…
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Queued commands may double-inject via snapshot_for_reminder path + finalize drain path; both read, convert, and (for drain) remove; asynchronous TaskNotification enqueue creates inter-round gap window
- **TS:** attachments.ts:829 getQueuedCommandAttachments is the single source of queued_command attachments; query.ts:1570-1642 takes one snapshot (getCommandsByMaxPriority), converts it to attachments via getAttachmentMessages, then removeFromQueue(consumedCommands) — read-convert-remove is one atomic flow producing exactly one attachment per queued item per turn.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/app/query/src/engine_turn_request.rs:114-126 (run_turn_reminder_pipeline calls snapshot_for_reminder at engine_turn_reminders.rs:534), core/system-reminder/src/generators/queued_command.rs:46-73 (QueuedCommandGenerator emits ReminderMessage per item), engine_finalize_turn.rs:501-508 (drain_command_queue_into_history as separate …
- **Fix:** Make the queued-command attachment single-source: either drop QueuedCommandGenerator from the default registration and rely solely on the turn-boundary finalize drain, or have snapshot_for_reminder atomically remove the items it surfaces (matching TS read-convert-remove) so the finalize drain cannot re-inject them.

### [x] system-reminder#110 — count_human_turns is whole-history monotonic, never reset per-turn — mechanism …
`● genuinely_open` · **LOW** · effort **medium** · fix-sketch *sound*

- **Gap:** count_human_turns is whole-history monotonic, never reset per-turn; frozen across tool-loop rounds within one prompt causing todo/task cadence under-fire in long agentic sessions
- **TS:** attachments.ts:3212-3264 / 3319-3373 recompute assistantTurnsSinceReminder by scanning backward to the marker attachment on every call, so the counter advances with each assistant turn even inside one human prompt.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/system-reminder/src/turn_counting.rs:99-109 (count_human_turns sums Message::User over full history, monotonic), turn_runner.rs:287-295 (throttle_gap = turn_number.saturating_sub(last_generated_turn)), engine_turn_reminders.rs:212 (reminder_human_turn_number = count_human_turns). No assistant-turn-since-reminder history sca…
- **Fix:** Introduce an assistant-turn-since-attachment scan (count_assistant_turns_since_attachment for AttachmentKind::TodoReminder/TaskReminder) rather than reusing human turn_number for these two generators' reminder cadence.

### tool-runtime — interrupt / sibling-abort / budget (4)

### [~] tool-runtime#8 — Bash-error sibling abort never fires on the production tool path
`◑ partially_fixed` · **MEDI** · effort **medium** · fix-sketch *sound*

> **✅ PART A LANDED 2026-06-07 (commit `d08c8ef630`).** Streaming path now fires the sibling-abort:
> shared `StreamingToolExecutor::abort_siblings_if_shell_error` called in-task from `start_safe_now`
> (+ `run_concurrent_batch` refactored to it), `is_shell_tool_id` made `pub(crate)`, streaming test added.
> **PART B still deferred** (predicate parity for ordinary non-zero-exit Bash, which returns `Ok`/
> `error_kind:None`): needs a new `is_error` signal threaded through the Tool trait + outcome model —
> disproportionate to its efficiency-only value (coco serializes all mutating Bash, so concurrent siblings
> are always read-only; no correctness/safety benefit). See the deep-validation note below.

> **2026-06-07 audit — corrects the framing + records the defer rationale.** The original "execute_concurrent
> is dead code → no sibling-abort anywhere" framing is HALF WRONG: the **non-streaming** live path
> (`ToolCallRunner::execute_with` → `run_concurrent_batch`) DOES implement sibling-abort correctly
> (`executor.rs:905-910`, tested `executor.test.rs:714`). But streaming is the production **default**
> (`config.rs:396 streaming_tool_execution=true`), and the streaming path (`executor_streaming.rs`
> `commit_flush`/`terminal_drain`) has ZERO sibling references — so a Bash failure in a concurrent batch
> does not cancel its siblings. This is a real **default-path** gap, NOT a dead-path one; the old
> "Wave-4 high-risk/low-value" label understated it.
>
> **Why deferred (sequencing, not dropped — answers "why defer"):** (1) *Risk* — it edits the concurrent
> streaming join loop where abort fires on JoinSet completion order; getting the synthetic
> "Cancelled: parallel tool call errored" outcome + ordering right needs a focused test, unlike the
> trivial `app/cli` plugin wiring. (2) *Impact is efficiency, not safety* — un-aborted siblings merely
> run to completion and commit wasted results; the turn still proceeds and the model still sees the Bash
> error. No data loss / no security hole. (3) *Isolation* — it is wholly inside `core/tool-runtime` and
> deserves its own review, so it is **PR #2** (after the plugin-wiring PR #1), not a drop.
> **Fix is mostly wired already:** `make_runtime` already passes `Some(self.sibling_abort.signal())` into
> spawned safe tools (`executor.rs:860`) — they already *listen*; only the *trigger* is missing. Make
> `is_shell_tool_id` (`executor.rs:1001`) `pub(crate)` and, in `commit_flush`'s + `terminal_drain`'s join
> loops, after each `unstamped` resolves with `error_kind.is_some() && is_shell_tool_id(tool_id)`, call
> `sibling_abort.abort(SiblingError{..})` before stamping the next; mirror `executor.test.rs:714` through
> `feed_plan`+`commit_flush`.
>
> **2026-06-07 DEEP VALIDATION — the fix above is INCOMPLETE; two compounding gaps + a placement error.**
> Verified end-to-end (field + listen exist; trigger missing on the streaming default path). But:
> - **(a) Placement: `commit_flush`/`terminal_drain` is the WRONG insertion point.** Safe tools are spawned
>   during streaming (`start_safe_now`); by the time `commit_flush` drains the JoinSet via `join_next`, the
>   concurrent siblings have often already finished → aborting there is frequently a no-op (too late). The
>   abort must fire **inside the spawned task at completion time** — wrap `run_one` in `start_safe_now` so a
>   shell-tool error calls `sibling_abort.abort(SiblingError)` the moment it resolves (during streaming),
>   while siblings are still inflight. Share the predicate with `run_concurrent_batch`.
> - **(b) Predicate is keyed on the wrong signal — the trigger is inert even on the NON-streaming path.**
>   `tool_outcome_builder::from_execution` sets `error_kind` to `Some` only when `execute()` returns `Err`
>   (ExecutionFailed/Cancelled). Bash returns **`Ok`** with `error_kind: None` on ordinary non-zero exit
>   (`bash.rs:584-591` composes an "Exit code N" string but still `Ok`s). So `error_kind.is_some() &&
>   is_shell_tool_id` NEVER fires for normal command failure — the only "passing" test (`executor.test.rs:714`)
>   hand-forces `error_kind`. Fix = propagate the command-level `is_error` (coco already computes
>   `interpret_command_result(...).is_error` at `bash.rs:588`) into the outcome and OR it into the predicate
>   (cleaner than changing Bash's Ok/Err contract, which would ripple into budget/retry/display).
> - **VALUE NUANCE (why this stays its own low-urgency PR despite being "more broken"):** in coco's model
>   *all* concurrent siblings are read-only/safe (mutating Bash is serialized — "no mixed safe+unsafe
>   inflight", `executor_streaming.rs:44-49`). Aborting read-only siblings is a pure **efficiency** win
>   (stop wasted work), NOT a correctness/safety guard — read-only tools have no side effects to prevent.
>   TS's dependency-safety motivation (`mkdir && write`) doesn't apply here. Real gap, bounded value →
>   do it when touching this code; not urgent. Effort medium, risk medium (touches concurrent-abort
>   ordering + the outcome `is_error` signal).

- **Gap:** Sibling abort on Bash error never fires in production; execute_concurrent path is dead code
- **TS:** StreamingToolExecutor.ts:354-363: when a tool yields an error tool_result AND tool.block.name === BASH_TOOL_NAME, it sets hasErrored, captures erroredToolDescription, and calls siblingAbortController.abort('sibling_error'). Sibling concurrent tools detect getAbortReason()==='sibling_error' (:216-218), break their generator loop (:336-345), and get a synthet…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tool-runtime/src/executor.rs:368-398 exist but ZERO production callers across coco-rs (only lib.rs:21 pub mod re-export). /lyz/codespace/codex/coco-rs/app/query/src/engine_turn_request.rs:226-238 and tool_call_runner.rs:209-225 both use bare tokio::select! with no sibling-error handling.
- **Fix:** In both live run_one closures, fork a child token off RunOneRuntime.cancellation + RunOneRuntime.sibling_abort into call_ctx.cancel, and after build_outcome_from_execution detect a Bash/PowerShell error outcome and call runtime.sibling_abort.cancel(); have run_concurrent_batch/StreamingHandle seed sibling_cancel per batch.

### [x] tool-runtime#9 — Tool::interrupt_behavior (Cancel vs Block) is never consulted in production ✅ VERIFIED RESOLVED (2026-06-07)
`✅ fixed` · **MEDI** · effort **medium** · fix-sketch *done*

> **2026-06-07 audit (tracker was WRONG):** `interrupt_behavior()` IS consumed in production at
> `core/tool-runtime/src/executor.rs:998` (`interruptible_set`, on both the SerialUnsafe and ConcurrentSafe
> batch paths via `emit_interruptibility` at :824/:838). Per-tool assignment matches TS: `SleepTool`=Cancel
> (`core/tools/src/tools/shell_tools.rs:61`), Bash/PowerShell=Block (default `traits.rs:548`). The submit-time
> gate is `app/tui/src/update.rs:391` (`has_submit_interruptible_tool_in_progress`, fed by
> `tui_only.rs:341`). The "zero callers" claim cited the wrong file (`tool_call_runner.rs`).

- **Gap:** Tool::interrupt_behavior enum defined but never consulted; all tools cancelled uniformly via turn-wide token
- **TS:** StreamingToolExecutor.ts:210-241: getAbortReason() consults getToolInterruptBehavior(tool) — on user interrupt, only tools whose interruptBehavior()==='cancel' get user_interrupted; 'block' tools return null (not cancelled). :254-260 updateInterruptibleState() calls toolUseContext.setHasInterruptibleToolInProgress(...) so the UI knows ESC can interrupt only…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tool-runtime/src/traits.rs:51-59 defines InterruptBehavior::{Cancel,Block} default Block; grep for interrupt_behavior() across app/ yields ZERO hits. app/query/src/tool_call_runner.rs:222-225 uses bare tokio::select! on turn-wide cancel token, never per-tool interrupt_behavior.
- **Fix:** Thread per-tool interrupt_behavior into the cancel path: on UserCancel, only cancel tools whose interrupt_behavior()==Cancel (let Block tools finish), and add a ToolUseContext callback / AppState flag mirroring setHasInterruptibleToolInProgress driven by the in-progress tool set.

### [x] tool-runtime#10 — Level 2 per-message budget total counts opted-out (Read) and already-replaced r…
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Budget trigger counts opted-out Read at full content_chars and already-replaced results at replacement.len(); TS excludes both
- **TS:** toolResultStorage.ts:823-831: over-budget total = frozenSize + freshSize, where eligible = fresh.filter(!shouldSkip) EXCLUDES opted-out skipToolNames (Read maxResultSizeChars=Infinity), and mustReapply (already-replaced) is partitioned out entirely (:797-804) and never counted toward the trigger. :809-814 also skips the whole group when fresh.length===0.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tool-runtime/src/tool_result_storage.rs:428-436 — aggregate sums replacements.get(...).len() | c.content_chars over ALL candidates including opted-out. Fresh filter at :446-454 correctly excludes opted_out, but trigger-total at :428-436 does not (before fresh filter).
- **Fix:** Compute the trigger total from the same eligible set used for selection: sum only frozen (seen, not replaced, not opted-out) + fresh-eligible (not opted-out), excluding already-replaced and opted-out; optionally short-circuit when no fresh-eligible candidates exist.

### [x] tool-runtime#12 — User-interrupt cancelled tool result uses 'Error: cancelled' instead of TS inte…
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** User-interrupt tool result commits 'Error: cancelled' instead of '[Request interrupted by user for tool use]'
- **TS:** On an execution-stage user interrupt, toolExecution.ts:1691-1726 commits content = formatError(error); toolErrors.ts:5-8 formatError(AbortError) = error.message || INTERRUPT_MESSAGE_FOR_TOOL_USE = '[Request interrupted by user for tool use]', with toolUseResult = `Error: [Request interrupted by user for tool use]`. (The pre-execution gate :415-452 uses CANC…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tool-runtime/src/error.rs:62 ToolError::Cancelled Display = 'cancelled'; :114 format_tool_error is dead code (zero production callers); app/query/src/tool_outcome_builder.rs:301-302 routes Cancelled through error.to_string() producing 'Error: cancelled' instead of interrupt message.
- **Fix:** In the Cancelled branch route the rendered text through format_tool_error (or an explicit interrupt-message constant) instead of error.to_string(), so the committed tool_result reads as an explicit interruption rather than a generic error.

### config — validation-suite & toggles dead code (3)

### [x] config#243 — available_models allowlist matching is a naive substring check, not enforced, w…
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound*

- **Gap:** Model allowlist matching diverges: TS uses family-alias/version-prefix/exact tiers with segment-boundary checking; Rust uses only exact-match in picker, dead code with naive byte-identical contains() in validator
- **TS:** modelAllowlist.ts:100 isModelAllowed has 3 ordered tiers: family-alias wildcard (narrowable by specific entries via familyHasSpecificEntries segment-boundary check), version-prefix at segment boundary (prefixMatchesModel: matches '...-4-5-20251101' but not '...-4-50'), exact full IDs, plus bidirectional alias resolution; an empty allowlist returns false = b…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/common/config/src/settings/validation.rs:783-799 model_matches_spec — lines 791 and 795 both use identical contains() logic (byte-identical dead code); validation.rs:73 model_matches_spec called only in validation path which has zero production callers (grep confirms no callers outside validation.rs/validation.test.rs); app/tui/…
- **Fix:** Port isModelAllowed (segment-boundary prefixMatchesModel + familyHasSpecificEntries + empty-list-blocks-all) into a shared fn; use it for the picker filter at show.rs:75 and treat empty available_models as block-all, not None.

### [~] config#247 — Fast-mode org prefetch is a stub and availability/model-support gates are unwir…
`◑ partially_fixed` · **MEDI** · effort **medium** · fix-sketch *sound*

> **✅ CORE LANDED 2026-06-07 (commit `e39ed258d1`).** Fast mode now actually works + is provider-agnostic:
> (1) gate is capability-driven — engine checks `active_snapshot.model_info.has_capability(FastMode)`;
> `is_fast_mode_supported_by_model` rewritten to a builtin-registry capability lookup (fixes the always-false
> `opus-4-6` substring; TUI toggle inherits the fix). (2) wire producer added — `PerCallOverrides.fast_mode`
> → `build_call_options` writes `speed=fast` into `provider_options["anthropic"]` (Anthropic-only lane) so
> the `fast-mode-2026-02-01` beta fires. Tests: capability gate + 3 wire cases.
>
> **GLOBAL-SINGLETON REMOVED (commit after `85bd6ccc83`).** The `fast_mode_global()` process-wide
> `OnceLock<Mutex<…>>` and its entire cooldown/org/availability/opt-in API were **0-production-reader dead
> code** — deleted outright (per "delete dead code" + "no global singleton" — instance state, not a global).
> This is the root-cause fix for the test race: the earlier `#[serial(fast_mode_global)]` patch
> (`85bd6ccc83`) treated the symptom and is now reverted (no global ⇒ no race ⇒ no serialization needed; the
> one surviving test is pure). The only live fast-mode state is `config.fast_mode: bool` on the engine config,
> already instance-level. **Still deferred (when implemented, hold state on the `SessionRuntime` INSTANCE —
> never re-introduce a global):** availability gating (env-disable + per-session opt-in provider-agnostic;
> first-party/org Anthropic-scoped), `getInitialFastModeSetting` seeding (+ a `Settings.fast_mode_per_session_opt_in`),
> 429/503 cooldown, SDK `fast_mode_state()` accessor, and Tier-B org prefetch (vercel-ai-anthropic).

> **2026-06-07 audit — landed + a NEW multi-provider correctness bug + the understated surface.**
>
> **Landed (since the original entry):** `UserCommand::ToggleFastMode` IS now consumed
> (`app/cli/src/tui_runner.rs:1127-1143` — toggles `set_session_opted_in` + `update_engine_config` +
> emits `FastModeChanged`), and the per-turn flag is threaded (`engine_turn_request.rs:171-172`,
> no longer hardcoded false). So the original "dead-end toggle" is fixed.
>
> **NEW — multi-provider correctness bug (the important part).** coco-rs is a **multi-LLM-provider** SDK;
> fast-mode support is a **per-model capability** and must be **capability-driven, provider-agnostic** —
> NOT provider-gated. Any provider/model that declares `Capability::FastMode` supports fast mode; whichever
> provider crate owns that model is responsible for the wire translation (Anthropic → beta header
> `fast-mode-2026-02-01`, `vercel-ai/anthropic/src/beta_capabilities.rs:32`; another provider → its own
> mechanism). But `is_fast_mode_supported_by_model(model_id: &str)` (`common/config/src/fast_mode.rs:249`)
> is a **literal TS port** — `model_id.to_lowercase().contains("opus-4-6")` — which is wrong for
> multi-provider: (a) it ignores `Capability::FastMode`, which is ALREADY declared per-model in the catalog
> (`common/config/src/builtin/anthropic.rs:55/88/121`) and is the **single, data-driven source of truth**;
> (b) the hardcoded substring won't extend to future or non-Anthropic fast-capable models without a code
> edit, and could mis-fire on an unrelated id containing `opus-4-6`. The per-turn gate
> (`engine_turn_request.rs:172`) consults ONLY this substring — no `Capability::FastMode` check, no
> `check_fast_mode_availability`. **The fix is capability-only; do NOT add a `provider == Anthropic` gate.**
>
> **Also dead/absent (tracker understated "just the prefetch stub"):**
> - `check_fast_mode_availability` (`fast_mode.rs:211`) is **not consulted** by the toggle or the per-turn
>   gate (dead). Note: its env-disable (`COCO_DISABLE_FAST_MODE`) + per-session opt-in gates are
>   provider-agnostic and fine to keep, but its blanket `is_first_party` requirement + org-status (penguin
>   mode) checks are **Anthropic-org-specific** — consistent with capability-driven support they must NOT
>   blanket-block a non-Anthropic model that declares `Capability::FastMode`; scope them to the Anthropic
>   provider (Tier B) rather than gating all providers.
> - `getInitialFastModeSetting` not mirrored: `Settings.fast_mode` is never read to seed the session
>   (`config.rs:402` hardcodes `fast_mode: false`); fast mode always starts off.
> - cooldown (`trigger_cooldown*`, `fast_mode.rs:119-138`) has zero production callers — not driven from
>   the inference 429/503 path.
> - SDK `fast_mode_state()` (`app/cli/.../mod.rs:229` impls) returns hardcoded `None` instead of computing
>   `FastModeState` (Off/Cooldown/On).
>
> **Fix — Tier A (do-now eligible, no network, restores correct multi-provider gating):**
> (1) Rewrite the support gate to be **capability-driven (no provider check)**: resolve the model's
> `ModelInfo`/`ResolvedModel` via `RuntimeConfig` and return `capabilities.contains(Capability::FastMode)`
> — replacing the `opus-4-6` substring; thread that (not the bare `model_id` string) into
> `engine_turn_request.rs:172`. (2) Seed `config.fast_mode` from `Settings.fast_mode` gated by the new
> support check + `check_fast_mode_availability` at `SessionRuntime` build (mirror `getInitialFastModeSetting`).
> (3) Gate the toggle (`tui_runner.rs:1127`) on `check_fast_mode_availability`, emitting the unavailable reason.
> (4) Drive `trigger_cooldown_from_status` from the inference 429/503 retry path. (5) Compute SDK
> `fast_mode_state()`. **Tier B (defer — multi-provider boundary):** the real org prefetch HTTP to
> `/api/claude_code_penguin_mode` + `penguinModeOrgEnabled` persistence belongs in **vercel-ai-anthropic**
> (per CLAUDE.md "provider concerns stay in provider crates"); `set_org_fast_mode_status` is the seam.
> **2026-06-07 DEEP VALIDATION — two findings that change the severity + the remediation.**
> - **(a) The substring is ALWAYS-FALSE for every shipped model (worse than provider-narrow).** The builtin
>   catalog ships `claude-sonnet-4-6` / `claude-opus-4-7` / `claude-haiku-4-5` (`builtin/anthropic.rs:42/75/110`)
>   — **none contains `"opus-4-6"`** (`rg opus-4-6 builtin/` = no match), yet all three declare
>   `Capability::FastMode` (:55/88/121). So `is_fast_mode_supported_by_model` returns **false for every
>   fast-capable model**; pressing the toggle computes `active=false` and fast mode can never turn on. The
>   capability-driven rewrite fixes this for free. Also: capability is already reachable at the engine gate —
>   `active_snapshot.model_info` is in scope (`engine_turn_request.rs:90`) and `has_capability` is already
>   called ~20 lines below (:194-200); **no `RuntimeConfig` threading needed** for the engine path (the
>   original fix-sketch over-scoped this). **Delete** `is_fast_mode_supported_by_model(&str)` — a bare string
>   cannot answer a capability question — and inline `info.has_capability(Capability::FastMode)` at both sites.
> - **(b) MISSING SHIP-BLOCKER: there is no `speed=Fast` wire producer.** Even with the gate fixed,
>   `QueryParams.fast_mode` is consumed ONLY by the cache-break detector (`client.rs`); **nothing in
>   `services/inference` or `app/` ever sets `AnthropicProviderOptions.speed = Speed::Fast`** (verified
>   repo-wide). The anthropic provider emits the `fast-mode-2026-02-01` beta ONLY when `speed == Fast`
>   (`vercel-ai/anthropic/src/messages/anthropic_messages_language_model.rs`). So today the toggle is a pure
>   no-op on the wire, and Tier A as originally written (fix the gate) would STILL do nothing. **Tier A must
>   also add the producer** at the inference seam (`build_call_options`/`model_factory`) translating
>   `fast_mode==true` → `provider_options[anthropic].speed=Fast` (provider-agnostic: only Anthropic consumes
>   it today; other providers ignore the flag until they add their own translation).
> - **Other prereqs the sketch missed:** `check_fast_mode_availability` hard-returns on `!is_first_party`
>   (`fast_mode.rs:221`) — for capability-driven parity that org/first-party check must be **Anthropic-scoped**,
>   not blanket; and `getInitialFastModeSetting` seeding needs a **net-new `Settings.fast_mode_per_session_opt_in`**
>   field that does not exist yet. SDK `supports_fast_mode` (`session.rs:207/217`) is hardcoded `Some(true)` —
>   low-impact, but should derive from `Capability::FastMode` for true parity.
>
> `get_fast_mode_model` returning a hardcoded dated id is a related P3 (config#252) — fast mode is a *mode*
> (beta header on the same model), not a separate model.

- **Gap:** Fast-mode toggle is wired dead-end: TUI emits ToggleFastMode but it falls into unhandled catch-all; every request hardcodes fast_mode:false
- **TS:** fastMode.ts:407 prefetchFastModeStatus makes a real /api/claude_code_penguin_mode call with auth retry + cache fallback; :167 isFastModeSupportedByModel gates fast mode to opus-4-6; getInitialFastModeSetting/getFastModeState wire the toggle into request behavior.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/common/config/src/fast_mode.rs:251-256 prefetch_fast_mode_status explicitly marked TODO stub with hardcoded status; tui/src/update.rs:356-358 emits UserCommand::ToggleFastMode but no handler in tui_runner.rs (grep finds zero UserCommand::ToggleFastMode case arms); app/query/src/engine_turn_request.rs:172 hardcodes fast_mode:fals…
- **Fix:** Consume UserCommand::ToggleFastMode in the session runtime to set the engine's per-turn fast_mode flag from session.fast_mode (instead of hardcoded false), and add a model-support gate; leave the real org API prefetch to vercel-ai-anthropic.

### [x] config#253 — validate_settings (and its entire validation suite) has zero production consume…
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Entire validation suite compiled and tested but zero production consumers: malformed permission rules silently pass through verbatim with no user feedback
- **TS:** validation.ts + settings.ts surface structured ValidationErrors to the user at settings load and on /config writes (e.g. formatZodError, the policyErrors path in the load loop).
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/common/config/src/settings/validation.rs:44 validate_settings, :616 filter_invalid_permission_rules — repo-wide grep finds ZERO production callers outside validation.rs/validation.test.rs; settings/mod.rs:717 load_and_merge never calls validate_settings or filter_invalid_permission_rules; no SettingsWithErrors return type or err…
- **Fix:** Call validate_settings (and filter_invalid_permission_rules) from the settings load path / load_settings_with and on /config writes, emitting the ValidationErrors as user-visible warnings.

### context / messages — memory guards, currentDate, API-boundary (8)

### [x] context#96 — Small PDFs are never inlined; page count is a crude size heuristic
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Small PDFs never inlined; threshold 20 vs TS/config 10; no real page-count parser (pdf-extract exists in read.rs but unused here)
- **TS:** attachments.ts:2986-3018 tryGetPDFReference computes a REAL page count via getPDFPageCount (pdfinfo), falling back to size heuristic only when unavailable, and returns a reference ONLY when effectivePageCount > PDF_AT_MENTION_INLINE_THRESHOLD (10). For small PDFs it returns null, so generateFileAttachment (3068-3074, 3121+) falls through to readTruncatedFil…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/context/src/attachment.rs:44 PDF_INLINE_THRESHOLD=20 (vs config constant 10); :579-602 generate_pdf_reference both branches return PdfReference (no text extraction); :583 estimated_pages always size heuristic (no real page count)
- **Fix:** On the @mention path, get a real page count (reuse pdf-extract page-separator counting from read.rs), use PDF_AT_MENTION_INLINE_THRESHOLD; for small PDFs fall through to extracting text via the Read tool's PDF reader and inline it as a FileAttachment.

### [x] context#97 — Nested-memory traversal has no pathInAllowedWorkingPath guard
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Nested-memory traversal lacks pathInAllowedWorkingPath guard; reads files outside allowed working roots unconditionally (instruction-injection vector)
- **TS:** attachments.ts:1799-1803 getNestedMemoryAttachmentsForFile early-returns an empty array when !pathInAllowedWorkingPath(filePath, appState.toolPermissionContext) — gating per-file nested-memory traversal on the trigger file being inside an allowed working path.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/app/query/src/engine_attachments.rs:49-91 drain_nested_memory_triggers passes ToolUseContext to traverse_for_file with no permission guard; permission_context field exists but unused for this check; no call to path_in_allowed_working_path
- **Fix:** Thread the permission context (allowed working roots) into the drain and add an early-return in drain_nested_memory_triggers (or traverse_for_file) that skips trigger files failing path_in_allowed_working_path, reusing shell_cwd's helper.

### [x] context#98 — File checkpointing default-on for non-interactive sessions; TS env overrides ab…
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound*

- **Gap:** SDK NDJSON sessions persist file-history by default (true); TS defaults OFF for non-interactive/SDK; no COCO_* env override exists
- **TS:** utils/fileHistory.ts:63-78 fileHistoryEnabled(): interactive defaults ON (fileCheckpointingEnabled !== false && !CLAUDE_CODE_DISABLE_FILE_CHECKPOINTING); non-interactive/SDK routes to fileHistoryEnabledSdk() which defaults OFF and requires CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING (and honors the disable env).
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/common/config/src/settings/mod.rs:264 #[serde(default = "default_true")] pub file_checkpointing_enabled; /lyz/codespace/codex/coco-rs/app/cli/src/session_runtime.rs:953-965 SDK path builds file_history=Some(...) when default true; no env key override (COCO_*_FILE_CHECKPOINTING missing)
- **Fix:** Add a COCO_* checkpointing EnvKey (enable-SDK + disable) and route the session_runtime gate through an fileHistoryEnabled-equivalent that defaults OFF for non-interactive/SDK sessions and ON for interactive, honoring the env overrides.

### [x] context#101 — Nested-memory injected files are never recorded in FileReadState
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Nested-memory injected files still never recorded in FileReadState; detect_changed_files remains blind to mid-session edits of auto-injected CLAUDE.md
- **TS:** attachments.ts:1725-1750 memoryFilesToAttachments calls readFileState.set with raw disk bytes (offset/limit undefined, isPartialView per contentDiffersFromDisk) for every injected nested-memory/CLAUDE.md file, so getChangedFiles surfaces mid-session edits to those files.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/context/src/attachment.rs:load_memory_attachment (0 FileReadState refs); /lyz/codespace/codex/coco-rs/app/query/src/engine_attachments.rs:49-91 drain_nested_memory_triggers passes ctx directly to traverse_for_file with no FileReadState.set call
- **Fix:** When injecting a nested-memory attachment, record a FileReadState entry caching the raw disk bytes + mtime (offset/limit None) so detect_changed_files picks up later edits; add an is_partial_view-equivalent marker if Edit/Write parity on raw-vs-shown content is desired.

### [x] messages#80 — No oversized-image validation at the API boundary (validateImagesForAPI)
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Rust pipeline has no oversized-image validation before API wire; 5MB cap constant unused
- **TS:** utils/messages.ts:2367 calls validateImagesForAPI as the final step of normalizeMessagesForAPI; utils/imageValidation.ts:65-104 scans every user message's base64 image blocks and throws ImageSizeError ('Please resize the image before sending') when base64 length exceeds API_IMAGE_MAX_BASE64_SIZE (5MB).
- **Rust (HEAD):** coco-rs/core/messages/src/normalize.rs has no validate_images_for_api call; coco-rs/common/config/src/constants.rs:87 defines API_IMAGE_MAX_BASE64_SIZE but grep finds zero production callers of this constant; coco-rs/services/inference/src/stream.rs:734-751 token_usage_from_provider_usage never checks base64 sizes; coco-rs/app/cli/src/at_mention_turn.rs:179…
- **Fix:** Add a final pass in normalize_messages_for_api that walks LlmMessage::User content for File parts with image media types, compares base64 length against API_IMAGE_MAX_BASE64_SIZE, and returns/surfaces a typed ImageSizeError (or drops with a clear error message) before the wire.

### [x] messages#81 — No strip of problematic document/image blocks after PDF/image/request-too-large…
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound*

- **Gap:** No error-keyed document/image block strip on PDF/image/request-too-large errors; repeated failures on resumed sessions
- **TS:** utils/messages.ts:2003-2054 builds errorToBlockTypes (keyed off getPdf*/getImage*/getRequestTooLargeErrorMessage from services/api/errors.ts:170-196) and a stripTargets map: on a synthetic API error it walks back to the nearest preceding isMeta user message and strips the offending document/image block (messages.ts:2113-2137) so it is not re-sent every turn.
- **Rust (HEAD):** coco-rs/core/messages/src/normalize.rs:308-422 normalize_messages_for_api lists 16 steps with no stripTargets/errorToBlockTypes equivalent; coco-rs/common/types/src/messages/message.rs:213-219 ApiError has only message/status_code/error_type, no PdfTooLarge/ImageTooLarge discriminants; grep for stripTargets/errorToBlockTypes across *.rs returns zero product…
- **Fix:** Add typed too-large error classification to ApiError (or detect via error_type/message), then a normalize pass that, on a preceding synthetic too-large api_error, strips the mapped block types (document/image) from the nearest prior meta/attachment user message.

### [x] messages#82 — local_command system messages are dropped from the API prompt instead of becomi…
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** LocalCommand system messages silently dropped before wire instead of becoming user turns for model to reference
- **TS:** utils/messages.ts:2068-2092 keeps SystemLocalCommandMessage in the filtered list (only non-local-command system messages are dropped), then case 'system' converts it via createUserMessage so the model can reference previous command output, merging into a prior user turn (mergeUserMessages) when the last result is a user message.
- **Rust (HEAD):** coco-rs/core/messages/src/normalize.rs:710-723 extract_llm_message returns None for ALL Message::System(_) variants unconditionally (line 716-720 comment: 'handled by system-reminder injection, not normalization' — but no such injection exists at coco-rs/app/query/src/engine_prompt.rs:153); coco-rs/app/cli/src/tui_runner.rs:4543-4552 emits Message::System(S…
- **Fix:** Special-case Message::System(SystemMessage::LocalCommand{command,output}) in extract_llm_message (or a pre-extract pass) to emit an LlmMessage::User wrapping the command output, and let the existing consecutive-user merge fold it into an adjacent user turn.

### [x] messages#83 — merge_consecutive_user_messages omits joinTextAtSeam newline, concatenating adj…
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Consecutive user prompts merge without seam newline, causing silent prompt corruption ('2+2' + '3+3' → '2+23+3')
- **TS:** utils/messages.ts:2411-2449 mergeUserMessages -> joinTextAtSeam (2505-2515): when the seam blocks are both text it appends '\n' to message A's trailing text block (because the Anthropic API concatenates adjacent text content parts with NO separator), and also runs hoistToolResults.
- **Rust (HEAD):** coco-rs/core/messages/src/normalize.rs:763 dest_content.extend(src_content) with no seam newline injection; test at coco-rs/core/messages/src/normalize.test.rs:127-141 asserts merged content.len()==2 with no newline; TS spec at /lyz/codespace/3rd/claude-code/src/cost-tracker.ts:177-179 shows pivot at 0.5, not 0.01
- **Fix:** In merge_consecutive_user_messages, when the last part of dest and first part of src are both Text, append '\n' to dest's trailing text before extending; optionally hoist tool_result parts to the front of the merged content.

### subagent — ordering & frontmatter (5)

### [x] subagent#113 — Handoff classifier adds read-only-agent and tool-count exemptions absent in TS
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** Handoff classifier short-circuits for read-only agents and zero-tool-use, diverging from TS which only gates on non-empty transcript.
- **TS:** agentToolUtils.ts:404-408 gates classifyHandoffIfNeeded ONLY on feature('TRANSCRIPT_CLASSIFIER') + toolPermissionContext.mode==='auto' + non-empty buildTranscriptForClassifier(agentMessages, tools). subagentType (line 441) and totalToolUseCount (line 442) feed analytics only, never a short-circuit. buildTranscriptForClassifier (yoloClassifier.ts:434) includ…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/subagent/src/handoff.rs:39-41 — should_classify() gates on !is_read_only_agent(agent_type) && total_tool_use_count > 0. is_read_only_agent() at lines 33-35 returns true for ['Explore', 'Plan', 'coco-guide'].
- **Fix:** Drop is_read_only_agent and the total_tool_use_count>0 gate from should_classify; gate only on a non-empty transcript (matching TS's `if (!agentTranscript) return null`).

### [x] subagent#116 — Auto-memory tool injection order differs from TS
`● genuinely_open` · **LOW** · effort **trivial** · fix-sketch *sound*

- **Gap:** Auto-memory tool injection order [Read, Edit, Write] is reversed from TS [Write, Edit, Read], altering prompt bytes and prompt-cache key.
- **TS:** loadAgentsDir.ts:458-462 (JSON path) and 665-669 (markdown path) both iterate [FILE_WRITE_TOOL_NAME, FILE_EDIT_TOOL_NAME, FILE_READ_TOOL_NAME] = [Write, Edit, Read], appending in that order.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/subagent/src/definition_store.rs:451 — for tool in [ToolName::Read, ToolName::Edit, ToolName::Write]. TS order (loadAgentsDir.ts:458-462, :665-669) is [Write, Edit, Read]. Rust reverses the sequence when appending to explicit allow-list.
- **Fix:** Change definition_store.rs:451 to `for tool in [ToolName::Write, ToolName::Edit, ToolName::Read]` to match TS injection order.

### [x] subagent#117 — Agent listing in AgentTool prompt is alphabetical, not TS source-load order
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** Agent listing in AgentTool prompt is alphabetical (BTreeMap) instead of source-load order [built-in, plugin, user, project, flag, managed], affecting model-visible prompt bytes and prompt-cache key.
- **TS:** loadAgentsDir.ts:203-220 getActiveAgentsFromList builds a Map in source-group order [built-in, plugin, user, project, flag, managed] preserving insertion order; prompt.ts:198-199 formatAgentLine iterates that order to render the 'Available agent types' block.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/subagent/src/snapshot.rs:25 — active: BTreeMap<String, AgentDefinition> (alphabetically ordered). Lines 37-38 active() returns self.active.values() which iterates BTreeMap in lexicographic order. Lines 99-111 in prompt.rs agent_list() calls snapshot.active() and renders in that order.
- **Fix:** Carry source-group + insertion order alongside the catalog (e.g. an ordered Vec for prompt rendering) so the agent listing renders built-in->plugin->user->project->flag->managed like TS, while keeping the BTreeMap for lookups.

### [x] subagent#119 — Rust parses requiredMcpServers from markdown frontmatter; TS never does
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** Rust parses requiredMcpServers from agent markdown frontmatter; TS never does. Field is only programmatically set in TS, making file-based values coco-rs-only.
- **TS:** loadAgentsDir.ts markdown parser reads frontmatter['mcpServers'] (line 693) but NEVER frontmatter['requiredMcpServers'] -- grep for frontmatter['requiredMcpServers'] across the TS tree returns zero hits. requiredMcpServers (interface line 122) is consumed by hasRequiredMcpServers (233) and AgentTool.tsx gating (367+) but is only set programmatically, never …
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/subagent/src/frontmatter.rs:208-210 — def.required_mcp_servers = read_csv_or_list_aliased(frontmatter, ["requiredMcpServers", "required_mcp_servers"]). TS loadAgentsDir.ts:693 reads only frontmatter['mcpServers'], never frontmatter['requiredMcpServers']; grep of TS tree confirms zero file-based setters for this field.
- **Fix:** Stop reading requiredMcpServers/required_mcp_servers from frontmatter in frontmatter.rs:208-210 (and json.rs translation), leaving the field programmatically-set-only as in TS.

### [x] subagent#120 — Classifier-unavailable handoff warning (UNAVAILABLE_WARNING) is never surfaced …
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** UNAVAILABLE_WARNING constant defined but never surfaced to parent on classifier error; both stage1/stage2 Err paths silently return response without warning.
- **TS:** yoloClassifier.ts:941-984 on API error returns shouldBlock:true + unavailable:true. agentToolUtils.ts:461-469: since shouldBlock is true, it enters the block branch, detects classifierResult.unavailable, and returns the 'Note: The safety classifier was unavailable...' UNAVAILABLE_WARNING prepended to the sub-agent output so the parent knows the safety revie…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/subagent/src/handoff.rs:160 defines UNAVAILABLE_WARNING constant. /lyz/codespace/codex/coco-rs/coordinator/src/agent_handle/handoff.rs:41 and :59 (stage1 and stage2 Err arms) both return qr.response_text.clone() with no warning prepended. TS agentToolUtils.ts:461-469 returns UNAVAILABLE_WARNING prepended to output; :617-619…
- **Fix:** In handoff.rs, on the classifier-error / no-handle arms return Some(format!("{UNAVAILABLE_WARNING}\n\n{output}")) (prepending the warning to qr.response_text) instead of returning the raw output silently.

### mcp / tools-web-mcp — per-domain & connect residuals (7)

### [x] tools-web-mcp#52 — Preapproved-host bypass for WebFetch is dead code (never consulted by the permi…
`◑ partially_fixed` · **HIGH** · effort **small** · fix-sketch *sound_with_caveats*

- **Gap:** Preapproved-host bypass is wired in Default mode (check_permissions + per-domain rules + suggestions + extraction branching all working), but auto_mode.rs still unconditionally forces WebFetch to NeedsPrompt without checking preapproval status
- **TS:** WebFetchTool.ts:104-121 checkPermissions early-returns {behavior:'allow', decisionReason:'Preapproved host'} when isPreapprovedHost(hostname, pathname) matches, so docs.python.org / developer.mozilla.org / github.com/anthropics fetch without prompting.
- **Rust (HEAD):** Live code verified: - /lyz/codespace/codex/coco-rs/core/tools/src/tools/web.rs:727-766 — WebFetchTool::check_permissions override with is_preapproved_url call - /lyz/codespace/codex/coco-rs/core/tools/src/tools/web.rs:206-309 — PREAPPROVED_WEB_HOSTS constant (73+ hosts) - /lyz/codespace/codex/coco-rs/core/tools/src/tools/web.rs:318-357 — is_preapproved_host…
- **Fix:** Add WebFetchTool::check_permissions that parses the URL, calls is_preapproved_host, and returns ToolCheckResult::Allow on match; remove the dead_code markers. Also short-circuit auto_mode.rs:113 for preapproved WebFetch URLs.

### [x] tools-web-mcp#54 — Cron + RemoteTrigger tools registered and model-visible but backed only by NoOp… ✅ FIXED (2026-06-07 — firing wake-loop wired)
`✅ fixed` · **HIGH** · effort **small** · fix-sketch *done*

> **✅ FIRING SUBSYSTEM LANDED 2026-06-07 (commits `6c824a4d4b`, `989b3ebdb5`, `c258d88d51`, `56fb12bffb`).**
> Scheduled tasks now actually fire, mirroring TS `cronScheduler.ts` + `cronTasks.ts`, in a clean 3-layer
> architecture:
> - **`coco-cron`** owns the pure, I/O-free scheduler core (`CronTickState::tick` — first-sight anchoring,
>   recurring reschedule-from-now, one-shot/aged drop, eviction; `find_missed`; `RECURRING_MAX_AGE_MS`). 19 tests.
> - **`core/tool-runtime`** owns `CronTask` + the `ScheduleStore` trait (TS `cronTasks.ts` semantics) with a
>   `DiskBackedScheduleStore` (durable → `<cwd>/.coco/scheduled_tasks.json`, camelCase/LF via
>   coco-file-encoding; session tasks in-memory; corrupt/invalid rows dropped on read). 7 tests.
> - **`app/cli::cron_tick`** is the 1s tokio-interval driver: reads the store → drives the core → enqueues each
>   due task's prompt with **`QueueOrigin::Cron`**. The enqueue wakes the idle agent driver
>   (`tui_runner::run_agent_driver` selects on `command_queue().wait_for_change`), so the prompt runs as a turn;
>   mid-turn it drains at the next boundary. Startup `find_missed` → batched "ask first" notification (TS
>   `buildMissedTaskNotification`, injection-fenced). 3 tests.
> - Tools rewired to the new store; CronCreate persists (durable→disk) + honest "Scheduled …" copy + errorCode-2
>   reachability; CronList renders `cron_to_human`. **RemoteTrigger** = *remote execution* on Anthropic's CCR
>   backend → explicit struct-level DEFER doc, sanctioned non-goal, stays behind `Feature::AgentTriggersRemote`.
>
> **Surface scope:** TUI-only (headless `-p` is one-shot, SDK has no queue-drain pump — a fired prompt would
> have nobody to run it). Durable tasks created in those modes still persist and fire in a later interactive
> session. **Feature gate stays `AgentTriggers` default-OFF** (deliberate divergence from TS `isKairosCronEnabled`
> GA-true); it now genuinely WORKS when enabled. **Deferred refinements (documented in `cron_tick.rs`, not core
> fire-path parity):** cross-process lease lock (`cronTasksLock.ts`), chokidar file-watcher (the 1s tick re-reads
> every pass), jitter (`cronJitterConfig.ts`), and the missed-task AskUserQuestion variant (a batched
> notification is used instead).

> **✅ TIER A LANDED 2026-06-07 (commit `436318f90f`).** `CronCreateTool` no longer claims disk persistence
> it never does: `render_for_model` always emits honest session-only wording and warns the job will NOT fire
> (scheduling under development); `durable` field doc + `description()` updated; tests assert the honest copy.
> **✅ TIER B partially LANDED 2026-06-07 (commits `3e228fb170`, `02bae43e9c`).** The deterministic cron core
> is ported faithfully from TS `utils/cron.ts` into a new `coco-cron` crate (parse / next-run / human /
> reachability; 12 tests, incl. DOM-OR-DOW, 7=Sunday, Feb-30-unreachable). Wired into the tools: CronCreate
> now rejects unreachable expressions (TS errorCode-2 `nextCronRunMs` check), `is_valid_cron_expression`
> delegates to the range-aware parser, and CronList renders `cron_to_human`. **RemoteTrigger** carries an
> explicit struct-level DEFER doc — it is *remote execution* of scheduled agents on Anthropic's CCR backend
> (claude.ai OAuth + `/v1/code/triggers` + `ccr-triggers` beta), a sanctioned non-goal; stays behind
> `Feature::AgentTriggersRemote` (off).
>
> ~~**STILL DEFERRED — the firing subsystem**~~ **— SUPERSEDED: the firing wake-loop landed (see the
> FIRING SUBSYSTEM note above).** The idle-session wake reused the existing `tui_runner::run_agent_driver`
> `wait_for_change` arm (no new wake path needed). Only the non-core refinements remain (lock / watcher /
> jitter / missed-AskUserQuestion variant).

> **2026-06-07 DEEP VALIDATION — split into a do-now correctness fix + the deferred scheduler.**
> - **Production store is NOT NoOp — it's a wired `InMemoryScheduleStore`** (`session_runtime.rs:1348` →
>   `with_schedule_store`). The `is_enabled` gates (`Feature::AgentTriggers`, default-off) are
>   **user-overridable** (`settings.json features.agent_triggers=true` / `COCO_FEATURE_AGENT_TRIGGERS=true`).
>   So an opt-in user hits LIVE broken behavior — not dead code.
> - **The `durable` flag + success message are ACTIVELY DECEPTIVE.** `CronCreateTool::execute` never reads
>   `input.durable` for persistence, and `render_for_model` claims the job is **"Persisted to
>   `.coco/scheduled_tasks.json`"** — but it is never written, never fires (no tick loop), and is lost on
>   session exit. This is the real near-term hazard.
> - **TIER A — DO-NOW (trivial, independent of the scheduler):** in `scheduling.rs::CronCreateTool`, drop the
>   `durable` branch that claims disk persistence; always emit session-only wording ("under development; will
>   NOT fire"), and stop advertising firing/`durable` semantics in `CronCreateInput` docs/`description()` (or
>   hide `durable` from the model schema until persistence exists). Removes the lie with zero new subsystem.
>   Keep `AgentTriggers` default-off.
> - **TIER B — DEFER (correctly):** the real subsystem (extend `ScheduleEntry` with cron/created_at/
>   last_fired_at/recurring/permanent/durable; port `cron.ts` parse+next-run via the `cron`/`saffron` crate;
>   disk persistence to `.coco/scheduled_tasks.json`; `cronTasksLock.ts` O_EXCL lease; a 1s tokio-interval
>   wake loop pushing a `QueuedCommand` with a new `QueueOrigin::Cron` at the turn boundary; missed-task
>   recovery; next-year reachability validation). Only then flip the feature posture.
> - **Record as DIVERGENCE, not parity:** TS `isKairosCronEnabled()` defaults **true** (GA); coco defaults
>   `AgentTriggers` **off**. Defensible (no real scheduler ⇒ correctly hidden) but it's a deliberate divergence.
> - **RemoteTrigger** stays a sanctioned non-goal (claude.ai OAuth + Anthropic-internal endpoint).

- **Gap:** Cron/RemoteTrigger tools are now gated from model visibility via is_enabled Feature checks (fixing parity divergence), but NoOp backend remains unimplemented; recommend P2 for real ScheduleStore implementation when feature is enabled
- **TS:** tools.ts:29-35 registers the three cron tools only behind feature('AGENT_TRIGGERS'); each tool's isEnabled() returns isKairosCronEnabled() (prompt.ts:36-45, GrowthBook default true / GA). The cron engine is fully present in the external tree (utils/cronTasks.ts addCronTask/listAllCronTasks, cronScheduler.ts, cron.ts, bootstrap/state setScheduledTasksEnabled…
- **Rust (HEAD):** coco-rs/core/tools/src/tools/scheduling.rs:167-169 (CronCreateTool.is_enabled), :336-338 (CronDeleteTool.is_enabled), :445-447 (CronListTool.is_enabled), :598-600 (RemoteTriggerTool.is_enabled); coco-rs/core/tool-runtime/src/registry.rs:209,240 (both loaded_tools and deferred_tools filter via passes_filter_pipeline which calls tool.is_enabled at line 45); c…
- **Fix:** Gate the cron tools' is_enabled behind a settings flag (mirror isKairosCronEnabled) and provide a real disk/in-memory ScheduleStore (port utils/cronTasks.ts), or filter them from the registry until a real store is injected. Keep RemoteTrigger gated off (sanctioned).

### [x] mcp#144 — headersHelper dynamic-headers script support absent
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound*

- **Gap:** Headers helper dynamic-headers script support (headersHelper.ts execution with CLAUDE_CODE_MCP_SERVER_NAME/_URL env injection) not ported; config structs and parsing only support static headers
- **TS:** headersHelper.ts:getMcpHeadersFromHelper executes config.headersHelper (shell, 10s timeout) with env CLAUDE_CODE_MCP_SERVER_NAME/_URL injected, requires workspace trust for project/local scope, parses stdout JSON, validates all values are strings; getMcpServerHeaders merges dynamic over static headers.
- **Rust (HEAD):** coco-rs/services/mcp/src/types.rs:57-73 (McpSseConfig and McpHttpConfig have only url/headers/oauth fields, no headers_helper); coco-rs/services/mcp/src/config.rs:227-229 (parse_headers reads static headers only); grep -r 'headers_helper' in coco-rs returns no results
- **Fix:** Add optional headers_helper to the remote config structs; on connect, exec the helper (10s timeout, server name/url env), parse+validate JSON string map, gate on trust for project/local scope, and merge over static headers before transport init.

### [x] tools-web-mcp#55 — WebFetch does not upgrade http:// to https:// despite advertising it
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** WebFetch advertises http->https upgrade but never performs it; url passed unchanged to fetch
- **TS:** utils.ts:376-378 — when parsedUrl.protocol === 'http:' it sets parsedUrl.protocol='https:' and uses upgradedUrl for the fetch (utils.ts:417).
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/web.rs:810-840 shows url scheme validation but NO http->https upgrade logic. Line 686 still advertises 'HTTP URLs will be automatically upgraded to HTTPS' despite no implementation. fetch_url at lines 1077+ accepts http:// unchanged.
- **Fix:** Before the fetch loop in fetch_url (or in execute after parse), if parsed.scheme()=='http', set scheme to 'https' and use the upgraded URL string as the initial current_url.

### [x] tools-web-mcp#57 — Binary content (PDF/octet-stream) rejected instead of persisted-and-summarized
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound*

- **Gap:** Binary content (PDF/octet-stream) hard-rejected instead of persisted-and-summarized like TS
- **TS:** utils.ts:435-449 persists binary bodies to disk (persistBinaryContent with a mime-derived extension), still UTF-8 decodes and runs the prompt; WebFetchTool.ts:280-285 appends '[Binary content (...) also saved to <path>]' to the result.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/web.rs:1170-1179 hard-rejects binary content (image/, audio/, video/, application/octet-stream, application/zip) with ExecutionFailed error. Comment at 1167-1169 self-admits 'out of scope for now'. No persist-to-disk path exists anywhere in coco-rs tree.
- **Fix:** On binary content-type, persist the raw bytes to a temp file with a mime extension, decode lossily, run the side-query, and append the '[Binary content ... also saved to <path>]' note instead of erroring.

### [x] mcp#151 — ElicitationComplete notification dismisses any prompt instead of matching elici…
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** ElicitationComplete notification dismisses any active prompt unconditionally instead of matching (mcp_server_name, elicitation_id) against queued elicitations; payload fields are discarded
- **TS:** elicitationHandler.ts:175-206 matches the completion's elicitationId against the queued URL elicitation via findElicitationInQueue(serverName, elicitationId), sets completed:true on only that queue entry, and logs+ignores unknown IDs.
- **Rust (HEAD):** coco-rs/app/tui/src/server_notification_handler/protocol.rs:855-858 (ServerNotification::ElicitationComplete(_) => { state.ui.dismiss_prompt(); true } unconditionally discards the payload); coco-rs/common/types/src/event.rs:869-872 (ElicitationCompleteParams carries mcp_server_name and elicitation_id but both are ignored)
- **Fix:** Match the notification's (mcp_server_name, elicitation_id) against the queued elicitation prompt and dismiss/complete only that entry; ignore unknown ids.

### [x] mcp#154 — truncate_description slices on byte index, can panic on multibyte UTF-8
`● genuinely_open` · **LOW** · effort **trivial** · fix-sketch *sound*

- **Gap:** truncate_description slices on byte index, can panic on multibyte UTF-8 at char boundaries; unsafe byte slicing used instead of char-boundary-safe truncation
- **TS:** client.ts:1791-1793 desc.length > MAX ? desc.slice(0, MAX) + '… [truncated]' — JS .slice over UTF-16 code units never panics.
- **Rust (HEAD):** coco-rs/services/mcp/src/tool_call.rs:34-40 and 43-51 (both truncate_description and prepare_tool_for_llm use byte-index slice &description[..MAX_DESCRIPTION_LENGTH] without char boundary checking); coco-rs/services/mcp/src/discovery.rs:360 calls truncate_description on raw server tool descriptions (production path); tests in tool_call.test.rs:13,19 only us…
- **Fix:** Use a char-boundary-safe truncation (e.g. floor_char_boundary or chars().take(MAX)) instead of &s[..MAX]; centralize one helper used by both call sites.

### other (commands · coordinator · hooks · plugins · query · sandbox · shell · skills · tasks · inference) (38)

### [x] permissions#68 — Classifier transcript includes assistant-authored text (TS excludes it for prom…
`◑ partially_fixed` · **HIGH** · effort **small** · fix-sketch *sound*

- **Gap:** Doc overstated findings: sub-claims 1-2 already fixed (assistant text dropped, no tool-result synthesis), but sub-claim 3 remains: 10-entry cap should be removed to match TS full-transcript spec."
- **TS:** yoloClassifier.ts buildTranscriptEntries (302-360): for assistant messages, ONLY tool_use blocks are kept (341-353) with the explicit comment 'Only include tool_use blocks — assistant text is model-authored and could be crafted to influence the classifier's decision.' It does not synthesize ToolResult entries and applies no fixed N-entry cap (the full trans…
- **Rust (HEAD):** coco-rs/core/permissions/src/classifier.rs:667-670 (AssistantContent::Text explicitly dropped with comment), lines 241-244 (Tool results catchall with comment), lines 575-596 (format_transcript uses saturating_sub(10) to cap to 10 entries); TS /lyz/codespace/3rd/claude-code/src/utils/permissions/yoloClassifier.ts:341-360 (buildTranscriptEntries returns full…
- **Fix:** In extract_assistant_blocks, drop the AssistantContent::Text arm (keep only ToolCall). Remove the Message::ToolResult => TranscriptBlock::ToolResult synthesis. Remove the `.take(10)` cap in format_transcript (rely on upstream compaction for bounding).

### [x] permissions#70 — Denial-limit circuit breaker silently falls through instead of prompting the us… ✅ VERIFIED RESOLVED (2026-06-07 — 4/4, not 3/4)
`✅ fixed` · **HIGH** · effort **small** · fix-sketch *done*

> **2026-06-07 audit (tracker was WRONG — claimed headless abort missing):** all 4 TS behaviors are
> present. When `avoid_permission_prompts` is set, the classifier denial returns
> `PermissionDecision::Abort { reason: PermissionAbortReason::ClassifierDenialLimit }`
> (`core/permissions/src/auto_mode_decision.rs:202`; variant `common/types/src/permission.rs:282`), wired
> through `PermissionOutcome::Aborted` → `cancel.cancel()` / `cancelled=true` on BOTH the batched
> (`engine_tool_execution.rs`) and streaming (`engine_stream_consume.rs`) paths. Test
> `test_denial_limit_headless_aborts` (`auto_mode_decision.test.rs:358`) locks it. It does NOT return Deny.

- **Gap:** Denial-limit circuit breaker implemented with 3/4 TS behaviors: total>=20 trip, warning message, and total-cap reset work. Headless abort (throw AbortError) is missing—Rust returns Deny instead, completing tool call with error rather than aborting the session.
- **TS:** permissions.ts handleDenialLimitExceeded (984-1058): when shouldFallbackToPrompting (denialTracking.ts:40-45 = consecutive>=3 || total>=20), it converts the deny into an 'ask' carrying a warning ('N consecutive/total actions were blocked. Please review the transcript'); throws AbortError in headless; and resets totalDenials+consecutiveDenials to 0 when the …
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/permissions/src/auto_mode_decision.rs:56-59 (should_fallback_to_prompting check), lines 172-191 (denial limit handling with warning), lines 177-182 (total cap reset); /lyz/codespace/codex/coco-rs/core/tool-runtime/src/denial_tracking.rs:56-59 (should_fallback_to_prompting), lines 63-74 (hit_total_limit and reset_after_total…
- **Fix:** After a classifier denial, record then check shouldFallbackToPrompting (consecutive>=3 OR total>=20); convert the Deny into an Ask with a review warning, abort in headless (shouldAvoidPermissionPrompts), and reset both counters when the total cap is hit. Move the limit check to post-denial rather than entry.

### [x] skills#193 — ${CLAUDE_SKILL_DIR} and ${CLAUDE_SESSION_ID} placeholders are never substituted
`◑ partially_fixed` · **HIGH** · effort **small** · fix-sketch *sound_with_caveats*

- **Gap:** Model-invoked skills now correctly substitute ${CLAUDE_SKILL_DIR}/${CLAUDE_SESSION_ID} via expand_skill_prompt (fixed in commit 92054f73a), but user-typed slash commands still do not via SkillPromptHandler which uses substitute_arguments — fix is small: capture skill.skill_root and session_id in handler struct and apply expand_skill_prompt instead
- **TS:** loadSkillsDir.ts:356-369 getPromptForCommand replaces /\$\{CLAUDE_SKILL_DIR\}/g with the skill's baseDir (win32 backslash-normalized) and /\$\{CLAUDE_SESSION_ID\}/g with getSessionId() on every invocation.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/app/query/src/skill_runtime.rs:187-203 (expand_skill_prompt called with skill_dir and session_id set for model-invoked path); /lyz/codespace/codex/coco-rs/commands/src/lib.rs:657-662 (SkillPromptHandler still using substitute_arguments which does not handle CLAUDE_SKILL_DIR/CLAUDE_SESSION_ID for user-typed slash path)
- **Fix:** Thread skill.skill_root and the session id into the expansion call (use expand_skill_prompt with skill_dir/session_id set, or add the two replacements to the live substitute_arguments path).

### [x] skills#194 — Skill change watcher is defined but never instantiated by the app - no hot relo…
`◑ partially_fixed` · **HIGH** · effort **small** · fix-sketch *sound_with_caveats*

- **Gap:** Skill watcher is now instantiated and hot-reloads catalog (fixing primary gap), but ConfigChangeHooks gate is missing—skill reloads proceed without checking for blocking hook results as TS does.
- **TS:** skillChangeDetector.ts:85-141 initialize() runs chokidar.watch over user/project skills+commands dirs and --add-dir paths with awaitWriteFinish (1s stabilityThreshold) and .git ignore; scheduleReload (255-279) fires executeConfigChangeHooks('skills',...) (aborting on a blocking result), clearSkillCaches, resetSentSkillNames, emit. Registered at startup via …
- **Rust (HEAD):** coco-rs/app/cli/src/skill_watch.rs:52-85 (spawn function instantiates and subscribes to SkillChangeDetector); coco-rs/app/cli/src/tui_runner.rs:348 (calls skill_watch::spawn); coco-rs/skills/src/watcher.rs:134-200 (SkillChangeDetector::new creates the watcher and spawns reload task). Watcher does reload_disk_skills and rebuild command registry, but does NOT…
- **Fix:** Instantiate SkillChangeDetector at session bootstrap over the resolved scoped dirs (+ --add-dir), subscribe to drive self.skill_manager.reload_disk_skills + command-registry rebuild; add stability threshold, .git ignore, and ConfigChange-hook gate.

### [x] tools-agent-task#44 — Standalone EnterWorktree/ExitWorktree tools use a different schema and skip all…
`◑ partially_fixed` · **HIGH** · effort **small** · fix-sketch *sound_with_caveats*

- **Gap:** Schema and session-state switching fixed in 92054f73a; outstanding gap: EnterWorktreeTool missing guard for already-active worktree (loses original_cwd)
- **TS:** EnterWorktreeTool.ts:24-127 takes optional validated `name` slug (validateWorktreeSlug), defaults to getPlanSlug(), guards getCurrentWorktreeSession() ('Already in a worktree session'), resolves to canonical git root, routes through createWorktreeForSession() (session record + symlinks), then chdir's + setOriginalCwd + saveWorktreeState + clears caches. Exi…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/worktree.rs:28-34 (EnterWorktreeInput now has {name: Option<String>}), 209-228 (ExitWorktreeInput has {action, discard_changes}), 87-198 (EnterWorktree::execute performs std::env::set_current_dir + app_state mutation), 285-427 (ExitWorktree::execute has change-counting gate at 314-350 and discard_changes che…
- **Fix:** Rework EnterWorktreeInput to {name:Option<String>} with slug validation + 'already in session' guard + session-state switch; rework ExitWorktreeInput to {action:keep|remove, discard_changes:Option<bool>} with validate_input change-counting and branch/tmux teardown. Session-state switch (chdir/originalCwd/cache clear) needs a query-engine cleanup-hook channe…

### [x] commands#207 — /config is a text read/write handler, not the TS local-jsx config panel
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound*

- **Gap:** /config returns flat key/value text (effective next-session) instead of TS interactive Settings panel with live-apply; text fallback exists so degraded UX not breakage
- **TS:** commands/config/index.ts is type:'local-jsx' 'Open config panel' -> config.tsx renders <Settings ... defaultTab="Config"/>, an interactive browse/toggle panel that applies many changes live.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/commands/src/implementations.rs:268-275 /config registered with config_extended_handler (text handler, BuiltinCommand); tui_runner.rs:2502-2591 dispatch_slash_command does not intercept config; config falls through to text handler emit_slash_text. No DialogSpec or interactive overlay wired.
- **Fix:** Intercept name=="config" in tui_runner and open a real settings overlay (analogous to the agents/model dialogs), or at minimum apply writes to the live RuntimeConfig hot-reload path instead of only settings.json next-session.

### [x] commands#208 — /plugin returns a text listing instead of opening the TS interactive plugin pic… ✅ VERIFIED RESOLVED (2026-06-07 — residual: 4th "Discover" tab)
`✅ fixed` · **MEDI** · effort **medium** · fix-sketch *done*

> **2026-06-07 audit (tracker + commands/CLAUDE.md were stale):** a real interactive overlay exists.
> `DialogSpec::PluginPicker` → `refresh_plugin_dialog_payload` (`app/cli/src/tui_runner.rs:3302`) opens
> `PluginDialogState` (`app/tui/src/state/surface_payloads.rs:1088`, tabs `Installed`/`Marketplace`/`Error`)
> rendered by `app/tui/src/presentation/picker.rs` (`render_plugin_tabs`/`render_installed_tab`/
> `render_marketplace_tab`) with live refresh. The old `DialogPending` info-message path is now the
> `unreachable!()` arm for PluginPicker. **Residual (minor):** TS has a 4th "Discover" tab
> (`DiscoverPlugins.tsx`: in-overlay marketplace browse + install-count + one-keystroke install); Rust has
> 3 tabs and routes discovery through text `/plugin search` + `/plugin install`. Delete the stale
> PluginPicker bullet from `commands/CLAUDE.md`.

- **Gap:** /plugin returns text listing instead of interactive plugin/marketplace picker; text subcommands remain functional so degraded UX not breakage
- **TS:** commands/plugin/index.tsx is type:'local-jsx', immediate:true (sibling ManagePlugins.tsx / DiscoverPlugins.tsx / BrowseMarketplace.tsx confirm the Ink picker) -> opens an interactive plugin/marketplace browser with install/enable/disable.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/commands/src/implementations.rs:520-528 /plugin registered with handlers::plugin::handler (text); tui_runner.rs:2817-2837 DialogSpec::PluginPicker maps to DialogPending (info message only); no real plugin picker overlay wired in app/tui/src
- **Fix:** Wire the DialogSpec::PluginPicker arm in tui_runner to a real coco-tui overlay (ManagePlugins/DiscoverPlugins equivalent), and register /plugin to emit DialogSpec::PluginPicker on no-arg invocation.

### [x] coordinator#258 — Tmux external swarm session/window is not reused across processes
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound*

- **Gap:** External swarm session unconditionally recreated; no has-session/list-windows probes for reuse
- **TS:** TmuxBackend.ts:466-546 createExternalSwarmSession probes `has-session` and `list-windows`; if the session+swarm-view window exist it reuses them (list-panes -> paneId = panes[0]); for the first teammate it reuses firstPaneId (paneId = firstPaneId) rather than splitting.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/coordinator/src/pane/tmux.rs:321-342 create_teammate_pane_external runs new-session -d -s SWARM unconditionally with no has-session guard; no list-windows or list-panes probes anywhere in the external path
- **Fix:** Add a has-session + list-windows probe in create_teammate_pane_external; reuse the existing swarm-view window's first pane (list-panes -> panes[0]) for the first teammate instead of unconditional new-session + split.

### [x] coordinator#265 — External (outside-tmux) teammate panes get no border color, no pane-border-stat…
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** External teammate panes skip border color, pane-border-status, and rebalance operations
- **TS:** TmuxBackend.ts:644-693 createTeammatePaneExternal calls setPaneBorderColor(paneId, color, true), setPaneTitle(paneId, name, color, true), enablePaneBorderStatus(windowTarget, true) (first teammate), and rebalancePanesTiled(windowTarget) after every external pane.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/coordinator/src/pane/tmux.rs:311-367 create_teammate_pane_external: _color parameter unused (line 314), only sets title via select-pane (line 361); no set_pane_border_color call, no enable_pane_border_status call, no rebalance call. Contrast in-leader path (lines 276-306) which calls set_pane_border_color (296), set_pane_title (…
- **Fix:** In create_teammate_pane_external, after creating the pane call set_pane_border_color, enable_pane_border_status(window) on first teammate, set_pane_title with color, and rebalance_panes_tiled(window) — mirroring the leader path (will need socket-aware variants for the external session).

### [x] hooks#184 — asyncRewake exit-code-2 does not enqueue a task-notification to wake the model ✅ VERIFIED RESOLVED (2026-06-07)
`✅ fixed` · **MEDI** · effort **medium** · fix-sketch *done*

> **2026-06-07 audit (tracker CONFLATED two things):** the wake path does NOT use `rewake_requested` —
> TS bypasses the registry (`utils/hooks.ts:206`) and coco mirrors that via `spawn_rewake_command`
> (`hooks/src/lib.rs:1142,1305`) → `AsyncRewakeSink` (`lib.rs:173`), impl'd by
> `CommandQueueNotificationSink` (`app/cli/src/command_queue_sink.rs:77`) and wired with live (non-None)
> sinks across the `OrchestrationContext` sites (`session_runtime.rs:106`). The `rewake_requested` field
> the tracker grepped is a vestigial struct snapshot with no behavioral consumer (candidate for deletion).

- **Gap:** asyncRewake exit-code-2 marks rewake flag but never wakes idle session; missing CommandQueue::TaskNotification enqueue into SessionRuntime
- **TS:** utils/hooks.ts:205-245 executeInBackground — asyncRewake hooks, on exit code 2, call enqueuePendingNotification({value: wrapInSystemReminder(...), mode:'task-notification'}) which wakes an idle model via the queue processor or injects mid-query.
- **Rust (HEAD):** coco-rs/hooks/src/orchestration.rs:894-901 calls reg.mark_rewake() on async_rewake+exit-code-2, but grep -rn 'rewake_requested' app/ finds zero consumers in app/query or app/cli; async_registry.rs:110-113 sets the flag but reminder_source.rs:76-92 to_hook_event() never checks it
- **Fix:** On rewake_requested completion, push a TaskNotification-origin QueuedCommand (system-reminder-wrapped blocking text) into the SessionRuntime CommandQueue and signal the idle-session wake path, mirroring enqueuePendingNotification(mode:'task-notification').

### [x] hooks#185 — Async/background execution not detected from hook's stdout {"async":true} ✅ VERIFIED RESOLVED (2026-06-07 — residual: forceSyncExecution only)
`✅ fixed` · **MEDI** · effort **medium** · fix-sketch *done*

> **2026-06-07 audit (tracker's central claims were FABRICATED):** there is no `wait_with_output()` —
> `hooks/src/lib.rs` uses `BufReader::read_line` + `read_to_string` + `child.wait()`. The dynamic first-line
> `{"async":true}` detection IS implemented (`first_line_is_async`, `lib.rs:1226`, consumed at `lib.rs:1177`)
> and reachable alongside the static `forced_async` config flag. Feature is DONE. **Sole residual gap** (small,
> low value, tracked here): no `forceSyncExecution` override — async-declaring setup/shutdown hooks detach to
> the registry and are lost on immediate exit; TS passes `forceSyncExecution:true` for those paths. Add
> `force_sync` to `AsyncCommandOptions` and short-circuit the three background branches.

- **Gap:** Dynamic stdout {'async':true} detection missing; only static config flag checked
- **TS:** utils/hooks.ts:1117-1164 — on the first stdout line, isAsyncHookJSONOutput(parsed) backgrounds the hook dynamically via executeInBackground even when hook.async was unset (forceSyncExecution overrides).
- **Rust (HEAD):** coco-rs/hooks/src/lib.rs:955-1012 executes commands with wait_with_output() blocking call, no streaming or first-line parse; orchestration.rs:679-680 checks is_async from config only, never stdout; async_registry.rs:6 comment documents TS behavior but runtime detection is unwired
- **Fix:** In the Command-hook runner, peek the first stdout line; if it parses as {"async":true}, hand the running process to the async_registry (background + register for reminder-pipeline delivery) instead of awaiting completion.

### [x] hooks#190 — Malformed/invalid hook JSON is silently injected as additional context instead …
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Valid-JSON-but-schema-mismatch silently injected as context instead of surfaced as validation error
- **TS:** utils/hooks.ts:399-451 parseHookOutput returns { plainText, validationError } when JSON starts with '{' but fails hookJSONOutputSchema; the result loop checks `if (validationError)` FIRST (hooks.ts:2504-2531) and yields a hook_non_blocking_error attachment, returning WITHOUT injecting plainText as context.
- **Rust (HEAD):** coco-rs/hooks/src/orchestration.rs:171-182 parse_hook_output has no ValidationError variant, only Json|PlainText; lines 1114-1118 PlainText (including malformed JSON) directly pushed to additional_contexts with no validation error emission; TS at utils/hooks.ts:382-449 distinguishes parse-fail vs schema-fail and yields hook_non_blocking_error on schema-mism…
- **Fix:** Add a ParsedHookOutput::ValidationError(String) variant returned when trimmed starts with '{' but from_str fails; in aggregation surface it as a hook_non_blocking_error and do NOT push it to additional_contexts.

### [x] inference#133 — No foreground/background query-source gating for 529 retries (capacity-cascade …
`● genuinely_open` · **MEDI** · effort **medium** · fix-sketch *sound*

- **Gap:** Query source not gated on 529 capacity retries; all sources treated equally.
- **TS:** withRetry.ts:62-89,316-324: FOREGROUND_529_RETRY_SOURCES set + shouldRetry529() -- non-foreground sources (titles, suggestions, summaries) throw CannotRetryError immediately on 529 to avoid 3-10x gateway amplification during a capacity cascade; foreground sources retry.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/services/inference/src/retry.rs:84-101 — should_retry() has no query_source param; /lyz/codespace/codex/coco-rs/services/inference/src/client.rs:75,563,694 — QueryParams.query_source exists but never used in retry decision.
- **Fix:** Add a query_source -> foreground/background classification (set mirroring FOREGROUND_529_RETRY_SOURCES); thread query_source into should_retry (or a wrapper) so background sources return false immediately on Overloaded/RateLimited.

### [x] inference#134 — Default retry count/base-delay diverge from TS (3/1000ms vs 10/500ms); no max-r…
`◑ partially_fixed` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Max delay diverges (60s vs 40s); no env override for max retries.
- **TS:** withRetry.ts:52-55 DEFAULT_MAX_RETRIES=10, BASE_DELAY_MS=500; getRetryDelay maxDelayMs=32000; getDefaultMaxRetries (789-794) honors CLAUDE_CODE_MAX_RETRIES env override.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/common/config/src/sections.rs:28-30 — defaults match TS (10/500) but max_delay_ms=60_000 vs TS 32_000; no COCO_API_MAX_RETRIES env override in env.rs.
- **Fix:** Add EnvKey::CocoApiMaxRetries (e.g. COCO_API_MAX_RETRIES) and fold it into ApiRetryConfig resolution as an override layer over settings.json api.retry, mirroring getDefaultMaxRetries.

### [~] plugins#234 — MCPB config validation only checks 'required'; no type/range checks or sensitiv… ◑ PARTIAL (Stage 4, 2026-06-06)
`◑ partially_fixed` · **MEDI** · effort **large** · fix-sketch *sound*

> **✅ SECURITY HARDENING LANDED 2026-06-07 (commit `6551d090fe`).** `merge_env` no longer exposes every
> string user_config value as its own env var (a coco-only divergence that risked leaking sensitive values
> into the MCP child process env) — only manifest-declared env is emitted, with `${user_config.X}`
> substitution (TS parity). **Slice A (wire the non-sensitive local `.mcpb` dispatch) and Slice B (the
> sensitive→keyring split) remain DEFERRED — they need a design decision:** there is no `pluginConfigs`
> persistence layer in `coco_config`, `plugins` has no `coco-keyring-store` dep, and the keyring's
> `service+account→String` shape must be mapped onto TS's flat `pluginSecrets` bucket. `mcpb.rs` stays
> caller-less until that subsystem is designed (and a real plugin ships a `.mcpb`/`.dxt` bundle).

> **2026-06-07 DEEP VALIDATION — "split done, just wire" UNDERSTATES it; net-new subsystem + a security bug.**
> Confirmed dead: `load_mcpb` has zero production callers; `mcp_bridge::merge_manifest_value` has no
> `.mcpb/.dxt` branch, so a manifest value `"server.mcpb"` is treated as a JSON path → `serde_json::from_str`
> fails on ZIP bytes → server silently dropped. Hidden prerequisites the sketch missed:
> - **No `pluginConfigs[pluginId].mcpServers` read/write exists anywhere in `coco_config`** (grep = empty).
>   `load/saveMcpServerUserConfig` cannot be ported until that persistence layer (with bidirectional
>   scrub-via-delete, TS `settings.ts:349`) is built — a net-new subsystem, not wiring.
> - **`plugins` does not depend on `coco-keyring-store`**, and the store is `service+account→String` whereas
>   TS `pluginSecrets` is a flat JSON bucket under a composite `${pluginId}/${serverName}` key — needs a
>   deliberate serialization (store the JSON map as the value under a fixed account, keychain-first fail-closed).
> - **`load_mcpb` is NOT a drop-in for `loadMcpbFile`** — no URL-download branch, no `checkMcpbChanged` mtime
>   recheck, different placeholder engine. The bridge is **sync**; URL download forces an async refactor of
>   `merge_manifest_value`/`extract_mcp_servers_from_plugins` + its 3 call sites.
> - **SECURITY: `merge_env` is a coco-only divergence** — it injects every user_config string value as its
>   own env var, which risks leaking sensitive values into the child process env. Must be removed/guarded
>   (only substitute manifest-declared env).
>
> **Two-slice remediation:** **Slice A (smaller, do-first if a real plugin needs local `.mcpb`):** add the
> `.mcpb/.dxt` dispatch in `merge_manifest_value` for the **non-sensitive, local-file** path (read bytes →
> `mcpb::load_mcpb`; skip+log on `NeedsConfig`, TS parity), **fix the `merge_env` leak**, and thread
> `config_home` into the bridge (currently absent). **Slice B (defer — the actual security fix):** the
> sensitive→keyring split, gated on the `pluginConfigs` persistence layer + `coco-keyring-store` dep above.
> URL-download `.mcpb` is a third slice, defer until needed. **Net: correctly a large defer** (Slice A is the
> only do-now-able sliver, and only if local `.mcpb` loading is actually wanted).

- **Done (2026-06-06):** `mcpb::validate_config` now mirrors TS `validateUserConfig` — required + per-field type (string / string[] when `multiple` / number / boolean / file|directory) + numeric min/max with `title`-prefixed messages; `${user_config.X}` + `${__dirname}` substitution wired into command/args/env (`substitute_template`). 4 new tests. **Remaining:** sensitive→keyring split (needs a live `saveMcpServerUserConfig` install flow — the whole `mcpb.rs` module is still caller-less, so the keyring split has nothing to drive yet).
- **Gap:** Three sub-gaps confirmed: (1) validate_config skips type/range checks (TODO at line 216); (2) no sensitive->keyring split; (3) no ${user_config.X} substitution. load_mcpb has zero production callers (ported-but-unwired).
- **TS:** mcpbHandler.ts validateUserConfig:346-409 enforces type correctness (string vs array-of-strings vs number vs boolean vs file/directory path) and numeric min/max with per-field messages; saveMcpServerUserConfig splits schema[key].sensitive===true into secureStorage (keychain) vs settings.json; getMcpConfigForManifest performs ${user_config.X} substitution.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/plugins/src/mcpb.rs:203-221 validate_config reads only `prop.get("required")` line 210-211 with explicit TODO at line 216. No type/range/enum checks. No sensitive->keyring split (merge_env at line 223 only stringifies str, no special handling). No ${user_config.X} substitution in mcp_config build (line 125-129). load_mcpb is tes…
- **Fix:** Extend validate_config to enforce type + min/max per JSONSchema field with TS-shaped messages; split sensitive fields into a secure store (utils/keyring-store) and implement ${user_config.X} substitution in command/args/env when building mcp_config.

### [x] plugins#235 — Headless install, seed marketplaces, and delisting auto-uninstall not wired ✅ FIXED (2026-06-07 — headless + SDK wired)
`✅ fixed` · **MEDI** · effort **medium** · fix-sketch *done*

> **FIXED 2026-06-07.** Extracted the TUI startup block into
> `session_bootstrap::spawn_marketplace_startup(config_home)` (fire-and-forget: `ensure_official_marketplace`
> → `run_marketplace_startup` = seed→reconcile→delist) and called it from all three surfaces:
> `tui_runner.rs` (refactored to the helper, no behavior change), `headless.rs::run_chat_with_options`
> (after `load_session_plugins`), and `main.rs::run_sdk_mode` (after `plugin_watch::spawn`). So delisted-plugin
> enforcement + seed-marketplace registration now run for `--print`/`chat`/`review`/SDK, not just the TUI.
> Delisting core already covered by `marketplace.test.rs` (7 tests). `just quick-check` + `just test-crate
> coco-cli` green. **Note:** kept best-effort/background on all paths (TS awaits only under
> `CLAUDE_CODE_SYNC_PLUGIN_INSTALL`); CCR zip-cache remains a sanctioned skip.

- **Done (2026-06-06):** the **delisting sweep** is live — `marketplace::detect_and_uninstall_delisted_plugins(config_home)` (TS `detectAndUninstallDelistedPlugins`) diffs the installed ledger against each known marketplace's cached manifest, flags + uninstalls + persists, and skips unreadable manifests (never false-delists). This drives all 4 formerly-dead fns.
- **Done (2026-06-06, seed + reconcile):** `register_seed_marketplaces` (TS `registerSeedMarketplaces`) reads `COCO_PLUGIN_SEED_DIR` (PATH-delimited, env-free testable core), registers seed marketplaces into `known_marketplaces.json` with runtime-recomputed `install_location` + `auto_update=false`, first-seed-wins, idempotent. `reconcile_marketplaces` (TS `reconcileMarketplaces`) reads settings `extraKnownMarketplaces` (`get_declared_marketplaces`) and fetch+registers any declared marketplace missing/source-changed (best-effort, per-entry error-isolated). `run_marketplace_startup` chains seed → reconcile → delisting and is wired into the startup task after `ensure_official_marketplace`. 4 new tests. The implicit official marketplace stays owned by `ensure_official_marketplace` (retry/backoff). ~~plugins#235 effectively complete.~~ **CORRECTED 2026-06-07 — NOT complete; genuine do-now gap.** `run_marketplace_startup` (`plugins/src/marketplace.rs:1058`) has EXACTLY ONE caller, `app/cli/src/tui_runner.rs:454` — verified by repo-wide grep. So seed + reconcile + **delisting auto-uninstall** silently never run on the headless path (`app/cli/src/headless.rs::run_chat_with_options`, reached by `coco --print`/piped/`chat`/`review`) or the SDK path (`main.rs::run_sdk_mode`). TS treats headless plugin install/delist as first-class (`utils/plugins/headlessPluginInstall.ts` `installPluginsForHeadless`, invoked at `print.ts:1721`), so delisted-plugin enforcement is **functionally absent on every non-interactive surface** — not "minor tui-only coverage". **Fix (small, `app/cli`-only):** extract the `tui_runner.rs:444-463` block (`ensure_official_marketplace` → `run_marketplace_startup`) into `session_bootstrap::spawn_marketplace_startup(config_home, cwd)`; call it from `headless.rs::run_chat_with_options` (after `load_session_plugins`, before `bootstrap_session_mcp`) and `main.rs::run_sdk_mode` (near `plugin_watch::spawn`). Add a headless test asserting a seeded delisted plugin is uninstalled. CCR zip-cache stays a sanctioned skip.
- **Gap:** Delisting detection functions exist but are dead (zero production callers). No seed-marketplace support anywhere. No headless plugin-install entry point. Gap persists fully.
- **TS:** headlessPluginInstall.ts installPluginsForHeadless:43 registers seed marketplaces, reconciles declared marketplaces, syncs zip cache, and calls detectAndUninstallDelistedPlugins on startup; marketplaceManager.registerSeedMarketplaces:380 + pluginBlocklist.detectAndUninstallDelistedPlugins.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/plugins/src/marketplace.rs:585-656 defines detect_delisted_plugins, load_flagged_plugins, save_flagged_plugins, flag_delisted_plugin. Grep shows ZERO production callers outside marketplace.rs definition. /lyz/codespace/codex/coco-rs/app/cli/src/session_runtime.rs:2938-2956 reload_plugins_with loads plugins via load_from_dirs but…
- **Fix:** Add a startup entry point that registers seed marketplaces, runs reconcile_marketplaces, and calls detect_delisted_plugins → uninstall for each flagged id; invoke it from the headless/non-interactive bootstrap path.

### [x] plugins#239 — Layer-3 refresh loads only PLUGIN.toml dirs and merely counts contributions ✅ FIXED (2026-06-07 — SDK reload wired)
`✅ fixed` · **MEDI** · effort **large** · fix-sketch *done*

> **FIXED 2026-06-07.** SDK `handle_plugin_reload` (`sdk_server/handlers/runtime.rs`) is no longer a no-op:
> it fetches the live runtime via `ctx.state.session_runtime` and runs the TUI `/reload-plugins` chain
> (`reload_plugins` → `reload_agent_catalog` → `reload_lsp_servers` → `reload_hooks`), then returns the real
> `PluginReloadResult` (live `snapshot_for_ui()` command names, `current_agent_catalog().active()` agent
> names, `load_all_installed_plugins` plugin ids, hook-reload `error_count`). Falls back to the empty ack
> only when no `SessionRuntime` is wired (handler-level tests). Test renamed to
> `plugin_reload_without_session_runtime_returns_empty` (covers the fallback; the wired chain's `reload_*`
> methods are covered on `SessionRuntime`). `just quick-check` + `just test-crate coco-cli` green.
> **Note:** `mcp_reconnect_key()` getter stays vestigial (reconnect work is done inline) — cleanup, not a gap.

- **Correction:** the original sub-gap (1) is STALE — `reload_plugins_with` already uses the V2 `load_enabled_plugins` and the V2 loader already understands the `cache/{mkt}/{plugin}/{version}` layout (`loader::resolve_cache_path`); the plugin-V2 unification fixed it.
- **Done (2026-06-06):** (a) `/reload-plugins` (`run_reload_plugins`) now chains `reload_agent_catalog()` + `reload_hooks()` after the command/skill rebuild (TS `refreshActivePlugins` rebuilds all). (b) **Unified config-driven MCP across SDK/headless/TUI.** New `session_bootstrap::bootstrap_session_mcp(runtime, cwd, existing_manager)` — the single init all three entry points call: builds/reuses the manager, registers **config-file servers** (`McpConfigLoader::load` — was dead, zero callers) + plugin servers, attaches the manager (`attach_mcp_manager`) + an `McpManagerAdapter` handle, then **connects every server in the background** (concurrent, per-server error-isolated + 30s-timeboxed via `connect_and_register_mcp`) and **registers each connected server's tools** into the live `ToolRegistry` so they reach the model (`connect → collect_server_schemas → register_mcp_tools`), then a best-effort `sync_all` for MCP skills. Mirrors codex-rs (single session-owned manager, eager concurrent fault-isolated connect) + TS (shared connect funnel). The SDK path's ~80-line inline block is deleted and replaced by this call (reuses its `SdkServer` manager); TUI/headless pass `None`. `SessionRuntime` holds the manager + an `mcp_reconnect_key` (TS `pluginReconnectKey`); `reload_plugin_mcp_servers` now also connects+registers new servers. No UI: connect-time elicitations are declined (the SDK `mcp/setServers` client-bridge path is separate + untouched). Tests: `bootstrap_session_mcp_attaches_handle_and_manager_with_no_servers`, `reload_plugin_mcp_servers_noops_without_manager_then_bumps_key_when_attached`; SDK setServers/status + adapter tests still green. (c) **Tails closed (2026-06-06):** **LSP re-register** — `LspHandle::reload` (default no-op) + `LspManagerAdapter::reload`→`reload_and_prewarm`; `SessionRuntime::reload_lsp_servers`; `/reload-plugins` now refreshes LSP too. **MCP server removal on disable** — `McpConnectionManager::unregister_server` (drops config + connection) + `reload_plugin_mcp_servers` reconciles: `plugin:`-namespaced servers no longer enabled are unregistered + `deregister_mcp_server`'d (config-file servers untouched). **Headless connect race** — `bootstrap_session_mcp(await_connect)`: headless passes `true` (awaits the connect batch, TS-print parity, bounded by the 30s per-server timeout), TUI/SDK pass `false` (background). New tests: `unregister_server_drops_config_and_connection_state`. ~~plugins#239 is now effectively complete.~~ **CORRECTED 2026-06-07 — the TUI reload chain is complete, but the SDK reload path is NOT.** `bootstrap_session_mcp` unifies MCP *bootstrap* across SDK/headless/TUI (true), but SDK *reload* `handle_plugin_reload` (`app/cli/src/sdk_server/handlers/runtime.rs:323`) is a `_ctx` **no-op stub** that returns empty `PluginReloadResult` vecs and never calls `reload_plugins` — so an SDK client's `plugin/reload` reloads nothing (commands/agents/hooks/MCP/LSP all unchanged). **Fix (small):** change `_ctx`→`ctx`, fetch the live runtime via `ctx.state.session_runtime.read().await.clone()` (mirror siblings at `runtime.rs:255/295`), then run `runtime.reload_plugins(&cwd)` → `reload_agent_catalog` → `reload_lsp_servers` → `reload_hooks` (mirror `tui_runner.rs:3832-3843`) and populate `PluginReloadResult` from the live registry/catalog snapshots. Sub-gaps the tracker worried about are confirmed N/A: marketplace-cache-layout reload IS closed (V2 loader); no `clearAllCaches` mirror needed (no in-memory plugin cache in Rust); `mcp_reconnect_key()` getter is vestigial (work is done inline) — drop it or annotate.
- **Gap:** Two sub-gaps confirmed: (1) marketplace-installed plugins under cache/{mkt}/{plugin} are invisible to reload_plugins because it uses discover_plugins, not V2 PluginLoader; (2) reload only swaps CommandRegistry, omitting hooks/MCP/agents/LSP and no pluginReconnectKey bump.
- **TS:** refresh.ts refreshActivePlugins clears all caches, reloads via loadAllPlugins (which understands the marketplace cache layout), then rebuilds commands/agents/hooks/MCP via loadPluginCommands/loadPluginHooks/loadPluginMcpServers/getAgentDefinitions, bumps the MCP pluginReconnectKey, and calls reinitializeLspServerManager.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/app/cli/src/session_runtime.rs:2938-2956 reload_plugins_with calls PluginManager::load_from_dirs (plugins/src/lib.rs does discovery) which uses discover_plugins, NOT the V2 PluginLoader. Rebuild only CommandRegistry (line 2941), no hooks/MCP/agents/LSP. No MCP pluginReconnectKey bump. /lyz/codespace/codex/coco-rs/plugins/src/loa…
- **Fix:** Point the reload path at a loader that scans the marketplace cache layout (cache/{mkt}/{plugin}/{version}) and plugin.json, then re-register commands/hooks/agents/MCP and re-init LSP; surface real lsp_count instead of 0.

### [x] plugins#240 — Slash-command /plugin enable/disable use a separate disabled_plugins.json, divo…
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** /plugin enable|disable mutate orphaned disabled_plugins.json (bare name); loader ignores it and reads only settings.json enabled_plugins (name@marketplace). Disabling via slash command has no load-time effect.
- **TS:** pluginOperations.ts setPluginEnabledOp:573 writes settings.json enabledPlugins[pluginId]=enabled (snake/camel) as the single source of truth, resolving the full pluginId; installedPluginsManager keeps installed_plugins.json in sync with that map.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/commands/src/handlers/plugin.rs:471-548 enable_plugin/disable_plugin read+write disabled_plugins.json via disabled_plugins_path() at line 529. Keys are bare dir names (name.trim(), line 471). /lyz/codespace/codex/coco-rs/plugins/src/install.rs:359-387 write_enabled_plugins writes settings.json enabled_plugins keyed by full Plugi…
- **Fix:** Make /plugin enable|disable mutate settings.json enabled_plugins[pluginId] (resolve full name@marketplace id) like install does, and delete the orphaned disabled_plugins.json path — single source of truth matching TS.

### [x] query#3 — Later-priority task notifications drain every turn instead of only after a Slee…
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Sleep-tool gating for Later-priority drain is unimplemented; unconditional Later drain surfaces background task notifications every turn instead of only after Sleep tool
- **TS:** query.ts:1566-1578 `const sleepRan = toolUseBlocks.some(b => b.name === SLEEP_TOOL_NAME)`; `getCommandsByMaxPriority(sleepRan ? 'later' : 'next')`. 'later'-priority items (incl. background task-completion notifications) are only drained when a Sleep tool ran in the just-completed batch; otherwise the boundary drain caps at 'next'. Not feature-gated.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/app/query/src/engine_finalize_turn.rs:501-507 unconditionally passes QueuePriority::Later to drain_command_queue_into_history; grep for sleepRan/SLEEP_TOOL across app/query/src/ (engine.rs, engine_finalize_turn.rs, engine_loop_state.rs, engine_session.rs, session_runtime.rs, helpers.rs) finds zero matches
- **Fix:** Compute sleep_ran from the just-executed tool batch (any ToolName::Sleep) and pass `if sleep_ran { Later } else { Next }` to drain_command_queue_into_history at engine_finalize_turn.rs:505.

### [x] sandbox#177 — Sandbox hot-reload + bootstrap subscriber only wired on TUI path, not SDK/print…
`◑ partially_fixed` · **MEDI** · effort **medium** · fix-sketch *sound_with_caveats*

- **Gap:** spawn_sandbox_reload was wired on TUI path but not SDK/headless; sandbox_reload.rs now exists and is called from tui_runner.rs:317. However, SDK (run_sdk_mode) and headless (run_chat) paths build RuntimeConfig without RuntimeReloader, so spawn_sandbox_reload cannot be called there — the gap partially persists for those entry points.
- **TS:** sandbox-adapter.ts:759-792 initialize() runs for REPL and print/SDK alike; the wrappedCallback comment states it covers 'all code paths (REPL, print/SDK)', and settingsChangeDetector.subscribe re-runs convertToSandboxRuntimeConfig + updateConfig on every settings change regardless of entry point.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/app/cli/src/sandbox_reload.rs:43-68 (function definition); tui_runner.rs:316-322 (single call in TUI path); main.rs:276-278 (SDK path via run_sdk_mode), 283-293 (headless path via run_chat) — SDK and headless do NOT create RuntimeReloader/RuntimePublisher
- **Fix:** Call spawn_sandbox_reload(state, &publisher, cwd) from the SDK runner and headless bootstrap (wherever the RuntimePublisher is available), mirroring tui_runner.rs:296.

### [x] shell#162 — JQ_FILE_ARGUMENTS security check (TS id 3) is entirely absent from the bash gate
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** jq system() and file-flag attacks auto-pass as read-only instead of routing to approval gate like TS does
- **TS:** bashSecurity.ts:742-781 validateJqCommand returns behavior:'ask' for both `jq system(...)` (JQ_SYSTEM_FUNCTION=2) and jq dangerous file flags `-f`/`--from-file`/`--rawfile`/`--slurpfile`/`-L`/`--library-path` (JQ_FILE_ARGUMENTS=3); BASH_SECURITY_CHECK_IDS reserves JQ_FILE_ARGUMENTS=3.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/exec/shell/src/read_only.rs:236 hardcodes jq/yq as safe; /lyz/codespace/codex/coco-rs/exec/shell/src/security.rs has 5 checks (no jq validator); /lyz/codespace/3rd/claude-code/src/tools/BashTool/bashSecurity.ts:742-781 validateJqCommand exists for TS and routes to behavior:'ask'
- **Fix:** Add a jq validator to the bash gate (or wire JqDangerAnalyzer), and remove the blanket jq/yq always-safe entry in read_only.rs so jq with system()/file flags falls through to the approval gate.

### [x] skills#196 — Skill listing drops skills under budget pressure instead of truncating; live mo…
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Live model-facing skill listing applies neither 1% context budget nor 250-char per-entry cap; reminder_source explicitly declares this 'out of scope'
- **TS:** tools/SkillTool/prompt.ts:70-171 formatCommandsWithinBudget always keeps every command: it shrinks non-bundled descriptions to maxDescLen (display width via stringWidth) or goes names-only, and never truncates bundled. attachments.ts:2741 calls it on the same unannounced-skills delta the Rust listing() mirrors.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/skills/src/reminder_source.rs:87-96 (emits `- {name}: {full description}` with NO char budget); reminder_source.rs:6-9 doc comment states 'out of scope for the per-turn reminder path'
- **Fix:** Apply formatCommandsWithinBudget semantics in reminder_source.rs::listing() (char/width-capped descriptions, names-only fallback, bundled never truncated, never drop a skill); fix or delete the dead byte-slice in format_skill_entry.

### [x] tools-agent-task#46 — EnterWorktree branch name not validated against path traversal
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** EnterWorktree does not validate branch name against path traversal like TS does; sanitizes with lossy worktree_slug() instead of strict validator.
- **TS:** utils/worktree.ts:66-87 validateWorktreeSlug validates each '/'-separated segment against /^[a-zA-Z0-9._-]+$/, rejects '.'/'..' segments and >64 chars, and runs synchronously before any git/chdir side effect; wired via EnterWorktreeTool.ts:25-38 superRefine on the `name` field.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/worktree.rs:87-119 execute() has no validate_input override and calls worktree_slug() which sanitizes (converts / to -) but does NOT validate input.name against the TS spec (split by /, reject . and .., check [a-zA-Z0-9._-]+ per segment, reject >64 chars). worktree_slug (lines 430-466) is permissive: it acce…
- **Fix:** Validate the branch/slug input through the existing coordinator validate_slug logic (or a shared helper) — reject `.`/`..` segments and >64 chars before constructing the path / invoking git.

### [x] tools-agent-task#48 — BriefTool accepts non-existent / inaccessible attachment paths instead of rejec…
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** BriefTool silently accepts non-existent or inaccessible attachment paths instead of rejecting like TS does; model receives false success confirmation.
- **TS:** BriefTool.ts:163-168 validateInput delegates to validateAttachmentPaths (tools/BriefTool/attachments.ts:26-61), which hard-fails (result:false, errorCode 1) on ENOENT ('...does not exist. Current working directory: <cwd>.'), not-a-regular-file, or EACCES/EPERM, so the model self-corrects the path before delivery.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/brief.rs:78-204: impl Tool for BriefTool has no validate_input override (implicit default no-op). execute() at lines 165-167 calls tokio::fs::metadata(&path).await and records exists: meta.is_ok(), size: meta.as_ref().map(...).unwrap_or(0) but DOES NOT reject on ENOENT/EACCES/not-a-regular-file. render_for_m…
- **Fix:** Add a validate_input override (or in-execute pre-check) that stats each resolved attachment and returns InvalidInput on ENOENT/not-a-file/EACCES, surfacing the resolve_root cwd in the ENOENT message like TS.

### [x] tools-exec#34 — Output truncation keeps head+tail instead of TS head-only, with a different mar…
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** Output truncation keeps head+tail instead of TS head-only, with marker text divergence (chars vs lines)
- **TS:** BashTool/utils.ts:133-163 formatOutput keeps HEAD only: `content.slice(0, maxOutputLength)` + `\n\n... [${remainingLines} lines truncated] ...`. EndTruncatingAccumulator (stringUtils.ts:138-175) also keeps the beginning and drops the end. Both TS paths are head-only with a 'lines truncated' marker.
- **Rust (HEAD):** bash.rs:1440 `format!("{first}\n... [{truncated_count} chars truncated] ...\n{last}")` confirms head+tail truncation with 'chars truncated' marker. Doc claims this diverges from TS head-only 'lines truncated' marker.
- **Fix:** Change truncate_output to head-only: keep first max_bytes (char-boundary snapped) and append `\n\n... [N lines truncated] ...` counting newlines after the cut, matching formatOutput.

### [x] tools-exec#38 — PowerShell background path does not apply sandbox wrapping
`● genuinely_open` · **MEDI** · effort **small** · fix-sketch *sound*

- **Gap:** PowerShell background path bypasses sandbox wrapping applied in foreground
- **TS:** PowerShellTool.tsx:746-750 computes shouldUseSandbox(command, dangerouslyDisableSandbox) once (Windows → false, else uniform) and threads it into the shared exec/shellCommand; spawnBackgroundTask (PowerShellTool.tsx:767+) flips that same already-sandbox-wrapped command to background, so backgrounded pwsh is sandboxed identically to foreground.
- **Rust (HEAD):** powershell_tool.rs:312-313 returns early from execute_background BEFORE sandbox resolution at :319-324. execute_background at :376-377 hardcodes `sandbox_state: None, sandbox_bypass: SandboxBypass::No` with comment 'W6: PowerShell bg path currently doesn't thread sandbox state'.
- **Fix:** Resolve sandbox_state/sandbox_bypass before the run_in_background branch and pass them into the BackgroundShellRequest in execute_background, matching the foreground path.

### [x] commands#209 — /compact ignores COCO_COMPACT_DISABLE for command visibility (TS hides it via i…
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** /compact ignored COCO_COMPACT_DISABLE for command visibility (stays in typeahead); should hide when env disables it
- **TS:** commands/compact/index.ts:9 isEnabled: () => !isEnvTruthy(process.env.DISABLE_COMPACT). getCommands (commands.ts:484) filters out non-enabled commands, and getCommand/findCommand only see the filtered list, so with the env set /compact is both invisible AND non-invocable (Command not found).
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/commands/src/implementations.rs:390-397 /compact registered with is_enabled=None (defaults to true); dispatch_slash_command at tui_runner.rs:2560 does not check disabled_by_env before TriggerCompact; visible() at lib.rs:265 passes since is_enabled=None; with COCO_COMPACT_DISABLE=1 the command still appears in typeahead/help
- **Fix:** Gate the COMPACT registration with is_enabled: Some(|| !compact.auto.disabled_by_env) (mirror /dream and /summary feature-gating), or check disabled_by_env at run_manual_compact entry and skip with a user-facing 'compaction disabled' message.

### [x] commands#210 — COCO_COMPACT_DISABLE hard-kill is not enforced on the manual /compact path
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** COCO_COMPACT_DISABLE hard-kill not enforced on manual /compact; user can still invoke full LLM compaction when env disables it
- **TS:** TS has no separate 'manual path checks DISABLE_COMPACT'; the isEnabled gate (index.ts:9) removes /compact from the registry entirely when the env is set, so the command cannot run at all -- the env effectively disables both auto and manual compaction.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/app/query/src/engine_compaction.rs:92-103 run_manual_compact has no disabled_by_env check; every production path (tui_runner.rs:2560, sdk_runner.rs:207) routes to run_manual_compact without gating; TS commands/compact/index.ts:9 isEnabled gate removes /compact entirely when DISABLE_COMPACT set
- **Fix:** In run_manual_compact (engine_compaction.rs:64), early-return CompactOutcome::Skipped with an emit_manual_compaction_failed('compaction disabled') when self.config.compact.auto.disabled_by_env, so the manual path honors the hard kill.

### [x] inference#136 — Backoff jitter is a fixed +25%, not randomized (no thundering-herd mitigation)
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** Backoff jitter is fixed +25%, not randomized in [0, 25%].
- **TS:** withRetry.ts:546 `const jitter = Math.random() * 0.25 * baseDelay` -- random jitter uniformly in [0, 25%] so concurrent clients de-correlate retries.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/services/inference/src/retry.rs:68-70 — fixed jitter (delay*0.25 added deterministically); TS /lyz/codespace/3rd/claude-code/src/services/api/withRetry.ts:546 — random jitter (Math.random()*0.25*baseDelay); no rand crate in Cargo.toml.
- **Fix:** Multiply jitter_factor by a uniform random in [0,1) (e.g. via fastrand/rand) so jitter is random in [0, jitter_factor*delay], matching TS Math.random()*0.25*baseDelay.

### [x] inference#137 — Blocking retry loop does not check for cancellation between attempts ✅ FIXED (feat/review — Wave 2 `QueryParams.cancel` + interruptible sleep; 2026-06-06 added the top-of-attempt `cancel.is_cancelled()` guard to both blocking + streaming loops, with `test_precancelled_token_short_circuits_before_request`)
`✅ fixed` · **LOW** · effort **medium** · fix-sketch *done*

- **Gap:** Retry loop sleeps are uninterruptible; no cancellation token in QueryParams.
- **TS:** withRetry.ts:190-192 checks `if (options.signal?.aborted) throw new APIUserAbortError()` at the top of each attempt, and sleep() (line 511/288/501) honors the abort signal so a backoff wait is interruptible.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/services/inference/src/client.rs:579,702 — bare tokio::time::sleep with no select! on token; QueryParams:39-130 has no abort_signal field; client.rs:590-594 doc comments confirm None.
- **Fix:** Add QueryParams.cancel: Option<CancellationToken>; replace `sleep(delay).await` with `tokio::select! { _ = sleep(delay) => {}, _ = token.cancelled() => return Err(Cancelled) }` and check the token at the top of each loop iteration.

### [x] permissions#75 — No 'tool declares no classifier-relevant input' short-circuit; format_action di…
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** Empty input projection (tool declares no classifier-relevant input) does not short-circuit to Allow; TS has this fast-path but Rust delegates to is_safe_tool allowlist instead (intentional divergence per code comments, but doc flags as missing)
- **TS:** yoloClassifier.ts classifyYoloAction (1019-1029): toCompact(action) runs the action through each tool's toAutoClassifierInput projection (toCompactBlock 384-424); if the projection encodes to '' (no security relevance) it returns shouldBlock:false immediately, skipping the classifier.
- **Rust (HEAD):** coco-rs/core/permissions/src/classifier.rs:330-340 (format_action_for_classifier accepts Optional projector, uses raw JSON if projection returns None). Lines 334-335 comment: 'A `None` projection is NOT an auto-allow here — the action being judged must always reach the classifier; the "no security relevance" fast-allow lives upstream in `is_safe_tool` (deli…
- **Fix:** Thread the tool's to_auto_classifier_input projection into format_action_for_classifier (via a per-tool callback), and short-circuit to should_block:false when the projection is empty, before invoking the LLM.

### [x] permissions#78 — Consecutive-denial counter is never reset on rule/hook-allowed tools in auto mo…
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** Rule-based and hook-based Allow decisions exit tool_call_preparer without reset_consecutive; only classifier-Allow path resets. Circuit-breaker can trip incorrectly on mixed allow+deny sequences.
- **TS:** permissions.ts hasPermissionsToUseTool (486-499): on ANY result.behavior==='allow' in auto mode with consecutiveDenials>0, recordSuccess is called to break the streak — regardless of whether the allow came from a rule, the allowlist, the acceptEdits fast-path, or the classifier.
- **Rust (HEAD):** coco-rs/app/query/src/tool_call_preparer.rs:347-368 (hook Allow at 348 and rule-based Allow at 367 both exit function at line 424 without touching denial_tracker). Auto-mode classifier path only entered at line 380 if decision==Ask. Denial tracker reset only happens inside can_use_tool_in_auto_mode (line 91 is_safe_tool, line 113 AllowInCwd, line 124 heuris…
- **Fix:** Add a top-of-wrapper hook in resolve_permission_decision: when the final decision is Allow and auto-mode is active, call denial_tracker.reset_consecutive() (no-op when consecutive==0) regardless of which branch produced the Allow.

### [x] query#2 — No submit-interrupt: Enter-while-streaming only queues, never interrupts the ru… ✅ VERIFIED RESOLVED (2026-06-07)
`✅ fixed` · **LOW** · effort **medium** · fix-sketch *done*

> **2026-06-07 audit (tracker was WRONG):** the full submit-interrupt fabric is live. Marker exists
> (`coco_types::TurnAbortReason::SubmitInterrupt`, `common/types/src/event.rs:1162`). Enter-while-streaming
> fires `UserCommand::Interrupt(SubmitInterrupt)` only when `has_submit_interruptible_tool_in_progress`
> (`app/tui/src/update.rs:388-396`) — i.e. only when every in-progress tool is Cancel-behavior, sparing
> Block tools, exactly mirroring TS `handlePromptSubmit.ts:313-343` + `getAbortReason`. Same fabric as
> tool-runtime#9. The "no marker / no per-tool cancel" claim was stale.

- **Gap:** Enter-while-streaming only queues, never interrupts: InterruptBehavior enum is defined but never used; no interrupt-aware per-tool cancel; no CancelReason::Interrupt marker
- **TS:** handlePromptSubmit.ts:313-343 — when queryGuard.isActive and `params.hasInterruptibleToolInProgress` (all in-progress tools have interruptBehavior()==='cancel', e.g. SleepTool), it calls abortController.abort('interrupt') THEN enqueues. StreamingToolExecutor.ts:219-228: on 'interrupt' reason only 'cancel'-behavior tools are aborted ('user_interrupted'), 'bl…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tool-runtime/src/traits.rs:53-57 InterruptBehavior enum defined, :299 interrupt_behavior() trait method; grep across /lyz/codespace/codex/coco-rs --include='*.rs' for interrupt_behavior() calls outside trait definition/test returns zero production callers. /lyz/codespace/codex/coco-rs/common/types/src/event.rs:1160-1169 Can…
- **Fix:** On QueueCommand while streaming, if any in-progress tool's interrupt_behavior()==Cancel, fire the cancel token with an 'interrupt' reason; thread that reason into the tool executor so only Cancel tools abort and Block tools continue, and suppress the UserInterruption marker when a queued prompt follows.

### [x] shell#164 — acceptEdits auto-allow checks only the first base command, not each subcommand …
`● genuinely_open` · **LOW** · effort **medium** · fix-sketch *sound*

- **Gap:** acceptEdits mode checks only first command, not subcommands; bash filesystem commands never auto-allow in acceptEdits mode (function is unwired)
- **TS:** modeValidation.ts:92-103 checkPermissionMode splits via splitCommand_DEPRECATED and returns allow if ANY subcommand is a filesystem command (mkdir/touch/rm/rmdir/mv/cp/sed). So `cd src && rm old.txt` → finds `rm` → allow.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/exec/shell/src/mode_validation.rs:16-24 is_auto_allowed_in_accept_edits only checks first base executable, no split_compound_command call; is_auto_allowed_in_accept_edits is never called from /lyz/codespace/codex/coco-rs/core/permissions/src/evaluate.rs AcceptEdits arm (checks is_file_modifying_tool instead); /lyz/codespace/3rd/…
- **Fix:** Wire acceptEdits bash auto-allow: split the command via split_compound_command and allow if any subcommand's base executable is in ACCEPT_EDITS_COMMANDS, then call it from the AcceptEdits arm in evaluate.rs for the Bash tool.

### [x] shell#167 — git-commit-substitution / dangerous-variables / incomplete-command early valida…
`● genuinely_open` · **LOW** · effort **medium** · fix-sketch *sound*

- **Gap:** Git-commit-substitution and dangerous-variables validators absent from bash gate; no Ask-approval routing exists (even Ask checks from core security are silently dropped)
- **TS:** bashSecurity.ts:2308-2313 early validators validateIncompleteCommands, validateSafeCommandSubstitution, validateGitCommit (GIT_COMMIT_SUBSTITUTION=12) + validateDangerousVariables (DANGEROUS_VARIABLES=6) route command-substitution inside git commit messages and dangerous variable expansions through approval (behavior:'ask').
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/exec/shell/src/security.rs:26-36 has 5 checks, none for git-commit-substitution or dangerous-variables validators; /lyz/codespace/codex/coco-rs/core/tools/src/tools/bash.rs:661 only acts on SecuritySeverity::Deny (Ask-level checks silently dropped); /lyz/codespace/3rd/claude-code/src/tools/BashTool/bashSecurity.ts:2310-2312,2352…
- **Fix:** Add git-commit-substitution and dangerous-variables validators to the bash gate (or wire the existing DangerousVariablesAnalyzer + a git-commit check via the analyzer suite).

### [x] skills#197 — Dynamically discovered nested .claude/skills dirs are loaded without the gitign…
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** Dynamically discovered nested .coco/skills dirs loaded without gitignore guard; TS skips e.g. node_modules/.coco/skills but Rust loads all
- **TS:** loadSkillsDir.ts:884-898 discoverSkillDirsForPaths runs isPathGitignored(currentDir) (via git check-ignore) before adding skillDir, skipping e.g. node_modules/pkg/.claude/skills; fails open outside a git repo.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/skills/src/lib.rs:738-780 (discover_skill_dirs_for_paths walks upward for .coco/skills dirs, no gitignore check); lib.rs comment line 732-735 explicitly states 'No gitignore filtering'
- **Fix:** Before pushing each discovered dir in track_skill_triggers (or inside discover_skill_dirs_for_paths), check the containing dir against coco_file_ignore (the unified ignore service) and skip ignored dirs.

### [x] tasks#213 — Verification-agent nudge drops the TS feature('VERIFICATION_AGENT') and tengu_h…
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** Verification nudge fires unconditionally without checking BuiltinAgentCatalog.include_verification gate; produces spawn-nonexistent-agent instructions in 3P builds
- **TS:** TaskUpdateTool.ts:334-349 AND TodoWriteTool.ts:77-86 fire the nudge only when feature('VERIFICATION_AGENT') && getFeatureValue_CACHED_MAY_BE_STALE('tengu_hive_evidence', false) (plus main-thread/completed/all-done/>=3/no /verif/). builtInAgents.ts:64-69 gates the catalog inclusion of VERIFICATION_AGENT on the SAME two conditions, so nudge and catalog are al…
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/tasks/src/task_list.rs:705-720 shows should_nudge_verification_after_update calls check_verification_nudge with no gate on include_verification; /lyz/codespace/codex/coco-rs/core/subagent/src/builtins.rs:56 shows include_verification=false in interactive() but nudge path has no access to this flag
- **Fix:** Gate both V1/V2 nudges on the same capability that controls catalog inclusion (e.g. pass the include_verification flag / a Feature into the nudge check) so the nudge only fires when the verification agent is actually registered.

### [x] tools-agent-task#49 — TaskStop hardcodes task_type='background' and drops the command field
`● genuinely_open` · **LOW** · effort **small** · fix-sketch *sound*

- **Gap:** TaskStop hardcodes task_type='background', omits command field, lacks not-running status check; model never sees real task type or shell command.
- **TS:** TaskStopTool.ts:107-131 returns message 'Successfully stopped task: <id> (<command>)' and output {task_id, task_type:<real type>, command} where command is the shell command for shell tasks else the description (stopTask.ts:97-99). validateInput (60-91) also pre-checks status: 'not running' → errorCode 3 'Task <id> is not running (status: X)'.
- **Rust (HEAD):** /lyz/codespace/codex/coco-rs/core/tools/src/tools/task_tools.rs:1100-1154: TaskStopTool::execute() never calls get_task_status() before kill_task(). On Ok() at line 1137-1147 hardcodes json!({ 'message': '...', 'task_id': task_id, 'task_type': 'background' }). No 'command' field in output. No validate_input status pre-check to distinguish not-found from not…
- **Fix:** Before/after kill_task, call ctx.task_handle.task_state(&task_id) and emit the real task_type_wire_name + command (shell_extras().command else description); add a status pre-check that distinguishes not-found vs not-running (errorCode 3).

## P3 — cosmetic / edge (compact)

| ☐ | Finding | Sev | Gap → Fix |
|:-:|---|---|---|
| [x] | tools-file#23 | MEDI | Empty/offset-beyond-EOF read warnings are plain text, not <system-reminder> blocks — *For the empty and offset-beyond-EOF cases, emit the exact TS <system-reminder> strings (parame…* |
| [x] | commands#211 | LOW | /status adds non-TS alias 'st'; /tasks adds non-TS alias 'todo' while dropping genuine TS alia… — *Drop the 'st' and 'todo' aliases (canonical-names-only); if the /tasks 'bashes' alias is inten…* |
| [ ] | config#245 | LOW | CLAUDE_CODE_EFFORT_LEVEL env override with 'unset'/'auto' suppress semantics missing entirely;… — *Add a COCO_EFFORT_LEVEL EnvKey + EnvOnlyConfig field with 'unset'/'auto'->None and numeric par…* |
| [ ] | config#249 | LOW | Plugin settings-base layer (ONE key: 'agent') documented but never merged; triage overstated p… — *Add a getPluginSettingsBase-equivalent that gathers allowlisted plugin-contributed settings an…* |
| [~] | config#250 | LOW | OS-level MDM (plist/HKLM/HKCU) unimplemented; remote API-sync is sanctioned Anthropic-only. Fi… — *Add the MDM (macOS plist / Windows HKLM/HKCU) readers and a remote-managed cache reader to loa…* |
| [x] | config#252 | LOW | get_fast_mode_model returns hardcoded dated ID ('claude-opus-4-6-20250514') matching no catalo… — *Return the fast-mode model via ModelRole resolution / a family selector rather than a hardcode…* |
| [ ] | config#254 | LOW | 'max' effort persists unconditionally to settings.json for all users; TS treats max as session… — *Add a toPersistableEffort-equivalent in the /effort handler that drops 'max' before write_user…* |
| [ ] | context#99 | LOW | File-history keys are absolute paths, not relative to cwd (TS stores relative); JSONL persiste… — *Store tracking keys relative to original cwd when under it (maybe_shorten equivalent) and expa…* |
| [x] | context#102 | LOW | Git status not truncated (2k chars) and not gated on include_git_instructions; remote-session … — *When wiring the git block (context#92): truncate status to 2000 chars with the TS suffix, and …* |
| [ ] | coordinator#259 | LOW | show_pane omits layout reapply and leader pane resize operations — *After the join-pane in show_pane, run select-layout main-vertical, list-panes -F #{pane_id}, t…* |
| [ ] | coordinator#261 | LOW | set_pane_title drops colored pane-border-format, rendering titles without per-teammate bold co… — *After select-pane -T, add a `set-option -p -t <pane> pane-border-format '#[fg=<tmux_color>,bol…* |
| [ ] | coordinator#262 | LOW | Subsequent teammate panes use bare split without list-panes target logic or rebalancing — *In the else branch, list-panes for the window, compute splitVertically = teammateCount%2==1 an…* |
| [ ] | hooks#186 | LOW | Unknown decision/permissionDecision values silently dropped instead of surfaced as non-blockin… — *On an unrecognized decision/permissionDecision value, set a non-blocking error on the aggregat…* |
| [x] | hooks#187 | LOW | Non-zero-exit stderr injected into model context instead of surfaced as non-blocking error — *Gate the PlainText->additional_contexts push on a clean exit (succeeded/exit 0); route non-zer…* |
| [ ] | hooks#188 | LOW | Both flat and nested additionalContext pushed to Vec, causing duplication vs TS single-slot se… — *Use a single per-hook additional-context slot during aggregation with nested-overrides-flat pr…* |
| [ ] | mcp#149 | LOW | Elicitation input validation (format/range/enum constraints and format hints) not ported; Elic… — *Add a validate_elicitation_input(value, schema) and get_format_hint(schema) over the elicitati…* |
| [ ] | mcp#150 | LOW | Natural-language datetime parsing for elicitation date fields absent; Date/DateTime field type… — *After sync validation fails on a date/date-time field and the input isn't ISO-8601, route a sm…* |
| [x] | mcp#156 | LOW | Tool-description truncation suffix (ASCII '...' vs TS Unicode) and length-unit threshold (byte… — *Unify on a single truncation helper using char-aware length and the TS marker '… [truncated]';…* |
| [x] | memory#226 | LOW | Rust caveat wording differs from TS; Rust omits 'point-in-time observations, not live state' a… — *Replace the format! body in memory_freshness_text with the verbatim TS sentence ('Memories are…* |
| [ ] | memory#227 | LOW | Rust extraction prompt places manifest after 'if user asks to remember' line; TS places it bef… — *Move the 'If the user explicitly asks...' line out of extract.md and append it AFTER manifest_…* |
| [ ] | messages#85 | LOW | Web-search-request count never recorded; cost accumulation omits ~$0.01/req charge — *Add a web_search_requests field to TokenUsage (mirroring vercel-ai server_tool_use), populate …* |
| [ ] | messages#86 | LOW | No reorderAttachmentsForAPI (defensive bubble pass); narrow impact since turn-boundary append … — *Port reorderAttachmentsForAPI as a leading pass (bottom-up bubble of Message::Attachment to th…* |
| [ ] | messages#87 | LOW | Assistant merge skips intervening tool_results in TS but not in Rust; latent on streaming path… — *Replace the strictly-adjacent write/read coalesce with a backward walk that, for each Assistan…* |
| [x] | messages#88 | LOW | Decimal precision threshold differs (0.01 vs 0.5); cosmetic UX divergence in cost display — *Change the threshold to match TS: render {:.2} only when cost_usd > 0.5, otherwise {:.4}.* |
| [ ] | plugins#236 | LOW | Install counts never fetched; cache infrastructure exists but fetch path missing. downloads al… — *Add an HTTP fetch (reqwest) of INSTALL_COUNTS_URL with TTL check, persist via InstallCountsCac…* |
| [x] | plugins#237 | LOW | Install-success suffix says '(with N dependencies)' in production; TS spec says '(+ N dependen… — *Replace install.rs::format_dep_note usage with dependency.rs::format_dependency_count_suffix (…* |
| [ ] | plugins#238 | LOW | V1->V2 migration keeps stale V1 installPath instead of recomputing from pluginId+version. vers… — *In migrate_v1_to_v2, ignore the V1 installPath and set install_path = versioned cache path com…* |
| [ ] | query#5 | LOW | max_turns_reached attachment reports turn N instead of N+1 (off-by-one, cosmetic) — *Set payload.turn_count = turn_state.turn + 1 (the turn it refused to start) at engine.rs:572 t…* |
| [ ] | query#6 | LOW | Model-fallback notice emitted as ephemeral TextDelta, not persisted as durable SystemMessage; … — *After emitting the stream TextDelta, also history_push_and_emit a SystemMessage(level=Warning)…* |
| [ ] | query#7 | LOW | Cancel/abort path skips max_turns_reached check; never emits silent attachment even when abort… — *In the engine.rs:507 cancel branch (and after cancel_epilogue), if config.max_turns.is_some_an…* |
| [ ] | sandbox#174 | LOW | getLinuxGlobPatternWarnings not ported. No user-facing warning about glob patterns on Linux wi… — *Add a linux_glob_pattern_warnings(settings) helper that, on cfg!(linux)/WSL when sandbox enabl…* |
| [x] | sandbox#175 | LOW | allow_pty defaults to false in SandboxSettings, overwriting the true default in SandboxConfig.… — *Flip SandboxSettings::default allow_pty to true to match SandboxConfig::default and the TS/san…* |
| [ ] | sandbox#176 | LOW | describe_filesystem/describe_network produce JSON but have zero production callers. Bash tool … — *Inject SandboxState::describe_filesystem()/describe_network() into the bash tool description (…* |
| [ ] | shell#161 | LOW | Sed parser tokenizes via split_whitespace (breaks quoted patterns), accepts any delimiter, and… — *Tokenize via the shell-parser tokenizer instead of split_whitespace; add an `arg.starts_with("…* |
| [ ] | shell#165 | LOW | Deleted cwd not recovered at spawn time; no friendly error message when cwd no longer exists — *In read_cwd_file / the post-exec cwd assignment, verify the new cwd exists (canonicalize/metad…* |
| [ ] | skills#199 | LOW | Per-skill /loop has no runtime CLAUDE_CODE_DISABLE_CRON env kill-switch; only static AgentTrig… — *Model the cron kill-switch as COCO_AGENT_TRIGGERS_DISABLE_CRON (EnvKey + a SkillDefinition run…* |
| [ ] | system-reminder#104 | LOW | Queued-command replay loses source_uuid provenance and human-visibility origin on both reminde… — *Thread source_uuid through QueuedCommandInfo and QueuedCommand→attachment so AttachmentMessage…* |
| [ ] | system-reminder#106 | LOW | Multiple same-type hook events joined into one reminder instead of one attachment each; N same… — *Emit a SystemReminder::messages with one ReminderMessage per hook event (each individually sys…* |
| [ ] | system-reminder#107 | LOW | CLAUDE_CODE_DISABLE_ATTACHMENTS / CLAUDE_CODE_SIMPLE queued-only fallback not implemented; no … — *Add a COCO_* env / settings flag (e.g. COCO_SYSTEM_REMINDER_QUEUED_ONLY) that, when set, makes…* |
| [x] | system-reminder#108 | LOW | Todo reminder body omits TS trailing newline when todo list is empty; empty case: TS ends with… — *Append '\n' to the empty-case return (out.push('\n') before returning when todos.is_empty()), …* |
| [ ] | tasks#215 | LOW | Agent terminal notification escapes summary/result/worktree XML; TS emits them raw for agent v… — *Split rendering so the AgentTerminal arm interpolates summary/result/worktree raw (no escape_x…* |
| [x] | tasks#218 | LOW | Shell terminal output reads last 8MB (tail) with middle-elide truncation; TS reads first 30k (… — *On the backgrounded-shell terminal read path, read from the HEAD (offset 0) up to max_output_b…* |
| [~] | tool-runtime#11 | LOW | Sibling error message lacks <tool_use_error> wrapper and Bash(cmd) descriptor; latent in dead … — *When wiring #8, format the synthetic sibling result as `<tool_use_error>Cancelled: parallel to…* |
| [x] | tool-runtime#13 | LOW | max-tool-concurrency=0 deadlocks; TS treats 0 as falsy and falls back to 10 — *Clamp parsed value: `.and_then(\|v\| v.parse::<usize>().ok()).filter(\|n\| *n > 0).unwrap_or(DEFAU…* |
| [x] | tool-runtime#15 | LOW | Both pre-execution and mid-execution cancels classified as ExecutionCancelled (fires PostToolU… — *Before the run_one select!, check call_ctx.cancel.is_cancelled() and, if set, return an Unstam…* |
| [ ] | tools-exec#32 | LOW | Model-facing exit-code interpretation wired; TUI display fields (returnCodeInterpretation/noOu… — *In the TUI bash result renderer, when stdout/stderr are empty, show returnCodeInterpretation (…* |
| [x] | tools-exec#35 | LOW | Timeout above max is rejected and clamped; TS honors raw model value — *Remove the timeout>max check from validate_input and drop the .min(max_timeout_ms) clamp; use …* |
| [ ] | tools-exec#37 | LOW | run_in_background ignores ctx.background_tasks_disabled gate — *In bash/powershell execute, when ctx.background_tasks_disabled, ignore run_in_background and f…* |
| [ ] | tools-exec#39 | LOW | PowerShell ignores configurable bash timeout/output limits — *Replace the powershell module-const reads with the shared default_timeout_ms/max_timeout_ms(&c…* |
| [ ] | tools-exec#40 | LOW | Auto-detach is reported as backgroundedByUser; assistantAutoBackgrounded never set — *Thread a source discriminant through signal_detach/detach handle (auto-detach-timer vs externa…* |
| [x] | tools-exec#42 | LOW | Bash <claude-code-hint /> tag is never stripped from stdout — *Add a coco-side extract_claude_code_hint(stdout, command) (HINT_TAG_RE parser) called in bash/…* |
| [~] | tools-exec#43 | LOW | Bash captures stdout/stderr separately; TS merges into single chronologically-interleaved stre… — *Run the shell with merged stdout+stderr (redirect child stderr to the stdout fd / a shared fil…* |
| [ ] | tools-file#27 | LOW | Glob does not extract base directory from absolute pattern; silently returns no matches — *Before compiling, if the pattern is absolute, split off the static prefix up to the first glob…* |
| [ ] | tools-web-mcp#58 | LOW | WebFetch URL validation omits userinfo, max-length, and public-hostname checks before fetch — *Add the three checks in execute() before fetch: reject url.len()>2000, reject parsed.username(…* |
| [x] | tools-web-mcp#59 | LOW | CronList derives recurring/durable from wrong fields; durable dropped on create; ScheduleEntry… — *Add recurring/durable fields to ScheduleEntry and ScheduleStore::create_schedule signature; Cr…* |
| [x] | tools-web-mcp#60 | LOW | CronCreate skips next-run reachability and teammate-durable validation that TS enforces — *Add a next-occurrence-within-a-year computation to reject unreachable expressions and, when te…* |
| [ ] | tools-web-mcp#61 | LOW | AskUserQuestion omits uniqueness validation that TS enforces via zod .refine — *Add AskUserQuestionTool::validate_input that rejects duplicate question texts and duplicate op…* |
| [x] | tools-web-mcp#63 | LOW | WebFetch schema declares no required fields, diverging from TS strictObject({url, prompt}) — *Set 'required': ["url", "prompt"] in WebFetchTool's runtime_validation_schema to match the TS …* |

## Closed — fixed / sanctioned (reference; do not re-open)

These were marked "open" by the re-audit but are **fixed in HEAD** (verified). The 14 critical/high the re-audit reclassified out (also verified fixed) follow the table.

| Finding | Sev | Verified status |
|---|---|---|
| plugins#230 | CRIT | ✅ Marketplace fetch and /plugin marketplace update subcommand fully implemented and production-wired; doc's RES… |
| commands#202 | HIGH | ✅ Gap FIXED in commit 92054f73a: /status handler now emits sentinel intercepted by tui_runner to call status_re… |
| commands#203 | HIGH | ✅ /cost handler completely refactored: now emits sentinel intercepted by runner, uses live multi-provider CostT… |
| compact#122 | HIGH | ✅ Rust full compaction now correctly defaults to keep_recent_rounds=0, matching TS behavior of keeping nothing … |
| config#244 | HIGH | ✅ Wire-choke-point clamping in build_call_options fully implements TS parity: explicit numeric efforts resolve … |
| context#92 | HIGH | ✅ Git status block is now fully rendered in system prompt with all required fields (branch, main_branch, user, … |
| coordinator#255 | HIGH | ✅ matchSessionMode (coordinator-mode auto-flip on resume) is now fully wired and production-tested across all r… |
| coordinator#257 | HIGH | ✅ Cross-process worker→leader permission-request resolution loop is fully wired and operational in current HEAD… |
| hooks#179 | HIGH | ✅ Agent hook evaluator is fully implemented and production-wired: spawns child QueryEngine with max_turns=50, S… |
| hooks#180 | HIGH | ✅ hooks#180 — stdout JSON control output now parsed regardless of exit code; fix confirmed present in HEAD with… |
| hooks#181 | HIGH | ✅ HookPermissionDecision now includes Ask variant; both flat and nested hook-output paths handle 'ask' correctl… |
| mcp#143 | HIGH | ✅ MCP env-var expansion (${VAR} / ${VAR:-default}) fully implemented and integrated into config load path; miss… |
| mcp#148 | HIGH | ✅ XAA token-exchange completely fixed by commit 92054f73a: wired to spawn path, both legs have correct paramete… |
| memory#223 | HIGH | ✅ Team-sync bootstrap is fully wired and actively invoked at session start via tui_runner; pull+push+watcher ar… |
| permissions#66 | HIGH | ✅ The file-writes path-safety bypass via "relative or /tmp shortcut" heuristic has been closed; commit 92054f73… |
| shell#157 | HIGH | ✅ Destructive warnings are correctly advisory-only in live code, matching TS behavior; finding conflates check_… |
| tasks#212 | HIGH | ✅ Agent stall watchdog has been completely removed from Rust codebase in commit 92054f73a, achieving TS parity.… |
| tasks#214 | HIGH | ✅ unassign_teammate_tasks is now wired and called; termination flow includes task reassignment notification to … |
| tools-agent-task#45 | HIGH | ✅ ExitWorktree safety gate (validate git state, refuse removal on changes unless discard_changes=true, fail-clo… |
| tools-exec#33 | HIGH | ✅ Auto-background-on-timeout is now fully implemented with default=true, matching TS behavior exactly. Gap is c… |
| tools-file#18 | HIGH | ✅ NotebookEdit insert now correctly places cells AFTER the referenced cell (TS-parity achieved): resolves cell_… |
| tools-web-mcp#53 | HIGH | ✅ WebFetch per-domain permission rules (domain:hostname), "Always allow this domain" suggestions, and preapprov… |
| compact#125 | MEDI | ✅ Commit fa9b29748 fixed the asymmetric keep/clear gates; Rust now matches TS spec: both use COMPACTABLE_TOOLS-… |
| coordinator#260 | MEDI | ✅ enable_pane_border_status correctly uses window-scoped -w -t instead of global -g |
| permissions#71 | MEDI | ✅ Transcript-too-long detection and fallback (ask in interactive / deny in headless) fully implemented and wired |
| permissions#72 | MEDI | ✅ DontAsk mode unconditionally converts ask→deny for all decision branches (tool-wide ask, rule ask, path-safet… |
| permissions#74 | MEDI | ✅ NTFS colon check correctly gated to Windows-only; Unix paths with colons no longer false-blocked |
| query#4 | MEDI | ✅ Agent Stop hooks fixed: evaluate_agent now runs via late-bound HookAgentRunner that executes a scoped 50-turn… |
| tasks#220 | MEDI | ✅ Agent stall watchdog was removed; no longer spawned unconditionally for foreground tasks |
| tasks#222 | MEDI | ✅ Unassign teammate tasks is wired in teardown path and called on teammate terminate/shutdown |
| tools-web-mcp#56 | MEDI | ✅ Preapproved markdown verbatim short-circuit now implemented; raw content returned without LLM extraction |
| permissions#79 | LOW | ✅ Safe-tool, path-safety AllowInCwd, and heuristic Allow branches all reset consecutive-denial counter before r… |

**Reclassified critical/high — verified fixed/sanctioned in HEAD (were not in the 184 set):**

| Label | Module | Verified |
|---|---|---|
| S1 | shell | ✅ compound-command read-only bypass — splits `&& \|\| ; \|`, every subcmd must be read-only |
| C1 | plugins | ✅ remote install reports success — full `coco-git::remote`+`fetch.rs` backend |
| S5 | permissions | ✅ classifier fail-closed on outage (`909b8e418`, `unavailable` flag + opt-in) |
| S3 | permissions | ✅ classifier security-taxonomy prompt (5 BLOCK categories, `92054f73a`) |
| S6 | sandbox | ✅ no-domains full-network — `network_isolated = enabled && !allow_network` |
| S8 | sandbox | ✅ settings.json write-deny populated unconditionally |
| S9 | hooks | ✅ SSRF redirect — `redirect(Policy::none())` + `SsrfGuardedResolver` |
| L2a | inference | ✅ `wrap_provider_error` now routes via `from_http_status` (is_retryable preserved) |
| L2b | inference | ✅ streaming backoff loop in `query_stream_with_config` |
| L2c | inference | ✅ HTTP 5xx now retryable |
| L1 | query | ✅ 30-turn cap removed — `max_turns` default `None` |
| C3 | tools-exec | ✅ non-zero exit surfaced via command-aware `render_for_model` |
| C4 | tools-file/context | ✅ Edit read-before-edit enforced (rejects unread + partial-view) |
| L3 | compact | ✅ Anthropic reactive — server-side queue counts as progress (sanctioned) |

**Partial (reclassified):** S7 sandbox per-domain proxy — macOS starts, Linux fails *closed* (per-domain unwired); L4 compact summarizer — full-compact path still re-ingests prior boundary.

## Notes

- Fix-sketch quality: **135 sound · 28 already-done · 20 sound-with-caveats** — actionable as written.
- **Scope down two sketches:** `permissions#68` (only `.take(10)` cap remains) and `#70` (only `total>=20` trip + reset remain) — other sub-claims already fixed.
- Provenance: this tracker consolidates the Round-1 audit (267 findings) + Round-2 re-audit (184 open + fix sketches) + Round-3 live-HEAD re-validation; the three predecessor docs were folded in here and deleted.

## Plugin V2 unification + adversarial review (feat/review, commits `00522a9d45`..`00f85864d2`)

The plugin system was unified to a **single-tier V2 loader** (TS `loadAllPlugins`);
the legacy name-keyed V1 was deleted outright (no back-compat). All **7 TS
contribution types** are now production-wired (were 3):

| Type | State | Notes |
|---|---|---|
| commands / skills / hooks | ✅ wired | Stage 3; V2-only bridges |
| output-styles | ✅ wired | bridge existed, both `build_output_style_manager` sites had passed `&[]` |
| agents | ✅ wired | `PluginAgentDir` + `<plugin>:<agent>` namespacing + TS security gate (strips permission_mode/hooks/mcp_servers) |
| MCP | ✅ wired | `mcp_bridge`; **SDK-path only** (TUI builds no MCP manager yet) |
| LSP | ✅ wired | `lsp_bridge` via `LspServerManager::merge_config` |

Also: `/plugin enable|disable` resolves the full id from `load_all_installed_plugins`
(works for marketplace plugins now, not just inline); dead `loader::resolve_dependency_closure`
deleted. Identity: local = `name@inline`; MCP/LSP servers = `plugin:<plugin>:<server>`.

**Deferred follow-ups (not regressions — don't re-flag as new gaps):**
- MCP bridge SDK-only until the TUI grows an `McpConnectionManager`.
- Nested-dir namespacing: command/skill bridges use shallow `read_dir` (deeply-nested
  files dropped + flat namespace vs TS `walkPluginMarkdown` multi-segment).
- `verify_and_demote` uses a non-counted name set → transitive bare-name demotion may
  not cascade; emits no error records.
- Single-file manifest `agents` entries not mapped (only `agents/` + manifest agent dirs).
- No enterprise-policy enforcement at **load** time (gated at install/enable only).
- Builtins-merge: ✅ wired (2026-06-06) — `init_builtin_plugins()` + `builtin_plugin_skills()` merge consulted at bootstrap + `/reload-plugins`. `register_builtin_plugin` itself stays caller-less *by design* (coco ships no builtins yet; TS's `initBuiltinPlugins` is likewise an empty scaffold) — the merge path is now live so a future builtin surfaces.
- No reconcile-on-load for declared-but-unmaterialized marketplaces.
