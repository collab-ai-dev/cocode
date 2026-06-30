---
allowed-tools: Bash(python3:*), Bash(ls:*), Bash(grep:*), Bash(rg:*), Bash(sed:*), Bash(wc:*), Bash(jq:*), Read, Grep, Glob
description: Analyze / troubleshoot (排查) a coco-rs run by correlating its logs, session transcript, wire captures and user prompts — optionally narrowed to a focus area (UI perf, provider/LLM, permissions, MCP, tools, compaction, cost). Takes an optional PID (may have already exited); otherwise resolves from the project directory.
argument-hint: "[pid] [focus…]"
---

## Context

- Arguments — `[pid] [focus…]`: `$ARGUMENTS`
- Project directory: !`echo "${CLAUDE_PROJECT_DIR:-$(pwd)}"`
- coco-rs config home: !`echo "${COCO_CONFIG_DIR:-$HOME/.cocode}"`
- Resolver location (you run it yourself — see "Run the resolver" below):
  !`sub=".claude/skills/coco-analyze/resolve.py"; SK=""; ROOT="$PWD"; for c in "$CLAUDE_PROJECT_DIR" "$PWD" "$HOME"; do d="$c"; while [ -n "$d" ] && [ "$d" != "/" ]; do if [ -f "$d/$sub" ]; then SK="$d/$sub"; ROOT="$d"; break 2; fi; d=$(dirname "$d"); done; done; if [ -z "$SK" ]; then echo "ERROR: coco-analyze resolve.py not found (cwd=$PWD CLAUDE_PROJECT_DIR=${CLAUDE_PROJECT_DIR:-unset})"; else echo "SCRIPT=$SK"; echo "CWD=${CLAUDE_PROJECT_DIR:-$ROOT}"; fi`

## Run the resolver (do this first)

The `$ARGUMENTS` above are **free text** (a PID plus a free-form focus that may
contain quotes, backticks, newlines, `$`, CJK …). **Never** interpolate them into a
shell `!`-command: the skill expands `$ARGUMENTS` *textually* into the command string
before the shell runs, so a `"` or `` ` `` in the focus breaks parsing (`unmatched "`).
That is why the auto-run was removed — you run the resolver yourself instead:

1. From `$ARGUMENTS`, take the **PID** = the first whitespace-delimited token that is
   all digits (there may be none). Ignore the rest — it is your focus hint.
2. Run the resolver with the **Bash tool**, passing only that numeric PID — never the
   focus text:
   - with a PID: `python3 <SCRIPT> --cwd <CWD> <pid>`
   - no PID:     `python3 <SCRIPT> --cwd <CWD>`
   - UI-perf focus (see Focus areas): add `--perf`; enumerate sessions with `--list`.
   `<SCRIPT>` and `<CWD>` are printed by the "Resolver location" line above.
3. Read the report it prints, then proceed with the analysis below.

## What this skill does

Analyze a coco-rs run by joining four artifact sources it writes under `~/.cocode`
(override with `COCO_CONFIG_DIR`). The resolver you run (see "Run the resolver"
above) locates and triages them; your job is to read the relevant ones and explain
what happened — a clean run, or a concrete failure with its root cause.

`resolve.py` is only a **locator + light triage**. The real analysis is yours: it
points at absolute paths and surfaces obvious signals (WARN/ERROR counts, non-ok wire
turns, user inputs, cost); you read the files to reach a verdict.

### The resolution chain (how the report found the files)

1. **cwd → project.** A coco-rs run's project dir is the project directory
   (`$CLAUDE_PROJECT_DIR`, falling back to `pwd`) *or* its `coco-rs/` subdirectory.
   The on-disk project folder under `~/.cocode/projects/` is the absolute cwd with every
   `/`, `.`, `_` replaced by `-` (e.g. `/Users/x/codex/coco-rs` → `-Users-x-codex-coco-rs`).
2. **PID → cwd + session_id.** `~/.cocode/sessions/pids/<pid>.json` maps a process to
   `{cwd, session_id, started_at}`. **This file survives after the process exits**, so
   a stale PID still resolves. If the pid file was reaped, the resolver recovers
   `cwd` + `session_id` by parsing the log file's startup lines instead.
3. **session_id → transcript + wire.** `~/.cocode/projects/<proj>/<session_id>.jsonl`
   is the transcript; `~/.cocode/projects/<proj>/<session_id>/wire/` holds raw provider
   HTTP; `.../usage.json` holds token/cost totals. The resolver finds these by globbing
   the session_id directly, so it never has to guess the sanitized name.
4. **PID → logs.** `~/.cocode/logs/coco.<pid>.log.<YYYY-MM-DD>` (rotated daily; there may
   be several dated files per pid). The resolver picks the newest and counts WARN/ERROR.

> File **location** is format-independent (filenames + JSON schemas). Only the
> WARN/ERROR *count* and the pidfile-missing *recovery* parse log text, and both
> degrade gracefully — the files are still found and you can `grep`/`Read` the raw log.

If no PID is given, the resolver matches pid files by cwd (exact cwd match wins over a
subdir match, then running over exited, then newest) and analyzes the best one, listing
the rest so you can re-run with an explicit PID.

## Focus areas (optional)

