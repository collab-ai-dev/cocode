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
- **Local shadows bundled.** A local file wins over a `bundled` workflow of
  the same `meta.name` outright — on the resolve path *and* in
  `list_workflows`, which must agree or the picker shows one script while
  the launcher runs another. `WorkflowOrigin` reports which won.

## Bundled workflows (`bundled.rs`)

`include_str!`'d scripts compiled into the binary; today only
`deep-research`. The registry stores **only the source** and parses
`meta` back out of it on first use (`LazyLock`), so the script literal is
the single source of truth for name / description / `whenToUse` / phases.
A script whose meta fails to parse is dropped rather than panicking;
`bundled.test.rs` is what makes that a build-time failure instead of a
silently-missing workflow.

A bundled workflow resolves with `source_path: None` — it has no on-disk
provenance, and the launcher persists the resolved source into the run's
own directory, which is what resume replays from.

`core/workflow-runtime/tests/deep_research.rs` runs the harness in the
real QuickJS realm against a stub host. That test is the only thing
proving the ~330 lines of embedded JavaScript actually execute (nothing
in the build type-checks a string), so treat it as load-bearing.

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
