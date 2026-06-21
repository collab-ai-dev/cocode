#!/usr/bin/env python3
"""Resolve and triage coco-rs runtime artifacts for debugging.

Given a coco-rs PID (possibly already exited) or the current working directory,
locate the matching log file, session transcript, wire-capture dir and usage
file, then print a triage report with absolute paths so the agent can dig in.

Resolution chain (mirrors how coco-rs lays files out under ~/.coco):
  1. PID  -> ~/.coco/sessions/pids/<pid>.json -> {cwd, session_id, started_at}
            (falls back to parsing the log file if the pid file was reaped)
  2. cwd  -> caller's $PWD or $PWD/coco-rs -> matching pid file(s)
  3. session_id -> ~/.coco/projects/*/<session_id>.jsonl   (glob, no guessing)
  4. project dir = parent of that jsonl; wire = <proj>/<sid>/wire
  5. log  -> newest ~/.coco/logs/coco.<pid>.log*

Usage:
  resolve.py [PID]            # explicit pid (running or exited)
  resolve.py --cwd <dir>      # force a working dir instead of $PWD
  resolve.py --list           # list all known coco-rs sessions and exit
"""

from __future__ import annotations

import glob
import json
import os
import re
import sys
from datetime import datetime, timezone

HOME = os.path.expanduser("~")
COCO = os.environ.get("COCO_CONFIG_HOME") or os.environ.get("COCO_HOME") or os.path.join(HOME, ".coco")
PIDS_DIR = os.path.join(COCO, "sessions", "pids")
LOGS_DIR = os.path.join(COCO, "logs")
PROJECTS_DIR = os.path.join(COCO, "projects")

# Tolerant: the level token is one of the first words of the line, regardless of
# the exact timestamp shape/spacing. Survives log-format tweaks (the 45-char cap
# keeps it from matching a "WARN"/"ERROR" word inside later message prose).
LEVEL_RE = re.compile(r"^.{0,45}?\b(TRACE|DEBUG|INFO|WARN|ERROR)\b")
UUID_RE = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")


def human_size(n: int) -> str:
    f = float(n)
    for unit in ("B", "KB", "MB", "GB"):
        if f < 1024 or unit == "GB":
            return f"{f:.0f}{unit}" if unit == "B" else f"{f:.1f}{unit}"
        f /= 1024
    return f"{f:.1f}GB"


def fmt_ms(ms) -> str:
    try:
        return datetime.fromtimestamp(ms / 1000, tz=timezone.utc).astimezone().strftime("%Y-%m-%d %H:%M:%S")
    except Exception:
        return str(ms)


