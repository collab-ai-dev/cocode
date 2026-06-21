# coco-output-styles

Output style catalog, dir/plugin loading, and active-style resolution.
Covers built-in styles (`default` / `Explanatory` / `Learning`), project +
user `.coco/output-styles/*.md` discovery, plugin-sourced styles, system-prompt
injection, and per-turn reminder generation.

## Key Types

| Type | Purpose |
|------|---------|
| `OutputStyleConfig` | `name`, `description`, `prompt`, `source`, `keep_coding_instructions`, `force_for_plugin` |
| `OutputStyleSource` | `BuiltIn` / `Plugin` / `UserSettings` / `ProjectSettings` / `PolicySettings`. `priority()` drives override ordering |
| `Aggregated` | Name → config catalog produced by `aggregate(..)`. `default` sentinel intentionally absent (mapped to `None`) |
| `OutputStyleManager` | Resolved catalog + active config. Built once at session bootstrap, cheap to clone, threaded into prompt builder + SDK init + reminder pipeline |
| `OutputStyleManagerBuilder` | Fluent builder used by the CLI — accepts settings name, user dir, project dirs, managed dir, plugin sources |
| `PluginOutputStyleSource` | Minimal data per plugin (`plugin_name`, `default_dir`, `extra_paths`) so this crate doesn't depend on plugin lifecycle types |
| `ForceForPluginVerdict` | `None` or `Selected { winner, competing }`. Lets the manager log multi-force conflicts |

## Module Layout

```
src/
├── lib.rs            re-exports + crate-local Result
├── error.rs          OutputStylesError (thiserror, Tier 2 boundary)
├── catalog.rs        OutputStyleConfig + OutputStyleSource
├── builtin.rs        DEFAULT/EXPLANATORY/LEARNING constants + builtin_styles()
├── dir_loader.rs     load_dir_styles + shared frontmatter extraction
├── plugin_loader.rs  load_plugin_output_styles + PluginOutputStyleSource
├── resolver.rs       aggregate() + resolve_active_style() + ForceForPluginVerdict
└── manager.rs        OutputStyleManager + builder
```

## Key Invariants

- **Built-ins ship verbatim.** The Explanatory and Learning prompt
  bodies are reproduced character-for-character (Unicode `★` and `●`
  substituted for the original `figures.star` / `figures.bullet`). Built-in
  tests assert presence of every section header so a regression rewrites
  the body intentionally, not by accident.
- **`default` is `None`, not an entry.** `default` is absent from
  `Aggregated.by_name` and the `None` mapping is baked into
  `Aggregated::get`. Callers that need the literal sentinel string
  (SDK init, picker default option) read `DEFAULT_OUTPUT_STYLE_NAME`.
- **Aggregation order**: `built-in < plugin < user < project < managed`.
  Implemented numerically by `OutputStyleSource::priority()` with
  `>=` overwrite so a later layer with the same priority replaces an
  earlier one of the same priority.
- **Plugin styles can't carry `keep-coding-instructions`.** The field is
  explicitly cleared after parsing. Plugin authors who need that escape hatch
  should ship a dir-style instead.
- **Force-for-plugin tie-break follows catalog order.** Multiple plugins
  all setting `force-for-plugin: true` is a misconfiguration; we log the
  full list and pick the first loaded style.

## What this crate does NOT own

- **Per-turn `<system-reminder>` injection** — that's
  `core/system-reminder/generators/output_style.rs`. It reads the
  active style name from `OutputStyleSnapshot` (set on
  `SessionBootstrap` by the CLI) and renders the reminder template
  (`{name} output style is active. Remember to...`).
- **System prompt assembly** — the CLI passes the active
  `OutputStyleConfig` into `coco_context::build_system_prompt`, which
  injects `# Output Style: <name>\n<prompt>` after the identity
  block. This crate has no direct dependency on `coco_context` to
  preserve the `core/` → `root/` direction.
- **Settings reading.** Callers pass the resolved
  `Settings.output_style` value as a string. This crate doesn't
  depend on `coco_config`.
- **`/output-style` slash command.** The deprecated stub lives in
  `commands/src/implementations.rs` (`output_style_handler`).
