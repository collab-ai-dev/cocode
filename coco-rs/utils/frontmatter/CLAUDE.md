# coco-frontmatter

YAML frontmatter parser for markdown files (skills, commands, agents, memories, output styles).

## Key Types
| Type | Purpose |
|------|---------|
| `parse(input)` | Strip leading `---` block and parse to `Frontmatter { data, content, parse_error }` |
| `Frontmatter` | `data: HashMap<String, FrontmatterValue>`, remaining markdown `content`, optional YAML `parse_error` |
| `FrontmatterValue` | `String` / `Bool` / `Int` / `StringList` / `Null` with `as_str` / `as_bool` / `as_string_list` |

Backed by `serde_yml`; supports scalars, sequences, nested mappings, booleans, integers, floats, and nulls.
