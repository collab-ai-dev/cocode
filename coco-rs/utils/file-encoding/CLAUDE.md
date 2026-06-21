# coco-file-encoding

Detect and round-trip file encoding (UTF-8 ± BOM, UTF-16LE/BE) and line endings (LF/CRLF/CR).

## Key Types
| Type | Purpose |
|------|---------|
| `Encoding` | `Utf8` / `Utf8WithBom` / `Utf16Le` / `Utf16Be` with `encode`/`decode`/`bom` |
| `LineEnding` | `Lf` / `CrLf` / `Cr` |
| `detect_encoding`, `detect_line_ending` | Byte/content sniffers (BOM first, CRLF heuristic) |
| `read_with_format` / `write_with_format` | Sync + async round-trip helpers |
| `preserve_trailing_newline`, `normalize_line_endings` | Reduce spurious diffs on edit |
