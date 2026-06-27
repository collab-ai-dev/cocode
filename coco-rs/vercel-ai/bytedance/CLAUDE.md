# vercel-ai-bytedance

ByteDance Seedance video provider for Vercel AI SDK v4 (via ModelArk API).

## Origin

coco-rs addition covering ByteDance's ModelArk video API (Seedance model family). Not based on any existing SDK spec.

## Key Types

- `ByteDanceProvider`, `ByteDanceProviderSettings`, `bytedance()` (default), `create_bytedance()`
- `ByteDanceVideoModel`, `ByteDanceVideoModelConfig`
- `ByteDanceVideoProviderOptions`, video settings module
- `ByteDanceErrorData`, `ByteDanceFailedResponseHandler`
- `map_resolution` — maps generic video resolutions to Seedance-specific strings

## Conventions

- Reads `ARK_API_KEY` by default.
- Only exposes `provider.video_model(id)` — no language / embedding / image models.
- Supported model: `seedance-1-5-pro-251215` (and successors).
- **`extra_body` deep-merge escape hatch (F1 doctrine).** `provider_options["bytedance"]` extras deep-merge over typed body writes via `merge_json_value`; extras win at final-merge priority. `ByteDanceVideoProviderOptions` carries `#[serde(flatten)] extra` + implements `ExtractExtras`. `null` in extras is a no-op (skips, does NOT unset). Single source of truth: `services/inference/CLAUDE.md` "Design Notes".
