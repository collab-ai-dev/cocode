#!/usr/bin/env python3
"""Measure coco TUI startup: time-to-first-frame and idle RSS.

Spawns the binary on a PTY, records the wall time until the first byte of
screen paint arrives, lets the UI settle, then samples RSS/physical footprint.
"""
import os, pty, select, signal, subprocess, sys, time, statistics

BIN = sys.argv[1] if len(sys.argv) > 1 else "coco-rs/target/release/coco"
RUNS = int(sys.argv[2]) if len(sys.argv) > 2 else 5
SETTLE = float(sys.argv[3]) if len(sys.argv) > 3 else 3.0


def rss_kb(pid):
    """Resident set size in KB for pid + all descendants (ps rss is KB on macOS)."""
    try:
        out = subprocess.run(["ps", "-o", "pid=,ppid=,rss=", "-A"],
                             capture_output=True, text=True, timeout=10).stdout
    except Exception:
        return None
    kids, rss = {}, {}
    for line in out.splitlines():
        parts = line.split()
        if len(parts) != 3:
            continue
        p, pp, r = int(parts[0]), int(parts[1]), int(parts[2])
        kids.setdefault(pp, []).append(p)
        rss[p] = r
    total, stack = 0, [pid]
    seen = set()
    while stack:
        p = stack.pop()
        if p in seen or p not in rss:
            continue
        seen.add(p)
        total += rss[p]
        stack.extend(kids.get(p, []))
    return total if seen else None


def footprint_mb(pid):
    """macOS phys_footprint via footprint(1) if available."""
    try:
        out = subprocess.run(["footprint", "-p", str(pid)],
                             capture_output=True, text=True, timeout=15).stdout
        for line in out.splitlines():
            if "phys_footprint" in line.lower():
                return line.strip()
    except Exception:
        pass
    return None


def one_run():
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.environ["LINES"] = "40"
        os.environ["COLUMNS"] = "120"
        try:
            os.execv(BIN, [BIN])
        except Exception:
            os._exit(127)
    t0 = time.monotonic()
    first_frame = None
    deadline = t0 + 20
    buf = b""
    while time.monotonic() < deadline:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            buf += chunk
            if first_frame is None and len(buf) > 64:
                first_frame = time.monotonic() - t0
        if first_frame is not None and time.monotonic() - t0 > SETTLE:
            break
    mem = rss_kb(pid)
    fp = footprint_mb(pid)
    try:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    except Exception:
        pass
    try:
        os.close(fd)
    except Exception:
        pass
    return first_frame, mem, fp, buf[:200]


def main():
    ffs, mems = [], []
    for i in range(RUNS):
        ff, mem, fp, head = one_run()
        print(f"run {i+1}: first_frame={ff*1000:.1f}ms " if ff else f"run {i+1}: first_frame=NONE ", end="")
        print(f"rss={mem/1024:.1f}MB" if mem else "rss=NONE", end="")
        print(f" | {fp}" if fp else "")
        if i == 0:
            print(f"   head bytes: {head!r}")
        if ff:
            ffs.append(ff * 1000)
        if mem:
            mems.append(mem / 1024)
        time.sleep(0.5)
    print("-" * 60)
    if ffs:
        print(f"time-to-first-frame: median {statistics.median(ffs):.1f}ms  min {min(ffs):.1f}  max {max(ffs):.1f}  (n={len(ffs)})")
    if mems:
        print(f"idle RSS:            median {statistics.median(mems):.1f}MB  min {min(mems):.1f}  max {max(mems):.1f}  (n={len(mems)})")


main()
