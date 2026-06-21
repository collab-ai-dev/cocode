# coco-symbol-search

Tree-sitter-based symbol extraction + fuzzy search for the TUI `@#SymbolName` mention feature.

## Key Types
- `SymbolIndex` — index over indexed files (via `index` module)
- `SymbolKind` — `Function | Method | Class | Struct | Interface | Type | Enum | Module | Constant | Other`; `from_syntax_type(&str)` and `label()` helpers
- `SymbolSearchResult` — name, kind, file path, line, fuzzy score, match indices
- Submodules: `extractor`, `index`, `languages`, `watcher`
