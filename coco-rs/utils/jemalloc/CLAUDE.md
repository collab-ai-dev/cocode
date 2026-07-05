# coco-utils-jemalloc

Safe, feature-gated jemalloc arena purge + stats. The workspace's single home
for the `mallctl` FFI (per the "wrap unsafe deps in their own crate" rule).

## Key API

| Item | Purpose |
|------|---------|
| `ENABLED: bool` | `true` iff compiled with `feature = "jemalloc"` on a non-Windows target. Gate work on it so callers need no `cfg`. |
| `stats_snapshot() -> Option<JemallocStats>` | Epoch-advanced read of `allocated`/`active`/`resident`/`retained`. `None` when disabled. Uses `tikv-jemalloc-ctl` typed wrappers (safe). |
| `purge_all_arenas() -> Result<(), JemallocError>` | `arena.4096.purge` — immediate MADV_DONTNEED of all dirty/muzzy pages. No-op `Ok(())` when disabled. |
| `heap_profiling_available() -> bool` | `opt.prof` — true only when jemalloc's startup conf has `prof:true`; the workspace bakes it in via `JEMALLOC_SYS_WITH_MALLOC_CONF` (.cargo/config.toml), fixed at init. |
| `set_heap_profiling_active(bool)` | Runtime `prof.active` sampling gate. Driven by `tui.performance.heap_profile_enabled`. No-op `Ok(())` when disabled. |
| `dump_heap_profile(&Path)` | `prof.dump` — writes the sampled live heap in `.heap` format for `jeprof` / `jemalloc-pprof`. Only records allocations made while sampling was active. |
| `JemallocStats` | Byte counters; `resident` is the RSS-pressure number a purge shrinks. |
| `JemallocError` | `thiserror` (Tier 2 boundary). |

## Invariants

- **`feature = "jemalloc"` OFF by default.** Off ⇒ no jemalloc dependency,
  stubs only. Turned on transitively by the `coco` binary's `jemalloc` feature,
  which also installs jemalloc as the global allocator. The two MUST move
  together: reading `stats.*` / purging is only meaningful when jemalloc owns
  the process heap.
- **Only `src/imp.rs` may contain `unsafe`.** It holds the raw `mallctl` calls:
  the void purge (the typed ctl write helpers can't express a NULL `newp`,
  which jemalloc requires for `*.purge`) and the `opt.prof` / `prof.active` /
  `prof.dump` ctls (`tikv-jemalloc-ctl` has no typed wrappers for them). Stats
  go through the safe ctl API. Keep it that way.
- **The `jemalloc` feature also enables `tikv-jemalloc-sys/profiling`**
  (`--enable-prof`), so the `prof.*` ctls always exist in jemalloc builds.
  Compile-time capability only — sampling costs nothing until `prof:true` is in
  the startup conf AND a caller activates `prof.active`. `prof:true` is baked
  into libjemalloc via `JEMALLOC_SYS_WITH_MALLOC_CONF` (.cargo/config.toml),
  deliberately NOT the `MALLOC_CONF` env: env leaks to every child process
  (rustc, the long-lived sccache server), whose non-prof toolchain jemalloc
  then spams `Invalid conf pair`.
- **Never on Windows.** jemalloc-sys has no MSVC build; deps are target-gated
  `not(windows)` and `imp` is `cfg(all(feature = "jemalloc", not(windows)))`.
- Resolves to the same `tikv-jemalloc-sys 0.7` the allocator (`tikv-jemallocator`)
  links — no second jemalloc in the tree.

## Why purge-on-demand

macOS jemalloc builds have **no `background_thread`** (excluded for the macho
ABI), so page decay only advances lazily on alloc/free traffic — an idle TUI
never reclaims. An explicit purge at a quiet boundary (e.g. `TurnEnded` in
`coco-tui`) is the manual stand-in. See `app/tui` for the caller.
