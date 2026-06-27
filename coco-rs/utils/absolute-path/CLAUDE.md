# coco-utils-absolute-path

Absolute, normalized path type with home-directory expansion and serde support.

## Key Types
| Type | Purpose |
|------|---------|
| `AbsolutePathBuf` | Guaranteed-absolute `PathBuf` wrapper (Serialize/Deserialize/JsonSchema/ts_rs) |
| `AbsolutePathBufGuard` | Thread-local base path for relative-path deserialization |
| `canonicalize_preserving_symlinks` | Canonicalize but keep logical path through symlinks |
| `test_support::PathExt` / `PathBufExt` | Test helpers (`.abs()`) |
