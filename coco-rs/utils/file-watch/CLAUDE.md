# coco-file-watch

Throttled, event-coalescing file watcher over the `notify` crate.

## Key Types
| Type | Purpose |
|------|---------|
| `FileWatcher<E>` | Generic watcher emitting typed domain events `E` via `broadcast::Sender` |
| `FileWatcherBuilder<E>` | Configures `throttle_interval` (default 1s) + `channel_capacity` (default 128); `build(classify, merge)` or `build_noop` |
| `ThrottledPaths` | Stand-alone coalescer: accumulate paths, emit at most once per interval |
| `RecursiveMode` | Re-exported from `notify` |

`classify: Fn(&notify::Event) -> Option<E>` maps raw events to domain events; `merge: Fn(E, E) -> E` coalesces bursts during the throttle window.

## Read/metadata pre-filter (self-feed-loop guard)

`Access(_)` (read-open / close) and `Modify(Metadata(_))` (atime / permissions)
events are dropped by `is_content_change` **before** `classify` runs — the loop
only surfaces content/name/existence changes (`Create` / `Modify(Data|Name)` /
`Remove`, plus the coarse `Any`/`Other` fallbacks). This is load-bearing:
notify's inotify backend watches with a mask including `IN_OPEN` / `IN_ATTRIB`,
so a consumer whose reaction re-reads the watched file would otherwise self-feed
an unbounded loop (the reaction's `open()` re-fires the watch; under a
`strictatime` mount the atime bump does the same). Do **not** rely on a
`classify` closure to see read or metadata-only events.
