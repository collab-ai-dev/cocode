# coco-cursor

TUI text cursor with kill ring, word movement, and UTF-8 safe indexing.

## Key Types
| Type | Purpose |
|------|---------|
| `Cursor` | Char-indexed cursor; left/right/home/end/word_left/word_right |
| `KillResult` | Output of a kill op (text + char range) |
| `word_at(text, pos)` | Word span at a char offset (double-click select) |

## Kill Ring
- Max 10 entries (`KILL_RING_MAX`).
- Consecutive `kill_to_end` calls accumulate into the top entry (emacs semantics).
- `yank()` returns the newest entry; `yank_pop()` cycles backward through the ring.
- Any non-kill op resets the accumulation flag.

`Cursor.pos` is a **character index** (not byte); use `to_byte_offset(text)` when slicing strings.
