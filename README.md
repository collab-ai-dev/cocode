# coco

Independent repository for coco.

## Contents

- `coco-rs/`: Rust workspace for the CLI, TUI, providers, services, tools, and shared crates.
- `coco-cli/`: npm package wrapper for the coco CLI.
- `coco-sdk/`: SDK schemas and language bindings.
- `docs/`: coco documentation and architecture notes.
- `.claude/` and `.codex/`: project-local agent instructions, scripts, and skills.

## Development

Most Rust development happens in `coco-rs/`.

```bash
cd coco-rs
just fmt-check
just quick-check
```

Read `CLAUDE.md` before making code changes; it contains the repository
conventions, build commands, and architecture guide.
