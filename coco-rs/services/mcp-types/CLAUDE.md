# coco-mcp-types

Auto-generated MCP protocol types (schema 2025-06-18): a single `src/lib.rs`
with a `// @generated` / DO NOT EDIT header.

The one rule: **regenerate, never hand-edit**. Generator:
`./generate_mcp_types.py` (crate root). Guard: `./check_lib_rs.py`, which
reruns the generator with `--check` and diffs against the checked-in `lib.rs`.
The MCP JSON schema it reads is not checked in (default path
`schema/2025-06-18/schema.json`) — fetch it from the modelcontextprotocol repo
or pass its path as the positional argument.

Caution: the checked-in `lib.rs` currently carries deltas the checked-in
generator does not reproduce (the custom `ReadResourceDirectory*` request; no
`ts_rs::TS` derives although the script emits them). Reconcile the generator
first — a blind regenerate would clobber those additions and emit a `ts_rs`
dependency `Cargo.toml` doesn't declare.
