# coco-apply-patch

Unified diff/patch application with fuzzy matching.

## Source

Ported from `codex-rs/apply-patch` (NOT from claude-code TS). Treat `codex-rs` as upstream. Lives under `exec/` (not `utils/`) because it performs filesystem side-effects alongside `coco-shell`, `coco-sandbox`, and `coco-exec-server`.

## Key Types

- `ApplyPatchError` — error enum (`ParseError` / `IoError` / `ComputeReplacements` / `ImplicitInvocation`)
- `Hunk`, `ParseError`, `parse_patch` — parser
- `maybe_parse_apply_patch_verified` — invocation detector
- `APPLY_PATCH_TOOL_INSTRUCTIONS` — `const &str` (GPT-4.1 tool instructions)
- `COCO_CORE_APPLY_PATCH_ARG1` — self-invocation arg1 flag for the standalone binary

## API Shape

Public entry points are async and take `cwd: &AbsolutePathBuf` + `fs: &dyn ExecutorFileSystem` (from `coco-exec-server`). Relative paths in patches resolve against `cwd`.
