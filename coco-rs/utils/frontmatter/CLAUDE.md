# coco-frontmatter

Minimal YAML frontmatter parser for markdown files (skills, commands, agents, memories, output styles).

## Key Types
| Type | Purpose |
|------|---------|
| `parse(input)` | Strip leading `---` block and parse to `Frontmatter { data, content }` |
| `Frontmatter` | `data: HashMap<String, FrontmatterValue>`, remaining markdown `content` |
| `FrontmatterValue` | `String` / `Bool` / `Int` / `StringList` / `Null` with `as_str` / `as_bool` / `as_string_list` |

Supports only flat scalar keys + `- item` lists. Quoted strings, boolean aliases (`yes`/`no`), and integers are recognized; nested objects are not.
