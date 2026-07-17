# coco-utils-path-uri

Typed, immutable `file:` URIs (`PathUri`) with host-independent lexical path ops —
a Windows-shaped URI behaves as Windows on every host (VS Code resource semantics).

## Key API

| Item | Purpose |
|------|---------|
| `PathUri::parse` / `from_abs_path` / `from_path` | Construct from a `file:` string or absolute host path; only the `file:` scheme, no query/fragment/port/credentials |
| `to_abs_path` | Back to `AbsolutePathBuf`; rejects foreign conventions rather than mis-projecting them onto the host |
| `basename` / `parent` / `ancestors` / `join` | Lexical segment ops; `join` cannot escape the POSIX root, Windows drive, or UNC share |
| `infer_path_convention` / `PathConvention` | Posix vs Windows from URI spelling (authority or `file:///C:/…` drive segment ⇒ Windows) |
| `LegacyAppPathString` | Raw-path compat at the app-server API boundary; serde-transparent string that converts to/from `PathUri` under an explicit convention |

## Gotchas

- Serde form is the canonical URI string; deserialization requires a valid `file:` URI — never a native path.
- Unrepresentable native paths (null bytes, Windows verbatim/device namespaces, non-URL-host UNC names) become the `BAD_PATH_URI_PREFIX` sentinel `file:///%00/bad/path/<base64>` — opaque to `basename`/`parent`/`join`, but `to_abs_path` round-trips losslessly on the origin host.
- Internal code cannot build `LegacyAppPathString` from a bare `String` (serde-only construction) — convert through `PathUri` or `AbsolutePathBuf` instead.
