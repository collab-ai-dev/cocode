# coco-utils-image

Image loading, resize-to-fit, and re-encoding for prompt attachments (PNG / JPEG / GIF / WebP).

## Key Types

| Type | Purpose |
|------|---------|
| `EncodedImage` | `bytes`, `mime`, current + original `width`/`height`; `into_data_url()` |
| `PromptImageMode` | `ResizeToFit` / `Original` |
| `load_for_prompt_bytes(path, bytes, mode)` | Main entry; decodes, optionally resizes to `MAX_WIDTH × MAX_HEIGHT` (2048×768), re-encodes, and caches |
| `normalize_image_bytes(bytes, mime)` | Convenience wrapper for clipboard bytes |
| `MAX_WIDTH`, `MAX_HEIGHT` | Resize caps |
| `error::ImageProcessingError` | Decode / encode failure variants |

Uses a 32-entry SHA-1-keyed `BlockingLruCache` (from `coco-utils-cache`) so repeated loads of the same bytes are free. Source bytes are pass-through only for PNG/JPEG/WebP; anything else is re-encoded to PNG.