def is_running(pid) -> bool:
    try:
        os.kill(int(pid), 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except Exception:
        return False


def load_pidfile(pid: int) -> dict | None:
    path = os.path.join(PIDS_DIR, f"{pid}.json")
    try:
        with open(path) as f:
            d = json.load(f)
        d["_pidfile"] = path
        return d
    except Exception:
        return None


def all_pidfiles() -> list[dict]:
    out = []
    for path in glob.glob(os.path.join(PIDS_DIR, "*.json")):
        try:
            with open(path) as f:
                d = json.load(f)
            d["_pidfile"] = path
            out.append(d)
        except Exception:
            pass
    return out


def find_logs(pid: int) -> list[str]:
    logs = glob.glob(os.path.join(LOGS_DIR, f"coco.{pid}.log*"))
    logs.sort(key=lambda p: os.path.getmtime(p), reverse=True)
    return logs


def parse_log_meta(pid: int) -> dict:
    """Recover cwd + session_id from a log file when the pid file is gone."""
    meta: dict = {}
    for log in find_logs(pid):
        try:
            with open(log, errors="replace") as f:
                head = f.read(20000)
        except Exception:
            continue
        m = re.search(r"building engine resources cwd=(\S+)", head) or re.search(r"\bcwd=(/\S+)", head)
        if m and "cwd" not in meta:
            meta["cwd"] = m.group(1)
        m = re.search(r"tasks/(" + UUID_RE.pattern + r")", head)
        if m and "session_id" not in meta:
            meta["session_id"] = m.group(1)
        if "cwd" in meta and "session_id" in meta:
            break
    return meta


def find_session_jsonl(session_id: str) -> str | None:
    hits = glob.glob(os.path.join(PROJECTS_DIR, "*", f"{session_id}.jsonl"))
    return hits[0] if hits else None


def sanitize_cwd(cwd: str) -> str:
    return re.sub(r"[/._]", "-", cwd)


def cwd_candidates(cwd: str) -> list[str]:
    cwd = os.path.abspath(cwd)
    cands = [cwd]
    if os.path.basename(cwd) != "coco-rs":
        cands.append(os.path.join(cwd, "coco-rs"))
    return cands


def pidfiles_for_cwd(cwd: str) -> list[dict]:
    """Rank: exact cwd match > subdir match; then running > exited; then newest."""
    cands = cwd_candidates(cwd)
    matches = []
    for pf in all_pidfiles():
        pc = pf.get("cwd", "")
        rank = 0
        if pc in cands:
            rank = 2
        elif any(pc.startswith(c + "/") for c in cands):
            rank = 1
        if rank:
            pf["_rank"] = rank
            matches.append(pf)
    matches.sort(key=lambda d: (d["_rank"], is_running(d.get("pid", 0)), d.get("started_at", 0)), reverse=True)
    return matches


# ----------------------------------------------------------------------------- triage


def triage_log(logs: list[str]) -> list[str]:
    out = []
    for log in logs:
        size = os.path.getsize(log)
        out.append(f"- `{log}`  ({human_size(size)})")
    if not logs:
        return ["- (no log file found for this pid)"]
    primary = logs[0]
    counts = {"WARN": 0, "ERROR": 0}
    interesting: list[str] = []
    try:
        with open(primary, errors="replace") as f:
            for line in f:
                m = LEVEL_RE.match(line)
                if not m:
                    continue
                lvl = m.group(1)
                if lvl in counts:
                    counts[lvl] += 1
                    interesting.append(line.rstrip())
    except Exception as e:
        out.append(f"  (could not read log: {e})")
        return out
    out.append(f"- level counts (primary log): ERROR={counts['ERROR']} WARN={counts['WARN']}")
    if interesting:
        out.append("- last WARN/ERROR lines:")
        for line in interesting[-20:]:
            out.append(f"    {line[:300]}")
    return out


def triage_wire(wire_dir: str) -> list[str]:
    out = []
    idx = os.path.join(wire_dir, "index.jsonl")
    if not os.path.isdir(wire_dir):
        return ["- (no wire dir)"]
    entries = []
    if os.path.exists(idx):
        with open(idx, errors="replace") as f:
            for line in f:
                try:
                    entries.append(json.loads(line))
                except Exception:
                    pass
    n_files = len(glob.glob(os.path.join(wire_dir, "*.req.json")))
    out.append(f"- wire dir: `{wire_dir}` ({n_files} requests captured)")
    if not entries:
        out.append("- (index.jsonl empty or absent — inspect *.meta.json / *.resp.txt directly)")
        return out
    providers = sorted({e.get("provider") for e in entries if e.get("provider")})
    models = sorted({e.get("model") for e in entries if e.get("model")})
    out.append(f"- providers: {', '.join(providers)} | models: {', '.join(models)} | requests: {len(entries)}")
    def _is_bad(e) -> bool:
        if e.get("outcome") not in (None, "ok"):
            return True
        st = e.get("status")
        return st is not None and not (200 <= st < 300)

    bad = [e for e in entries if _is_bad(e)]
    if bad:
        out.append(f"- ⚠ {len(bad)} non-ok request(s):")
        for e in bad[-10:]:
            out.append(
                f"    seq={e.get('seq')} {e.get('turn_id')} outcome={e.get('outcome')} "
                f"status={e.get('status')} model={e.get('model')}  (see {e.get('seq'):04d}-*.resp.txt)"
            )
    else:
        out.append("- all captured requests outcome=ok")
    last = entries[-1]
    out.append(
        f"- last request: {last.get('turn_id')} outcome={last.get('outcome')} "
        f"resp_bytes={last.get('resp_bytes')} ts={fmt_ms(last.get('ts_ms'))}"
    )
    return out


def _msg_text(msg) -> str:
    c = msg.get("content") if isinstance(msg, dict) else None
    if isinstance(c, str):
        return c
    if isinstance(c, list):
        return "".join(p.get("text", "") for p in c if isinstance(p, dict) and p.get("type") == "text")
    return ""


def triage_session(jsonl: str) -> list[str]:
    out = []
    types: dict[str, int] = {}
    prompts: list[tuple[str, str]] = []
    last_assistant = ""
    try:
        with open(jsonl, errors="replace") as f:
            lines = f.readlines()
    except Exception as e:
        return [f"- (could not read transcript: {e})"]
    out.append(f"- transcript: `{jsonl}` ({len(lines)} records)")
    for line in lines:
        try:
            o = json.loads(line)
        except Exception:
            continue
        t = o.get("type", "?")
        types[t] = types.get(t, 0) + 1
        if t == "user":
            txt = _msg_text(o.get("message", {})).strip()
            if not txt:
                continue
            ts = o.get("timestamp", "")[:19].replace("T", " ")
            if "<command-name>" in txt:
                m = re.search(r"<command-name>(.*?)</command-name>", txt, re.S)
                prompts.append((ts, f"/cmd {m.group(1).strip() if m else txt[:60]}"))
            elif txt.startswith("❯"):
                prompts.append((ts, txt.lstrip("❯ ").replace("\n", " ")))
            else:
                prompts.append((ts, "(injected) " + txt.replace("\n", " ")))
        elif t == "assistant":
            at = _msg_text(o.get("message", {})).strip()
            if at:
                last_assistant = at
    out.append("- record types: " + ", ".join(f"{k}={v}" for k, v in sorted(types.items())))
    if prompts:
        out.append("- user inputs (chronological):")
        for ts, txt in prompts:
            out.append(f"    [{ts}] {txt[:160]}")
    if last_assistant:
        out.append(f"- last assistant text: {last_assistant[:200].replace(chr(10), ' ')}")
    return out


def triage_usage(usage: str) -> list[str]:
    if not os.path.exists(usage):
        return ["- (no usage.json)"]
    try:
        with open(usage) as f:
            d = json.load(f)
    except Exception as e:
        return [f"- (could not read usage.json: {e})"]
    t = d.get("totals", {})
    return [
        f"- usage: `{usage}`",
        f"- requests={t.get('request_count')} in={t.get('input_tokens')} out={t.get('output_tokens')} "
        f"cache_read={t.get('cache_read_input_tokens')} cost=${t.get('total_cost_usd', 0):.4f}",
    ]


# ----------------------------------------------------------------------------- perf

FRAME_BUDGET_US = 16600  # one 60fps frame


def _kv_int(field: str, s: str):
    m = re.search(rf"\b{field}=(-?\d+)", s)
    return int(m.group(1)) if m else None


def _kv_bool(field: str, s: str):
    m = re.search(rf"\b{field}=(true|false)\b", s)
    return (m.group(1) == "true") if m else None


def _stats(xs: list[int]) -> str:
    if not xs:
        return "n=0"
    sx = sorted(xs)
    p95 = sx[min(len(sx) - 1, int(len(sx) * 0.95))]
    med = sx[len(sx) // 2]
    return f"n={len(xs)} med={med} p95={p95} max={max(sx)}"


def triage_perf(logs: list[str]) -> list[str]:
    """TUI render-perf analysis: frame timing, action frequency, flicker/位移 proxies.

    coco-rs has NO 'flicker'/'位移' log keyword — these are inferred from
    `invalidated=true` (full repaint) and input_bottom/viewport_bottom churn.
    Lines only exist when the run had tui.performance.enabled + tui=debug.
    """
    if not logs:
        return ["- (no log file — cannot analyze perf)"]
    primary = logs[0]
    try:
        with open(primary, errors="replace") as f:
            lines = f.readlines()
    except Exception as e:
        return [f"- (could not read log: {e})"]

    stage_dur: dict[str, list[int]] = {}
    sub: dict[str, list[int]] = {k: [] for k in ("render_us", "diff_us", "draw_us", "flush_us")}
    inval: list[bool] = []          # invalidated per viewport_draw, in frame order
    buf_updates: list[int] = []
    seats: list[tuple] = []         # (input_bottom, viewport_bottom) per viewport_draw
    history_rows: list[int] = []
    cmds: dict[str, int] = {}
    keys: dict[str, int] = {}
    redraw = dict.fromkeys(
        ("stream_text_deltas", "stream_thinking_deltas", "core_events", "terminal_inputs", "ticks", "settings_reloads"), 0
    )
    slow_cells: list[str] = []

    for l in lines:
        if "tui::perf::frame" in l:
            if "redraw completed" in l:
                for k in redraw:
                    v = _kv_int(k, l)
                    if v:
                        redraw[k] += v
                continue
            stg = re.search(r'stage="([^"]+)"', l)
            dur = _kv_int("duration_us", l)
            if not stg or dur is None:
                continue
            s = stg.group(1)
            stage_dur.setdefault(s, []).append(dur)
            if s == "viewport_draw":
                for k in sub:
                    v = _kv_int(k, l)
                    if v is not None:
                        sub[k].append(v)
                iv = _kv_bool("invalidated", l)
                if iv is not None:
                    inval.append(iv)
                bu = _kv_int("buffer_updates", l)
                if bu is not None:
                    buf_updates.append(bu)
                ib, vb = _kv_int("input_bottom", l), _kv_int("viewport_bottom", l)
                if ib is not None and vb is not None:
                    seats.append((ib, vb))
            elif s == "native_surface_draw":
                hr = _kv_int("history_rows", l)
                if hr is not None:
                    history_rows.append(hr)
        elif "tui::perf::cell" in l and "slow" in l:
            slow_cells.append(l.strip())
        elif "coco_tui::command" in l:
            m = re.search(r"\bcmd=(\w+)", l)
            if m:
                cmds[m.group(1)] = cmds.get(m.group(1), 0) + 1
        elif "coco_tui::keybinding" in l:
            m = re.search(r"\bkey=(\S+)", l)
            if m:
                keys[m.group(1)] = keys.get(m.group(1), 0) + 1

    if not stage_dur:
        return [
            "- perf埋点 OFF (no `tui::perf::frame` lines).",
            "  Enable BOTH `tui.performance.enabled=true` (settings.json) AND log filter `tui=debug`, then reproduce.",
        ]

    out = ["- perf埋点 ON", ""]
    out.append("### Frame stages (duration_us)")
    for s in sorted(stage_dur, key=lambda k: -len(stage_dur[k])):
        out.append(f"- {s:22} {_stats(stage_dur[s])}")
    # frame total = native_surface_draw (outer) when present, else viewport_draw
    frame = stage_dur.get("native_surface_draw") or stage_dur.get("viewport_draw") or []
    over = sum(1 for d in frame if d >= FRAME_BUDGET_US)
    out.append(f"- frames over 16.6ms (60fps budget): {over} / {len(frame)}")

    out.append("")
    out.append("### viewport_draw sub-stages (us)")
    for k in ("render_us", "diff_us", "draw_us", "flush_us"):
        if sub[k]:
            out.append(f"- {k:10} {_stats(sub[k])}")

    out.append("")
    out.append("### Flicker proxies  (no direct 'flicker'/闪烁 log key — inferred)")
    if inval:
        nt = sum(inval)
        run = best = 0
        for v in inval:
            run = run + 1 if v else 0
            best = max(best, run)
        out.append(
            f"- invalidated=true (full repaint): {nt}/{len(inval)} frames "
            f"({100 * nt // max(1, len(inval))}%), longest consecutive burst={best}"
        )
    if buf_updates:
        out.append(f"- buffer_updates (cells rewritten/frame): {_stats(buf_updates)}")
    out.append("  ↳ many invalidated=true (esp. consecutive) + high buffer_updates ≈ visible flash.")

    out.append("")
    out.append("### Viewport reseat proxies  (位移 / 图标跳动 — inferred)")
    reseats = sum(1 for i in range(1, len(seats)) if seats[i] != seats[i - 1])
    distinct = sorted(set(seats))
    out.append(f"- input/viewport-bottom transitions between frames: {reseats} (distinct seatings: {len(distinct)})")
    if 1 < len(distinct) <= 8:
        out.append("  seatings (input_bottom,viewport_bottom): " + ", ".join(str(d) for d in distinct))
    if history_rows:
        # history_rows is the *last* insert's row count (sticky across frames),
        # so count value-changes ≈ distinct scrollback-commit events, not a sum.
        events = sum(1 for i in range(1, len(history_rows)) if history_rows[i] != history_rows[i - 1])
        out.append(f"- scrollback-commit events (history_rows changes, approx): {events}; rows/commit max={max(history_rows)}")
    out.append("  ↳ frequent transitions / commit churn mid-stream ≈ input bar or icons jumping.")

    out.append("")
    out.append("### Actions (frequency)")
    if cmds:
        out.append("- commands: " + " ".join(f"{k}={v}" for k, v in sorted(cmds.items(), key=lambda kv: -kv[1])))
    if keys:
        out.append("- keys: " + " ".join(f"{k}={v}" for k, v in sorted(keys.items(), key=lambda kv: -kv[1])[:15]))
    if any(redraw.values()):
        out.append("- input load (summed): " + " ".join(f"{k}={v}" for k, v in redraw.items() if v))
    if slow_cells:
        out.append(f"- slow transcript cell renders: {len(slow_cells)}")
        for c in slow_cells[:5]:
            # keep the tail (cell=… lines_added=… duration_us=…), drop the timestamp prefix
            out.append("    " + re.sub(r"^.*?slow transcript cell render ", "", c)[:140])
    return out


# ----------------------------------------------------------------------------- main


def emit_report(pid, info: dict, perf: bool = False):
    cwd = info.get("cwd")
    sid = info.get("session_id")
    started = info.get("started_at")
    logs = find_logs(pid) if pid else []

    if perf:
        # Focused UI-render report: resolution header + perf only (the full
        # triage is already available from the default run).
        print("# coco-rs UI render performance\n")
        print("## Resolution")
        print(f"- pid: {pid}" + (f"  (running: {'yes' if is_running(pid) else 'no — exited'})" if pid else ""))
        print(f"- cwd: {cwd}")
        print(f"- session_id: {sid}")
        print(f"- log: {logs[0] if logs else '(none)'}")
        print()
        print("## UI render performance (tui::perf)")
        for l in triage_perf(logs):
            print(l)
        return

    jsonl = find_session_jsonl(sid) if sid else None
    if not jsonl and cwd and sid:
        # last resort: sanitize cwd to the project dir name
        cand = os.path.join(PROJECTS_DIR, sanitize_cwd(cwd), f"{sid}.jsonl")
        if os.path.exists(cand):
            jsonl = cand
    project = os.path.dirname(jsonl) if jsonl else (
        os.path.join(PROJECTS_DIR, sanitize_cwd(cwd)) if cwd else None
    )
    wire = os.path.join(project, sid, "wire") if (project and sid) else None
    usage = os.path.join(project, sid, "usage.json") if (project and sid) else None

    print("# coco-rs debug context\n")
    print("## Resolution")
    print(f"- pid: {pid}" + (f"  (running: {'yes' if is_running(pid) else 'no — exited'})" if pid else "  (unknown)"))
    print(f"- cwd: {cwd}")
    print(f"- session_id: {sid}")
    if started:
        print(f"- started_at: {fmt_ms(started)}")
    print(f"- project dir: {project}")
    print()

    print("## Logs")
    for l in triage_log(logs):
        print(l)
    print()

    if jsonl:
        print("## Session transcript")
        for l in triage_session(jsonl):
            print(l)
        print()
    else:
        print("## Session transcript\n- (not found — no transcript for this session_id)\n")

    print("## Wire (raw provider HTTP)")
    if wire:
        for l in triage_wire(wire):
            print(l)
    else:
        print("- (wire dir unresolved)")
    print()

    print("## Usage")
    if usage:
        for l in triage_usage(usage):
            print(l)
    print()

    print("## Next steps for the agent")
    print("- Read the WARN/ERROR log lines above; grep the primary log around their timestamps for context.")
    if wire and os.path.isdir(wire):
        print(f"- For LLM-side issues, read `{wire}/<seq>-*.req.json` (request) and `.resp.txt` (raw stream).")
    print("- Correlate user inputs (transcript) with the log timeline to pin the failing turn.")


def main(argv: list[str]) -> int:
    args = argv[1:]
    if "--list" in args:
        rows = all_pidfiles()
        rows.sort(key=lambda d: d.get("started_at", 0), reverse=True)
        print("# known coco-rs sessions\n")
        for pf in rows:
            pid = pf.get("pid")
            run = "RUN " if is_running(pid) else "exit"
            print(f"- [{run}] pid={pid} sid={pf.get('session_id')} started={fmt_ms(pf.get('started_at'))}\n        cwd={pf.get('cwd')}")
        return 0

    perf = "--perf" in args
    args = [a for a in args if a != "--perf"]

    forced_cwd = None
    if "--cwd" in args:
        i = args.index("--cwd")
        forced_cwd = args[i + 1] if i + 1 < len(args) else None
        args = args[:i] + args[i + 2:]

    pid_arg = next((a for a in args if a.isdigit()), None)

    if pid_arg:
        pid = int(pid_arg)
        pf = load_pidfile(pid)
        if pf:
            info = {"cwd": pf.get("cwd"), "session_id": pf.get("session_id"), "started_at": pf.get("started_at")}
        else:
            print(f"(no pid file at {os.path.join(PIDS_DIR, f'{pid}.json')} — recovering from log)\n")
            info = parse_log_meta(pid)
            if not info:
                print(f"ERROR: pid {pid} has no pid file and no readable log under {LOGS_DIR}.")
                print("Try `resolve.py --list` to see known sessions.")
                return 1
        emit_report(pid, info, perf=perf)
        return 0

    # cwd mode
    cwd = forced_cwd or os.getcwd()
    matches = pidfiles_for_cwd(cwd)
    if not matches:
        print(f"No coco-rs pid files match cwd `{cwd}` (or its coco-rs/ subdir).")
        print("Pass a PID explicitly, or run `resolve.py --list`.")
        return 1
    chosen = matches[0]
    if len(matches) > 1:
        scope = "exact cwd" if chosen.get("_rank") == 2 else "subdir"
        print(f"({len(matches)} sessions match this cwd; picked pid {chosen.get('pid')} ({scope} match, newest).")
        print(" others: " + ", ".join(str(m.get("pid")) for m in matches[1:]) + " — pass one explicitly to switch.)\n")
    emit_report(
        chosen.get("pid"),
        {"cwd": chosen.get("cwd"), "session_id": chosen.get("session_id"), "started_at": chosen.get("started_at")},
        perf=perf,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
