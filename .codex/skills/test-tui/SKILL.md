---
name: test-tui
description: Guide for testing the COCODE TUI interactively
---

You can start and use the COCODE TUI to verify changes.

Important notes:

- Run from the `coco-rs/` workspace directory.
- Start the process interactively with a PTY.
- Always set `RUST_LOG="trace"` when starting the process.
- Pass `--log-file <absolute_temp_dir>/coco.log` so logs are isolated and easy
  to inspect. In COCODE, `-c` means `--continue-session`; it is not a config
  override flag. The daily rotating sink writes a date-suffixed file such as
  `coco.log.2026-07-18`.
- Use the repository's `just coco` recipe, for example:
  `RUST_LOG="trace" just coco --log-file /tmp/cocode-tui-test/coco.log`.
- When sending a test message programmatically, send the text first, then send
  Enter in a separate write. Do not send text and Enter in one burst.
