# coco-config-reload

Hot-reload loop wiring `coco-file-watch` → `coco-config::RuntimePublisher`.

## Why a separate crate

`coco-config` is L1 (depends only on `coco-types` + `coco-error` + utils).
Wiring a `tokio::spawn` reload loop in L1 forces every L1 consumer (including
the leaf-utility `coco-error`) to transitively pull Tokio's runtime. This crate
sits at L2 alongside `coco-inference`, holds the runtime requirement, and lets
`coco-config` stay layer-pure.

## Key Types

| Type | Purpose |
|------|---------|
| `RuntimeReloader` | Spawned task that watches settings + catalog paths, rebuilds `RuntimeConfig`, publishes via `RuntimePublisher` |
| `ConfigChange` | Domain event emitted on file change |
| `ReloadOptions` | Builder for `RuntimeReloader::spawn` (cwd, flag_settings, env_factory, overrides, debounce) |

## Drop semantics

`RuntimeReloader::drop` aborts the spawned task explicitly — no longer relies
on `_watcher` field-drop ordering for termination.

## Watch strategy

Catalog paths (`providers.json`, `models.json`) may not exist at startup.
The watcher subscribes to the **parent directory** non-recursively and filters
events by the exact path in the classify closure, so a first-time `touch` of
`~/.coco/providers.json` triggers a rebuild.
