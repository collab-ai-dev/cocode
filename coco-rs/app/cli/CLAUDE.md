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

## Verification

Run formatting and checks from the workspace root. The focused TUI suite is:

```bash
cargo nextest run -p coco-cli tui_runner --no-fail-fast
```
