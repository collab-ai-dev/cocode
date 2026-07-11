# coco-maintenance

Shared safety primitives for automated and manual maintenance passes used by
memory and skill-learning workflows. This crate owns mechanism only; prompts,
model selection, scheduling policy, target roots, and allowed-path predicates
remain in the consuming domain crate.

## Key Types

| Type | Purpose |
|------|---------|
| `MaintenanceLock` | Caller-named cross-process PID/mtime lock with atomic create, stale-holder recovery, and last-successful-run cadence. |
| `MaintenanceGuard` | RAII rollback guard; callers must `commit()` a successful automatic run or explicitly roll back a manual run. |
| `MaintenanceLockOutcome` | `Acquired`, `Held`, or local filesystem `Error` outcome without a cross-layer error contract. |
| `write_fence::*` | Fail-closed extraction of write targets from Write/Edit/NotebookEdit and ApplyPatch inputs. |

## Boundaries

- Do not add background scheduling, subagent execution, review prompts, memory
  policy, or skill policy here.
- Write-target extraction identifies paths only. The consumer decides whether
  each path is permitted.
- The lock is a cadence/single-run guard, not a hard mutual-exclusion primitive;
  callers requiring strict exclusion must use an OS advisory lock.
