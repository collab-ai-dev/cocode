# coco-utils-jemalloc

Safe, feature-gated jemalloc arena purge + stats. The workspace's single home
for the `mallctl` FFI (per the "wrap unsafe deps in their own crate" rule).

## Key API

| Item | Purpose |
|------|---------|
| `ENABLED: bool` | `true` iff compiled with `feature = "jemalloc"` on a non-Windows target. Gate work on it so callers need no `cfg`. |
| `stats_snapshot() -> Option<JemallocStats>` | Epoch-advanced read of `allocated`/`active`/`resident`/`retained`. `None` when disabled. Uses `tikv-jemalloc-ctl` typed wrappers (safe). |
| `purge_all_arenas() -> Result<(), JemallocError>` | `arena.4096.purge` — immediate MADV_DONTNEED of all dirty/muzzy pages. No-op `Ok(())` when disabled. |
| `JemallocStats` | Byte counters; `resident` is the RSS-pressure number a purge shrinks. |
| `JemallocError` | `thiserror` (Tier 2 boundary). |

## Invariants

- **`feature = "jemalloc"` OFF by default.** Off ⇒ no jemalloc dependency,
  stubs only. Turned on transitively by the `coco` binary's `jemalloc` feature,
  which also installs jemalloc as the global allocator. The two MUST move
  together: reading `stats.*` / purging is only meaningful when jemalloc owns
  the process heap.
- **Only `src/imp.rs` may contain `unsafe`.** It holds exactly one raw
  `mallctl` call (the void purge — the typed ctl write helpers can't express a
  NULL `newp`, which jemalloc requires for `*.purge`). Stats go through the safe
  ctl API. Keep it that way.
- **Never on Windows.** jemalloc-sys has no MSVC build; deps are target-gated
  `not(windows)` and `imp` is `cfg(all(feature = "jemalloc", not(windows)))`.
- Resolves to the same `tikv-jemalloc-sys 0.7` the allocator (`tikv-jemallocator`)
  links — no second jemalloc in the tree.

## Why purge-on-demand

macOS jemalloc builds have **no `background_thread`** (excluded for the macho
ABI), so page decay only advances lazily on alloc/free traffic — an idle TUI
never reclaims. An explicit purge at a quiet boundary (e.g. `TurnEnded` in
`coco-tui`) is the manual stand-in. See `app/tui` for the caller.
