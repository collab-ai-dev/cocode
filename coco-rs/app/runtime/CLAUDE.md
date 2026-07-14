# coco-app-runtime

Owns transport-independent process/project resources, workspace paths, and
session bootstrap contracts. Application-session composition stays in
`coco-agent-host` because it integrates QueryEngine, tasks, MCP, hooks, and
persistence rather than defining a lower-level resource primitive.

## Key Types

| Type | Purpose |
|------|---------|
| `ProcessRuntime` | Process-lifetime owner for the project registry manager and its explicit background-task shutdown policy. |
| `ProjectRegistry` | Per-project cache with freshness checks and idle eviction. |
| `ProjectServices` | Shared project-rooted config snapshot and plugin catalog. |
| `ProjectConfigSnapshot` | Fingerprint-tracked project settings inputs. |
| `SessionWorkspace` (`workspace`) | Per-session path anchors: cwd, resolved `project_root` (the `ProjectServices` cache key), and `ProjectPaths` storage. |
| `resolve_project_root` / `git_root_for` / `project_paths` / `runtime_paths` / `settings_roots_for_cwd` | Path/project-root resolution shared by session bootstrap and the project registry (single source of the worktree-root derivation). `coco-agent-host::paths` exposes the application-facing helpers. |
| `SessionRuntimeBootstrap` (`bootstrap`) | The fully-resolved, config-derived inputs for constructing one session runtime (output of the per-session fold). |
| `BootstrapSource` / `SessionRuntimeBootstrapBuild` / `BootstrapError` / `PrebuiltBootstrapSource` | The fold seam: a trait producing the bundle + reloader, its result, a Tier-3 error, and the pre-built-bundle impl. The `AgentHostOptions`-backed production fold lives in `coco-agent-host`. |

## Invariants

- A stale project settings file (fingerprint mismatch) rebuilds the whole
  `ProjectServices` entry on the next `get_or_load`, so a later session in the
  same project observes current settings AND a freshly-loaded plugin catalog.
  This never mutates an already-built session's config snapshot: sessions hold
  their own `Arc<ProjectServices>` from their fold and keep it.
- Explicit project reload also replaces the whole `ProjectServices` entry
  (plugin/catalog reload).
- Registry entries remain alive while a session owns their `Arc`; idle eviction
  only removes entries owned solely by the registry after the grace period.
- Project roots are worktree roots, not shared git-common-directory roots.
