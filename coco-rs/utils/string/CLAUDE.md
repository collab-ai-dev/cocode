# coco-utils-string

String truncation, boundary-safe slicing, and small formatting helpers.

## Key Functions

| Function | Purpose |
|----------|---------|
| `take_bytes_at_char_boundary` | Prefix at byte budget, UTF-8 safe |
| `take_last_bytes_at_char_boundary` | Suffix at byte budget, UTF-8 safe |
| `truncate_str` | Prefix with `...` when over limit |
| `truncate_for_log` | `[N chars] prefix...` for debug logging |
| `sanitize_metric_tag_value` | Enforce `[A-Za-z0-9._/-]`, trim `_`, cap 256 |
| `find_uuids` | Extract all UUIDs via regex |
| `normalize_markdown_hash_location_suffix` | `#L10C5-L12` → `:10:5-12` |
| `bytes_to_hex` | Lowercase hex encoding |