`$ARGUMENTS` is `[pid] [focus…]`: the **first numeric token is the PID**, the remainder
is a free-text focus. The resolver ignores the focus — **you** use it to decide where to
dig. With no focus, do the general triage below. Match focus loosely (substring/synonym,
English or 中文); if it names something not listed, infer the right crate/log target and
grep for it.

- **UI / TUI performance** (`ui`, `perf`, `flicker`, `闪烁`, `位移`, `跳动`, `渲染`, `性能`) →
  run **`resolve.py <pid> --perf`** for the full breakdown: per-stage frame timings
  (`viewport_draw` / `native_surface_draw` + `render/diff/draw/flush_us`), frames over the
  16.6ms/60fps budget, **action frequency** (`cmd=` / `key=` / input-load counters), and
  **flicker / 位移 proxies**. ⚠ These lines only exist if the run had
  `tui.performance.enabled=true` **and** log filter `tui=debug`; if absent, the tool says
  so — tell the user to enable both and reproduce.
  - **No direct `flicker`/`闪烁`/`位移` log key exists** — infer from proxies:
    **闪烁/flicker** ≈ `invalidated=true` (full repaint vs incremental diff) + high
    `buffer_updates`, especially consecutive bursts. **位移/图标跳动/jump** ≈
    `input_bottom`/`viewport_bottom` changing frame-to-frame (composer/viewport reseat)
    and scrollback-commit churn. `--perf` computes all of these; read them, don't grep raw.
- **Provider / LLM** (`provider`, `llm`, `model`, `api`, `stream`) → the **wire** dir:
  `index.jsonl` for `outcome`/`status`, then the offending `*.resp.txt` (raw SSE) and
  `*.req.json` (params/tools/messages). Log anchors: `coco_inference`, retry / rate-limit.
- **Permissions** (`permission`, `approval`, `权限`) → log `permission_controller`,
  `approval bridge`; cross-reference the tool that was gated.
- **Tools** (`tool`, `工具`) → log `tool_outcome_builder`, `coco_tools`; the tool error text.
- **MCP** (`mcp`) → log `coco_mcp`, `rmcp`; connect / auth (`needs_auth`, 401, OAuth).
- **Compaction / context / cost** (`compact`, `context`, `token`, `cost`, `成本`) →
  `usage.json` totals + `req_bytes` growth across `wire/index.jsonl`; log `coco_compact`.
- **Startup / config** (`startup`, `config`, `boot`) → log `session_bootstrap`,
  `runtime config`, model-role resolution.

## How to investigate

Read the triage report first. Then go deeper **only into what the focus / symptom points
at** (the focus section above maps each area to its artifact + grep anchors):

- **Crash / error / unexpected behavior →** the **log**. Read the WARN/ERROR lines, then
  `grep`/`Read` around their timestamps — the line *preceding* an error is usually the
  cause. General anchors: `coco_query`, `coco_tools`, `permission`, `session_bootstrap`,
  `task_runtime`, `mcp`.
- **Bad / empty / malformed model output, tool-call problems, refusals →** the **wire**
  capture for the offending turn (`*.req.json` / `*.resp.txt` / `*.meta.json`). Check
  `index.jsonl` for any `outcome != ok` or non-2xx `status`.
- **"Why did it do that" / prompt or flow questions →** the **transcript** jsonl. Each
  record is one event: `user` (typed input is prefixed `❯`; slash commands carry
  `<command-name>`; tool results and reminders also arrive as `user` role), `assistant`,
  `attachment`, `system`, `file-history-snapshot`.
- **Cost / token / context-pressure →** `usage.json` + `req_bytes` growth in `index.jsonl`.

Always cross-reference: one turn appears in all three streams at the same wall-clock time
(transcript timestamp ≈ wire `ts_ms` ≈ log timestamp). Use that to line up "user asked X"
→ "request sent" → "log error" → "response".

## Output

**Reply in the language of the user's prompt/focus.** Match the language of the `focus`
text (or, if no focus, the user's request): a Chinese focus like `分析UI渲染性能` →
write the whole report in 中文; an English focus → English. Keep code, log lines, paths,
field names and identifiers verbatim regardless of report language.

1. **What was resolved** — pid (running/exited), cwd, session_id, artifact paths; and the
   focus you investigated under (if any).
2. **Findings** — the concrete result, quoting the decisive log line / wire field /
   transcript entry (with `file:line`-style refs to the artifact when useful).
3. **Root cause** — the why, tied to the evidence.
4. **Fix / next step** — what to change in coco-rs source, or what to capture next if the
   evidence is insufficient (e.g. "enable `tui.performance` and re-run").

If nothing is wrong, say so plainly and point at what *did* happen (last assistant action,
last wire outcome) rather than inventing a problem.

## Notes

- `resolve.py --list` enumerates every known coco-rs session (pid, running state,
  session_id, cwd) — use it when the user doesn't know the PID.
- `resolve.py --cwd <dir>` forces a working directory; `resolve.py <pid>` targets one run.
- `resolve.py <pid> --perf` emits the focused UI-render report (frame timing, action
  frequency, flicker/位移 proxies) — the analyzer for any UI-performance focus.
- Secrets in wire `meta.json` headers are already `[REDACTED_SECRET]`; never echo raw
  `Authorization` values even if a future capture is unredacted.
- Artifacts can be large (logs MBs, req bodies 100KB+). Prefer `grep`/`rg` with context
  flags and targeted `Read` offsets over reading whole files.
