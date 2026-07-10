# coco-app-runtime

Owns process-, project-, and session-lifetime runtime resources without UI or
transport policy. Process and project ownership have moved here; session
runtime construction is the remaining extraction from `coco-cli`.

## Key Types

| Type | Purpose |
|------|---------|
| `ProcessRuntime` | Process-lifetime owner for the project registry manager. |
| `ProjectRegistry` | Per-project cache with freshness checks and idle eviction. |
| `ProjectServices` | Shared project-rooted config snapshot and plugin catalog. |
| `ProjectConfigSnapshot` | Fingerprint-tracked project settings inputs. |

## Invariants

- Project settings refresh in place. It must not replace project-heavy service
  ownership or change already-built session config snapshots.
- Explicit project reload may replace the whole `ProjectServices` entry because
  it is used for plugin/catalog reload.
- Registry entries remain alive while a session owns their `Arc`; idle eviction
  only removes entries owned solely by the registry after the grace period.
- Project roots are worktree roots, not shared git-common-directory roots.
