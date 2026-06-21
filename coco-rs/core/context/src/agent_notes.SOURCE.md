# agent_notes.md — provenance

This sibling file documents the source of `agent_notes.md`. Kept
separate because `agent_notes.md` is `include_str!`'d directly into
the subagent system prompt — any header inside that file would be
sent verbatim to the model.

## Source

TS: `claude-code-kim/src/constants/prompts.ts:766-770` — the `notes`
array passed to `enhanceSystemPromptWithEnvDetails` on the subagent
path. TS appends this block immediately after the `<env>...</env>`
section; coco-rs mirrors that placement via the `custom_append` slot
in `coco_context::build_system_prompt`.

## Bullets — line-by-line

1. `cwd reset between bash calls` — TS bullet 1 verbatim.
2. `share file paths (always absolute, never relative)` — TS bullet 2
   verbatim.
3. `MUST avoid using emojis` — TS bullet 3 verbatim.
4. `Do not use a colon before tool calls` — TS bullet 4 verbatim.

## Maintenance rule

When updating `agent_notes.md`:
1. Quote the TS source exactly (no rewording).
2. Update this file's bullet list to match.
3. Regenerate the insta snapshot in `prompt.test.rs`.
