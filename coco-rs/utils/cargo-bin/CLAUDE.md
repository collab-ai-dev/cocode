# coco-utils-cargo-bin

Resolve `CARGO_BIN_EXE_*` paths (Cargo or Buck2) to absolute paths for test harnesses.

## Key Types
| Type | Purpose |
|------|---------|
| `cargo_bin(name)` | Absolute path to a built binary target, Cargo + Buck2 aware |
| `find_resource!` | Macro to locate test resources at runtime (Cargo `CARGO_MANIFEST_DIR` or Bazel `RUNFILES_DIR`) |
| `buck_project_root` | Best-effort Buck2 workspace root detection |
| `CargoBinError` | `thiserror` enum for resolution failures |
