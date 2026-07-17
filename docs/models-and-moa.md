# Models, roles, and Mixture of Agents

cocode does not have "a model". It has eight **roles**, each bound to a
`<provider>/<model_id>` selection with its own fallback chain, retry policy, and
reasoning effort. This page covers how roles resolve, and how to point one at a
Mixture of Agents preset.

## Model roles

Only `main` is required. Every other role falls back to whatever `main`
resolves to, so a one-line config is a complete config.

<!-- BEGIN GENERATED: model-roles -->

| Role | Settings key | Used for |
| --- | --- | --- |
| Main | `models.main` | The primary conversation and coding agent. Required. |
| Plan | `models.plan` | Plan mode |
| Fast | `models.fast` | Cheap helper calls, such as title generation |
| Explore | `models.explore` | Read-only codebase exploration |
| Review | `models.review` | Review-oriented subagent work |
| Subagent | `models.subagent` | Generic spawned subagents |
| Memory | `models.memory` | Memory extraction and recall |
| HookAgent | `models.hook_agent` | Agents invoked by hooks |

<!-- END GENERATED: model-roles -->

The point of roles is that you bind a *job* to whichever model is currently best
at it, and change your mind in one place. Custom subagents declare a
`modelRole` rather than a concrete model, so they follow your routing instead of
pinning a vendor.

## Selecting a model

There is no default model. If `main` is unset, cocode refuses to start:

```text
no Main model configured: set `models.main` in settings.json, pass
`--models.main <provider>/<model_id>`, or set COCO_MODEL_MAIN=<provider>/<model_id>
```

Those are exactly the three ways to set it, in precedence order — the CLI flag
beats the environment variable, which beats `settings.json`:

```bash
cocode-cli --models.main anthropic/claude-sonnet-4-6      # this run only
export COCO_MODEL_MAIN=anthropic/claude-sonnet-4-6         # this shell
```

```jsonc
// ~/.cocode/settings.json — the durable form
{
  "models": {
    "main": "anthropic/claude-sonnet-4-6"
  }
}
```

Only `main` has a CLI flag and an environment variable. Every other role is
configured in settings.

### The `/model` command

Bare `/model` opens a picker with a role pill, so you can rebind any role
interactively; it shows each provider's status and why an unusable one is
unavailable. With an argument it validates the selection and persists it to
`~/.cocode/settings.json`:

```
/model anthropic/claude-opus-4-7          # rebinds main
/model plan anthropic/claude-opus-4-7     # rebinds a specific role
/model moa/default                        # binds main to a MoA preset
```

## Fallback chains and policies

A role can be a bare string, or an object with a fallback chain. When the
primary is exhausted or overloaded, cocode walks the chain:

```jsonc
{
  "models": {
    "main": {
      "primary": "anthropic/claude-sonnet-4-6",
      "fallbacks": ["deepseek-openai/deepseek-v4-pro"],
      "policy": {
        // How many times to cycle the whole chain before giving up, and how
        // long to back off between cycles.
        "exhausted_retry": {
          "max_cycles": 2,
          "initial_backoff_secs": 2,
          "max_backoff_secs": 30
        },
        // After switching to a fallback, half-open probe the primary to see if
        // it has recovered. Backoff doubles each failed probe.
        "recovery": {
          "initial_backoff_secs": 60,
          "max_backoff_secs": 1800,
          "max_attempts": 10
        }
      }
    }
  }
}
```

Use `fallback` (single) or `fallbacks` (list), never both — setting both is a
configuration error rather than a silent precedence rule. Setting
`recovery.max_attempts` to `0` disables recovery probes, pinning you to the
fallback until restart.

You can also append fallbacks for one run from the CLI, which *replaces* the
configured chain:

```bash
cocode-cli --fallback-model deepseek-openai/deepseek-v4-pro \
           --fallback-model groq/llama-3.3-70b-versatile
```

## Reasoning effort

Effort rides on each *slot*, not the role, so a fallback can think at a
different level than the primary:

