# `coco-output-styles` — output style catalog and resolution

> Crate path: `coco-rs/output-styles/`
> CLAUDE.md: [`coco-rs/output-styles/CLAUDE.md`](../../coco-rs/output-styles/CLAUDE.md)

## Goal

TS-mirror for the entire output-styles surface: catalog definitions,
custom-dir + plugin loaders, force-for-plugin resolution, and the
system-prompt + per-turn-reminder integration points.

## TS source map

| Concern | TS file | coco-rs file |
|---|---|---|
| Catalog type | `constants/outputStyles.ts:11-23` | `output-styles/src/catalog.rs` |
| Built-in styles (`Explanatory`, `Learning`) | `constants/outputStyles.ts:39-135` | `output-styles/src/builtin.rs` |
| Custom dir loader (project + user) | `outputStyles/loadOutputStylesDir.ts` | `output-styles/src/dir_loader.rs` |
| Plugin loader (with `force-for-plugin`) | `utils/plugins/loadPluginOutputStyles.ts` | `output-styles/src/plugin_loader.rs` |
| Aggregation + active-style resolution | `constants/outputStyles.ts:137-211` | `output-styles/src/resolver.rs` |
| CLI manager | (per-call functions) | `output-styles/src/manager.rs` |
| System-prompt injection | `constants/prompts.ts::getOutputStyleSection` | `core/context/src/prompt.rs::OutputStyleSection` |
| Per-turn reminder template | `utils/messages.ts:3797`, `utils/attachments.ts:1597` | `core/system-reminder/src/generators/output_style.rs` |
| `/output-style` deprecation stub | `commands/output-style/output-style.tsx` | `commands/src/implementations.rs` (`output_style_handler`) |
| SDK `output_style` + `available_output_styles` | `entrypoints/sdk/controlSchemas.ts` | `app/cli/src/sdk_server/cli_bootstrap.rs` |

## Resolution chain

```
~/.coco/output-styles/*.md   ┐
<cwd>/.claude/output-styles  │
managed `.claude/output-…    │  load_dir_styles(...)
                             │
<plugin>/output-styles/*.md  │  load_plugin_output_styles(...)
manifest.outputStyles[..]    │
                             ▼
                aggregate(dir_groups, plugin_styles)
                    │
                    ▼
        resolve_active_style(catalog, settings.output_style)
            │                            │
            │ force-for-plugin? ─────► winner
            │
            ▼ otherwise look up settings name
        Some(OutputStyleConfig) | None (default sentinel)
                    │
                    ▼
        EngineResources.output_style_manager
                    │
            ┌───────┼─────────────────────────────────┐
            ▼       ▼                                  ▼
   build_system_prompt_for_model       SessionBootstrap.output_style (name)
   (#`# Output Style: …` block)        ─► OutputStyleSnapshot
                                       ─► per-turn reminder
                                       ─► SDK init wire field
                                       CliInitializeBootstrap
                                       ─► available_output_styles
```

## Source priority

Mirrors TS aggregation: built-in < plugin < user < project < managed.
A later layer with the same priority overwrites an earlier one. The
priority is encoded numerically on `OutputStyleSource::priority()` so
the resolver can apply it without referencing the variant order.

## Force-for-plugin

A plugin style with `force-for-plugin: true` overrides
`settings.output_style`. If multiple plugins set it, the alphabetically
first one wins (deterministic across machines) and the rest are logged
as competitors. TS uses insertion order; coco-rs picks alphabetical so
the same `~/.coco` config produces the same active style regardless of
plugin install timing.

## TS divergence (intentional)

| Behavior | TS | coco-rs |
|---|---|---|
| User dir | `~/.claude/output-styles` | `~/.coco/output-styles` (consistent with `~/.coco/skills` / `~/.coco/agents`) |
| Project tree walk | walks every ancestor up to git root | single `<cwd>/.claude/output-styles` (matches `get_skill_paths` and agent search paths) |
| Multi-force tie-break | first by insertion order | first alphabetically |
| Intro phrasing toggle | `getSimpleIntroSection` swaps wording when style active | static identity string, no swap (callers customize identity if needed) |
| `Doing tasks` skip | TS suppresses when `keepCodingInstructions: false` | flag is surfaced on `OutputStyleSection` for future use; current builder leaves the static identity untouched |

These deliberately match the rest of coco-rs's loader conventions and
keep the system-prompt cache prefix stable across sessions.

## Wire shape

### SDK `system/init`

```json
{
  "type": "system",
  "subtype": "init",
  "output_style": "Explanatory",
  "available_output_styles": ["default", "Explanatory", "Learning", "concise", "alpha:plugin-style"]
}
```

The CLI prepends `default` to the catalog names so clients can
"select" the no-style sentinel via the picker, matching TS.

### Per-turn `<system-reminder>`

```
<system-reminder>
Explanatory output style is active. Remember to follow the specific guidelines for this style.
</system-reminder>
```

Emitted when the active style is non-`default`. The generator reads
`OutputStyleSnapshot.name` set by the engine from
`SessionBootstrap.output_style` (the resolved name string).

## What this design does NOT cover

- **Live settings reload.** `OutputStyleManager` is built once at
  session bootstrap. A future hot-reload pass would need to rebuild it
  on `Settings.output_style` change; the current cache invalidates
  on `/reload` because the whole `EngineResources` is rebuilt.
- **TS GrowthBook / `USER_TYPE=ant` gates.** Not ported per
  `CLAUDE.md` "no ant gates" rule.
