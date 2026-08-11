# coco-utils-common

Small cross-crate helpers with zero internal deps.

## Key Modules
| Module | Exports |
|--------|---------|
| `coco_home` | `find_coco_home`, `COCO_CONFIG_DIR_ENV`, `COCO_CONFIG_DIR_NAME` — resolves config home / override via `COCO_CONFIG_DIR` |
| `elapsed` | `format_duration`, `format_elapsed` — human-readable durations |
| `format_env_display` | `format_env_display` — redacted env-var printing |
| `fuzzy_match` | `fuzzy_match`, `fuzzy_indices` — lightweight fuzzy scoring |
| `logging` | `LoggingConfig`, `TimezoneConfig`, `ConfigurableTimer`, `build_env_filter` — `tracing-subscriber` bootstrap |
| `fs` | `open_regular` / `read_regular(_async)` descriptor-validated reads; `replace_regular_atomic` same-directory atomic replace + exact verification proof |

`replace_regular_atomic` rejects final symlinks and non-regular targets, breaks
hard links intentionally, preserves existing permission mode bits, fsyncs the
replacement and (on Unix) parent directory, then verifies bytes by streaming
from a newly validated descriptor. New Unix files use `0o666` filtered through
the process umask. Ownership, ACLs, and extended attributes are not copied to
the new inode; callers must not use it where those metadata are part of the
file's contract.