```jsonc
{
  "models": {
    "main": {
      "primary": { "provider": "anthropic", "model_id": "claude-sonnet-4-6", "effort": "medium" },
      "fallbacks": [
        { "provider": "anthropic", "model_id": "claude-opus-4-7", "effort": "high" }
      ]
    }
  }
}
```

Putting `effort` at the role level is rejected with a message telling you to
move it onto a slot. In the TUI, `Ctrl+T` cycles thinking level and `F2` toggles
it.

## Mixture of Agents

**MoA is a virtual provider.** It occupies the provider slot of a model
selection, and the "model id" is a preset name:

```text
moa/<preset>
```

Bind a role to it and every model call on that role becomes a three-step
pipeline:

1. **Fan out.** The preset's reference models are queried **concurrently**, each
   told it is an advisor that does not execute tools.
2. **Gather.** Their outputs are concatenated into a private advisory block and
   appended to the prompt.
3. **Act.** The **aggregator** model runs the real turn with that advice in
   hand. The aggregator is the acting model and owns every tool call.

This composes with the normal agent loop rather than replacing it — plan mode,
permissions, and tools all behave the same.

### Configuring a preset

```bash
coco moa configure default \
  --aggregator anthropic/claude-sonnet-4-6 \
  --reference openai/gpt-5-5 \
  --reference deepseek-openai/deepseek-v4-pro \
  --fanout per_iteration \
  --default

coco moa list
coco moa delete default
```

Or in `~/.cocode/settings.json`:

```jsonc
{
  "moa": {
    "default_preset": "default",
    "presets": {
      "default": {
        "enabled": true,
        "aggregator": "anthropic/claude-sonnet-4-6",
        "reference_models": ["openai/gpt-5-5", "deepseek-openai/deepseek-v4-pro"],
        "fanout": "per_iteration",
        "reference_max_tokens": 4096,
        "reference_temperature": 0.7,
        "aggregator_temperature": 0.2
      }
    }
  }
}
```

`coco moa configure` also accepts `--reference-max-tokens`,
`--reference-temperature`, `--aggregator-temperature`, `--enable`, and
`--disable`.

### Using a preset

```
/model moa/default        # bind the main role
/model review moa/default # bind any other role
/moa <prompt>             # run one prompt through the default preset, changing no binding
```

`/moa` is the zero-commitment way to try it: it runs a single prompt through
`moa.default_preset` without touching your role bindings.

### Fanout policy

| Value | Behavior |
| --- | --- |
| `per_iteration` (default) | References re-run on every agent-loop iteration — freshest advice, highest cost |
| `user_turn` | References run once per user turn and the result is cached — cheaper on long tool-using turns |

### Rules and limits

- A preset needs an aggregator and at least one reference model, and accepts at
  most **8** references. Duplicates are collapsed by `(provider, model_id)`.
- Preset members must be real providers. Nesting `moa/*` inside a preset is
  rejected.
- Presets are validated against the model registry at startup, so a typo fails
  fast rather than mid-turn.
- **Reference failures never fail your turn.** A reference that errors or
  returns nothing is rendered inline as `[failed: …]` or `[empty]` and the
  aggregator proceeds.
- Reference calls are cost-accounted and reported, so MoA's price is visible
  rather than hidden.

### The synthesized default

If you run `/moa` with no preset configured, cocode synthesizes a `default` one
from the roles you already have: `main` aggregates, with `review` and `fast` as
references. It works with zero MoA configuration — but a deliberate preset is
better, because MoA is most useful when the references genuinely disagree with
each other.

### When MoA is worth it

MoA costs at least one extra model call per reference per iteration. It earns
that on hard, ambiguous problems where models genuinely differ — architecture
decisions, tricky debugging, code review. For routine edits it is usually a
worse deal than simply using a stronger `main`. Start with `/moa <prompt>` on a
hard problem and compare before binding a role to it.

## See also

- [Providers and authentication](providers-and-auth.md) — the provider catalog
- [Configuration](configuration.md) — where settings live
- [Subagents and teams](subagents-and-teams.md) — `modelRole` in agent definitions
