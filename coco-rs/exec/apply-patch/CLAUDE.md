# coco-apply-patch

Codex-compatible patch parsing, verification, application, and standalone CLI.
The implementation is synchronized with OpenAI Codex `279b93242cfe`; cocode
names, the local `PathUri::from_path` constructor, and companion test-file
splits are mechanical port adaptations. Upstream tests and fixtures are
retained in full.

## Architecture

- `parser` + `streaming_parser` parse strict, lenient shell, and incremental
  patch input.
- `file_update` + `text_file` derive updates in either `NormalizeToLf` or
  `PreserveLineEndings` mode.
- `invocation` recognizes direct and shell-wrapped calls and produces typed
  `ApplyPatchAction` values.
- The public `apply_patch` path mirrors Codex ordered-commit semantics and
  reports committed or potentially committed work through `AppliedPatchDelta`.
- `path_effects` and `prepared_patch` are cocode integration layers. They
  resolve every source and move destination once, reject aliases, derive all
  content once, bind the plan to its filesystem/sandbox, commit through the
  validated canonical paths, and detect stale targets. Failed commits are not
  rolled back over potentially concurrent external edits; instead they return
  the exact committed prefix, or an explicitly inexact delta when transport or
  filesystem errors make the resulting state unknowable.
- The local Linux executor implements checked writes with a private staged
  inode and atomic exchange, checked removals with atomic capture, and
  no-follow handle-relative traversal. Security metadata is copied before
  publication. A race at the linearization point preserves the displaced entry
  in a private adjacent recovery directory and returns unknown state; it never
  performs a path-based rollback. Platforms without equivalent primitives fail
  checked mutations closed.

The cocode built-in tool always selects `PreserveLineEndings`; the public/CLI
default stays Codex-compatible and the rollout environment variable remains
supported. Do not introduce local parser, update, rendering, or CLI behavior
that diverges from the synchronized Codex revision.

All filesystem APIs use `PathUri`, `ExecutorFileSystem`, and an optional
`FileSystemSandboxContext`; do not project remote paths through host-native
path resolution.

## Testing

Keep Codex parity tests and scenario fixtures unchanged except for cocode crate
names and local constructor spelling. Add cocode hardening tests in companion
`*.test.rs` files. Run `just test-crate coco-apply-patch` after every change.
