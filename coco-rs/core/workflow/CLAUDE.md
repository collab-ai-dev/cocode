# coco-workflow

Dynamic-workflow **source loading and static validation** (tree-sitter
TypeScript AST). No execution: the QuickJS engine lives in
`core/workflow-runtime`; the Workflow tool (`core/tools/tools/workflow.rs`)
calls this crate first, then hands the validated source to the engine.

## Loading pipeline (`source.rs`)

`resolve_workflow_source(WorkflowSourceInput) -> WorkflowSourceSpec`.
Source-kind precedence: `script_path` > `name` > inline `script`; none →
`MissingSource`. An inline `script` alongside a path/name **overrides the
body but keeps the path as provenance** (`source_path`).

- Named lookup matches the parsed `meta.name` of on-disk scripts — NOT the
  filename stem (a saved `My Build` slugifies to `my-build.js` yet is
  invoked by name). The name never builds a path, so name-based path
  traversal is structurally impossible.
- Lookup dirs, in precedence order: `<coco-config-dir>/workflows` before
  `.claude/workflows`. Model-facing text uses `workflow_dirs_hint()` —
  never hardcode the namespace.
- Registry scans visit files in sorted order for determinism and silently
  skip unreadable / oversize / non-UTF-8 / meta-less files; the
  determinism check is intentionally NOT run during indexing.

## Validation invariants

- `MAX_WORKFLOW_SOURCE_BYTES` (512 KiB) caps every path; file reads use
  `take(limit+1)` — never slurp then check. Source must be valid UTF-8.
- UNC paths are rejected on the RAW input **before** the cwd join: a
  backslash-UNC isn't absolute on Linux, so joining first would hide the
  leading `\\` from the guard.
- `meta.rs`: `export const meta = {...}` must be the FIRST statement in
  the exact shape (const, single declarator named `meta`, object literal),
  evaluated as **pure literals only** — no expressions or `${}`
  substitutions; `__proto__`/`constructor`/`prototype` keys rejected; JS
  escapes cooked (acorn semantics), not JSON-parsed. `parse_workflow_script`
  returns the meta plus `script_body` (source with the meta excised).
- TypeScript-only syntax (annotations, interfaces, enums, decorators,
  `as`/`satisfies`, …) is a `Syntax` error — the body runs verbatim in
  QuickJS, which speaks plain JS.
- The static determinism check (`Date.now` / `Math.random` / argless
  `new Date`) matches by AST name, so `Date["now"]` slips past — the
  runtime shim in `core/workflow-runtime` is the defense-in-depth backstop.

Errors are tier-3 (`WorkflowError`, snafu + `coco-error`): all
`InvalidArguments` except `ReadSource` → `FileNotFound`.
