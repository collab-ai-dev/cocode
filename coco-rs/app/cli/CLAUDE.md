# coco-cli

Thin `coco` binary composition root. It owns clap dispatch, process signals,
listener startup/shutdown, the interactive TUI command loop, and presentation
adapters. Reusable application behavior belongs in `coco-agent-host`.

## Boundaries

- The `coco_cli` library target contains only the clap schema and process-only
  helpers; application behavior must stay in `coco-agent-host`.
- Keep AppServer handlers, session runtime construction, SDK request semantics,
  and headless operations in `app/agent-host`.
- Keep terminal lifecycle, TUI modal/input policy, command-line subcommand
  selection, and OS process policy here.
- The TUI surface invokes host application operations through `AppServerLocalBridge`; it
  does not build a second generic runner abstraction.

## Allocator (jemalloc)

- `#[global_allocator]` (tikv-jemallocator) lives in `src/main.rs`, gated on
  `all(feature = "jemalloc", not(target_os = "windows"))`; tuning is baked at
  build time via `JEMALLOC_SYS_WITH_MALLOC_CONF` in `.cargo/config.toml`.
- The binary's `jemalloc` feature also enables `coco-tui/jemalloc` →
  `coco-utils-jemalloc` (purge/stats); the two MUST move together — see
  `utils/jemalloc/CLAUDE.md`.

## Verification

Run formatting and checks from the workspace root. The focused TUI suite
(companion tests under `src/tui/*.test.rs`, e.g. the `tui::tests` module) is:

```bash
cargo nextest run -p coco-cli tui --no-fail-fast
```
