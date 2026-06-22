# coco

coco is a multi-provider AI coding agent. The main implementation lives in the
`coco-rs/` Rust workspace and includes the CLI, TUI, model providers, tool
runtime, permissions, MCP support, memory, plugins, and SDK protocol.

## Quick Start

Run from source during local development:

```bash
cd coco-rs
just coco
```

Send a one-shot prompt:

```bash
cd coco-rs
just coco -p "Summarize this repository"
```

Run without the TUI:

```bash
cd coco-rs
just coco --no-tui -p "List the available commands"
```

Temporarily override the main model:

```bash
cd coco-rs
just coco --models.main deepseek-openai/deepseek-v4-flash
```

Install the published npm package:

```bash
npm install -g @coco-cli/coco-cli
coco-cli --version
coco-cli --help
```

Package page:

https://www.npmjs.com/package/@coco-cli/coco-cli

The npm package installs a small JavaScript launcher plus the native `coco`
binary for your platform. Supported published builds are Linux x64, Linux
arm64, and macOS Apple Silicon.

Upgrade to the latest published version:

```bash
npm install -g @coco-cli/coco-cli@latest
```

Uninstall:

```bash
npm uninstall -g @coco-cli/coco-cli
```

## Configuration Files

coco reads user configuration from `~/.coco/` by default. The most important
files are:

| File | Purpose |
| --- | --- |
| `~/.coco/settings.json` | Runtime settings: selected models, TUI, permissions, diagnostics, tools |
| `~/.coco/providers.json` | Provider catalog: API type, auth env var, base URL, model list |
| `~/.coco/models.json` | Model catalog or overrides: context window, output limits, capabilities |

Provider configuration can be written inline in `settings.json`, but the
recommended shape is to keep provider and model catalogs in
`providers.json` / `models.json`, then keep `settings.json` focused on the
active model selection and user preferences.

Do not commit real API keys. Prefer `env_key`, which points coco at an
environment variable. For a local-only workstation config, providers also
accept `api_key` directly in `~/.coco/providers.json`; if both are present,
the environment variable wins.

## Model Selection

`settings.json` selects models under the `models` object. The minimum useful
configuration sets `main`:

```json
{
  "models": {
    "main": "deepseek-openai/deepseek-v4-flash"
  }
}
```

Model references use this format:

```text
<provider>/<model_id>
```

Common model roles:

| Role | Purpose |
| --- | --- |
| `main` | Default conversation and primary coding agent |
| `fast` | Fast helper calls, such as title generation |
| `plan` | Plan mode |
| `explore` | Exploratory subtask work |
| `review` | Review-oriented subtask work |
| `hook_agent` | Agent invoked by hooks |
| `memory` | Memory-related calls |
| `subagent` | Generic spawned subagent |

Only `models.main` is required. If `fast`, `plan`, `explore`, `review`,
`hook_agent`, `memory`, or `subagent` is not configured, coco defaults that
role to the resolved `main` model, including its fallback chain and fallback
policy.

`settings.json` accepts JSONC-style comments, so a fully annotated model-role
configuration can look like this:

```jsonc
{
  "diagnostics": {
    // "all" is useful while debugging provider wire format. Remove this block
    // for normal usage if you do not want verbose request/response dumps.
    "wire_dump": "all"
  },
  "models": {
    // Required. Every unconfigured role below falls back to this role.
    "main": {
      "primary": "deepseek-openai/deepseek-v4-flash",
      "fallbacks": [
        "deepseek-openai/deepseek-v4-pro"
      ],
      "policy": {
        "exhausted_retry": {
          "max_cycles": 2,
          "initial_backoff_secs": 2,
          "max_backoff_secs": 30
        },
        "recovery": {
          "initial_backoff_secs": 60,
          "max_backoff_secs": 1800,
          "max_attempts": 10
        }
      }
    },

    // Optional. Fast helper calls such as title generation.
    // Remove this key to use models.main.
    "fast": "deepseek-openai/deepseek-v4-flash",

    // Optional. Plan mode.
    // Remove this key to use models.main.
    "plan": "deepseek-openai/deepseek-v4-flash",

    // Optional. Exploratory background/subtask work.
    // Remove this key to use models.main.
    "explore": "deepseek-openai/deepseek-v4-flash",

    // Optional. Review-oriented subtask work.
    // Remove this key to use models.main.
    "review": "deepseek-openai/deepseek-v4-flash",

    // Optional. Hook-triggered agent calls.
    // Remove this key to use models.main.
    "hook_agent": "deepseek-openai/deepseek-v4-flash",

    // Optional. Memory extraction and memory-related model calls.
    // Remove this key to use models.main.
    "memory": "deepseek-openai/deepseek-v4-flash",

    // Optional. Generic spawned subagents from agent/skill dispatch.
    // Remove this key to use models.main.
    "subagent": "deepseek-openai/deepseek-v4-flash"
  }
}
```

