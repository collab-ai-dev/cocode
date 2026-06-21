# Builtin Model Instructions

> Status: Implemented
> Scope: `coco-rs/common/config/src/model/instructions.rs`, `coco-rs/common/config/src/model/registry.rs`, `coco-rs/app/cli/src/main.rs`, `coco-rs/app/cli/src/tui_runner.rs`
> Owners: `coco-config` owns model metadata and instruction files. `coco-cli` wires resolved model instructions into `coco-context`.
> References: `codex-rs/models-manager/models.json`, `cocode-rs/common/config/gemini_prompt.md`

## Summary

`coco-rs` now carries per-model builtin instruction bodies in `coco-config` and uses them as the identity block for the system prompt when a resolved `ModelInfo` exposes `base_instructions`.

The GPT prompts are verbatim `base_instructions` values from `codex-rs/models-manager/models.json`, including the `You are Codex` identity. That identity is intentional. Gemini uses the shared prompt from `cocode-rs/common/config/gemini_prompt.md`.

Claude models currently do not define builtin `base_instructions`, so they continue to use the default Coco identity:

```text
You are coco, an AI coding assistant. Be concise and helpful.
```

## Architecture

Instruction text is model metadata, not prompt assembly logic:

```text
coco-config
  common/config/instructions/*.md
  common/config/src/model/instructions.rs
      include_str! constants
      builtin_base_instructions(model_id)

  common/config/src/model/registry.rs
      L0 builtin model metadata
      L1 ~/.coco/models.json overlay
      L2 providers.<name>.models.<id> overlay
      base_instructions_file normalization

coco-context
  core/context/src/prompt.rs
      build_system_prompt(identity, ...)
      no ModelInfo / provider / registry dependency

coco-cli
  app/cli/src/main.rs
      build_system_prompt_for_model(...)
      resolves ModelRegistry entry and passes identity to coco-context

  app/cli/src/tui_runner.rs
      uses the same helper as headless and SDK paths
```

`coco-config` may depend on `coco-types` and config-local utilities. It must not depend on `coco-context`, `coco-cli`, `coco-query`, or `coco-inference`.

`coco-context` stays generic: it assembles a prompt from already-rendered strings and context inputs. It does not resolve model registries or provider config.

`coco-cli` is the composition layer because it has `RuntimeConfig`, `ApiClient`, cwd, and access to `coco-context`.

## Priority

Resolved model instructions follow the existing config priority:

```text
L2 providers.<name>.models.<id>        highest
L1 ~/.coco/models.json
L0 builtin defaults                    lowest
```

`base_instructions_file` wins over inline `base_instructions` within the same merged entry. During registry construction, user catalog entries are normalized so `ModelRegistry.user_catalog` stores resolved inline `base_instructions` and no unresolved `base_instructions_file`. This matters for lazy `ModelRegistry::try_resolve()` lookups where a provider has no explicit model entry.

## Builtin Files

```text
coco-rs/common/config/instructions/
  gpt5_4_prompt.md
  gpt5_5_prompt.md
  gpt5_3_codex_prompt.md
  gemini_prompt.md
```

Current builtin mappings:

| Model id | Instruction source |
|---|---|
| `gpt-5-4` | `gpt5_4_prompt.md` |
| `gpt-5-5` | `gpt5_5_prompt.md` |
| `gpt-5-3-codex` | `gpt5_3_codex_prompt.md` |
| `gemini-2.5-pro` | `gemini_prompt.md` |
| `gemini-2.5-flash` | `gemini_prompt.md` |

Adding a future model-specific prompt should require:

1. Add a Markdown file under `common/config/instructions/`.
2. Add one `include_str!` constant and mapping entry in `model/instructions.rs`.
3. Add or update the builtin `PartialModelInfo` entry in `model/registry.rs`.

## Non-Goals

Cache breakpoint threading remains out of scope. `coco-cli` still passes a flat `String` through `QueryEngineConfig::system_prompt`.

Personality-template expansion from `codex-rs` is not ported. Coco uses the `base_instructions` field only.

Provider routing is not an instruction axis. A model's base instructions are provider-agnostic metadata and apply regardless of which provider instance serves the model.

## Verification

Focused coverage:

```bash
cd coco-rs
just test-crate coco-config
just test-crate coco-cli
```

Full pre-commit coverage remains:

```bash
cd coco-rs
just fmt
just pre-commit
```
