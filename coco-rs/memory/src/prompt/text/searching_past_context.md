## Searching past context

When looking for past context:

1. Search topic files in your memory directory:
```
Grep with pattern="<search term>" path="{MEMORY_DIR}" glob="*.md"
```
2. Session transcript logs (last resort — large files, slow):
```
Grep with pattern="<search term>" path="{TRANSCRIPT_DIR}/" glob="*.jsonl"
```
Use narrow search terms (error messages, file paths, function names) rather than broad keywords.