Fallback chains can be configured with a nested object:

```json
{
  "models": {
    "main": {
      "primary": "deepseek-openai/deepseek-v4-flash",
      "fallbacks": [
        "deepseek-openai/deepseek-v4-pro"
      ],
      "policy": {
        "exhausted_retry": {
          "max_cycles": 2,
          "initial_backoff_secs": 2,
          "max_backoff_secs": 30
        },
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

You can also override the main model for one run:

```bash
coco-cli --models.main deepseek-openai/deepseek-v4-pro
```

## DeepSeek Configuration

coco includes built-in DeepSeek providers and DeepSeek V4 model metadata:

| Provider | Compatibility layer | Models |
| --- | --- | --- |
| `deepseek-openai` | OpenAI-compatible chat API | `deepseek-v4-flash`, `deepseek-v4-pro` |
| `deepseek-anthropic` | Anthropic-compatible API | `deepseek-v4-flash`, `deepseek-v4-pro` |

The recommended default is:

```text
deepseek-openai/deepseek-v4-flash
```

### Minimal DeepSeek Setup

1. Export your DeepSeek API key:

```bash
export DEEPSEEK_API_KEY="sk-..."
```

To keep it across new shells, add it to your shell profile:

```bash
echo 'export DEEPSEEK_API_KEY="sk-..."' >> ~/.bashrc
source ~/.bashrc
```

2. Create or update `~/.coco/settings.json`:

```bash
mkdir -p ~/.coco
cat > ~/.coco/settings.json <<'JSON'
{
  "diagnostics": {
    "wire_dump": "all"
  },
  "models": {
    "main": "deepseek-openai/deepseek-v4-flash"
  }
}
JSON
```

The same file should look like this:

```json
{
  "diagnostics": {
    "wire_dump": "all"
  },
  "models": {
    "main": "deepseek-openai/deepseek-v4-flash"
  }
}
```

`wire_dump = "all"` writes provider request/response wire dumps for debugging.
Remove the `diagnostics` block for normal daily use if you do not need verbose
logs.

3. Start coco:

```bash
coco-cli
```

From source:

```bash
cd coco-rs
just coco
```

### Explicit Provider Catalog

The local reference setup under `/root/.coco` uses this split:

- `settings.json` selects the main model:
  `models.main = "deepseek-openai/deepseek-v4-flash"`
- `providers.json` defines the `deepseek-openai` provider.
- `models.json` contains a `deepseek-v4-flash` model entry.

The example below follows that split with an environment variable. This is the
recommended shape for checked-in or shared configs.

Create `~/.coco/providers.json`:

```json
{
  "deepseek-openai": {
    "api": "openai_compat",
    "env_key": "DEEPSEEK_API_KEY",
    "base_url": "https://api.deepseek.com/v1",
    "wire_api": "chat",
    "models": {
      "deepseek-v4-flash": {},
      "deepseek-v4-pro": {}
    }
  }
}
```

For a local-only machine config, you can put the DeepSeek key directly in
`~/.coco/providers.json`. Keep this file private and do not commit it:

```json
{
  "deepseek-openai": {
    "api": "openai_compat",
    "env_key": "DEEPSEEK_API_KEY",
    "api_key": "sk-...",
    "base_url": "https://api.deepseek.com/v1",
    "wire_api": "chat",
    "models": {
      "deepseek-v4-flash": {},
      "deepseek-v4-pro": {}
    }
  }
}
```

If `DEEPSEEK_API_KEY` is exported in the environment, it overrides the
`api_key` value from the file. Unset the environment variable when you want to
force coco to use the file value:

```bash
unset DEEPSEEK_API_KEY
```

Keep `~/.coco/settings.json` focused on model selection:

```jsonc
{
  "diagnostics": {
    "wire_dump": "all"
  },
  "models": {
    // Required. Other roles can be omitted; they fall back to main.
    "main": "deepseek-openai/deepseek-v4-flash",

    // Optional examples. Delete any role you do not need to customize.
    "fast": "deepseek-openai/deepseek-v4-flash",
    "plan": "deepseek-openai/deepseek-v4-flash",
    "explore": "deepseek-openai/deepseek-v4-flash",
    "review": "deepseek-openai/deepseek-v4-flash",
    "hook_agent": "deepseek-openai/deepseek-v4-flash",
    "memory": "deepseek-openai/deepseek-v4-flash",
    "subagent": "deepseek-openai/deepseek-v4-flash"
  }
}
```

You usually do not need `~/.coco/models.json` for DeepSeek V4 because coco
already includes these model definitions. If you need to add a custom model,
add metadata like this:

```json
{
  "deepseek-custom": {
    "display_name": "DeepSeek Custom",
    "context_window": 1000000,
    "max_output_tokens": 12288,
    "capabilities": [
      "text_generation",
      "streaming",
      "tool_calling",
      "parallel_tool_calls"
    ]
  }
}
```

Then add `deepseek-custom` to the provider's `models` map and select
`deepseek-openai/deepseek-custom`.

## Common Settings

Enable wire dump diagnostics:

```json
{
  "diagnostics": {
    "wire_dump": "all"
  },
  "models": {
    "main": "deepseek-openai/deepseek-v4-flash"
  }
}
```

Limit max tokens and max turns for one run:

```bash
coco-cli --max-tokens 4096 --max-turns 8
```

Set the working directory:

```bash
coco-cli -C /path/to/project
```

Use a separate settings file:

```bash
coco-cli --settings /path/to/settings.json
```

## Troubleshooting

**`no Main model configured`**

Set `models.main` in `settings.json`:

```json
{
  "models": {
    "main": "deepseek-openai/deepseek-v4-flash"
  }
}
```

**Provider is missing an API key**

Check that the environment variable exists:

```bash
echo "$DEEPSEEK_API_KEY"
```

If you use `providers.json`, make sure the provider points at that variable:

```json
{
  "env_key": "DEEPSEEK_API_KEY"
}
```

**`unknown model`**

A selected model must be one of:

- a built-in model, such as `deepseek-v4-flash`
- an entry in `~/.coco/models.json`
- an entry in `providers.<name>.models`

For built-in DeepSeek V4, check the spelling:

```text
deepseek-openai/deepseek-v4-flash
deepseek-openai/deepseek-v4-pro
```

**Plaintext `api_key` in config files**

`providers.json` supports an `api_key` field for local-only configs:

```json
{
  "api_key": "sk-..."
}
```

For shared configs, prefer:

```json
{
  "env_key": "DEEPSEEK_API_KEY"
}
```

That keeps secrets in the environment or your secret manager instead of in
dotfiles or git. If both `env_key` and `api_key` are set, the environment
variable wins.

## Repository Layout

- `coco-rs/`: Rust workspace for the CLI, TUI, providers, services, tools, and shared crates.
- `coco-cli/`: npm package wrapper for the prebuilt `coco` binary.
- `coco-sdk/`: SDK schemas and language bindings.
- `docs/`: Architecture, design, and development notes.
- `.claude/` and `.codex/`: Project-local agent instructions, scripts, and skills.

## Development

Most Rust development happens in `coco-rs/`:

```bash
cd coco-rs
just fmt
just quick-check
just pre-commit
```

`just pre-commit` runs the full nextest suite and is expensive. Use
`just quick-check` while iterating, then run `just pre-commit` before a commit.

Read the root `CLAUDE.md` before code changes. If the crate you are editing
has its own `CLAUDE.md`, follow that file as well.
