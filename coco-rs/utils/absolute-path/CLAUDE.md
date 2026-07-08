# coco-utils-absolute-path

Absolute, normalized path type with home-directory expansion and serde support.
Relative inputs must be resolved with an explicit base path. The default
constructors reject relative paths instead of falling back to the process cwd;
only the explicitly named `current_dir` / `relative_to_current_dir` entrypoints
may read it.

## Key Types
| Type | Purpose |
|------|---------|
| `AbsolutePathBuf` | Guaranteed-absolute `PathBuf` wrapper (Serialize/Deserialize/JsonSchema/ts_rs) |
| `AbsolutePathBufGuard` | Thread-local base path for relative-path deserialization |
| `canonicalize_preserving_symlinks` | Canonicalize but keep logical path through symlinks |
| `test_support::PathExt` / `PathBufExt` | Test helpers (`.abs()`) |
