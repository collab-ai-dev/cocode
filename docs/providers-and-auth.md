# Providers and authentication

cocode is not tied to one model vendor. This page covers the built-in provider
catalog, how to add your own provider, and the two ways to authenticate: an API
key, or an OAuth login against a subscription you already pay for.

## How a model is addressed

Every model reference is explicit:

```text
<provider>/<model_id>
```

There are no vendor aliases like `opus` or `sonnet` — `provider` names a
*provider instance* from the catalog, and `model_id` names a model within it.
Two instances can point at the same vendor with different transports or
credentials, which is why `deepseek-openai` and `deepseek-anthropic` both exist.

## Built-in providers

<!-- BEGIN GENERATED: providers -->

| Provider | `api` | Auth | Base URL |
| --- | --- | --- | --- |
| `anthropic` | `anthropic` | `ANTHROPIC_API_KEY` | `https://api.anthropic.com/v1` |
| `openai` | `openai` | `OPENAI_API_KEY` | `https://api.openai.com/v1` |
| `openai-chatgpt` | `openai` | OAuth (ChatGPT subscription) | `https://chatgpt.com/backend-api/codex` |
| `google` | `google` | `GOOGLE_API_KEY` | `https://generativelanguage.googleapis.com/v1beta` |
| `gemini-code-assist` | `google` | OAuth (Gemini Code Assist) | `https://cloudcode-pa.googleapis.com/v1internal` |
| `volcengine` | `volcengine` | `ARK_API_KEY` | `https://ark.cn-beijing.volces.com/api/v3` |
| `zai` | `zai` | `ZAI_API_KEY` | `https://api.z.ai/v1` |
| `deepseek-openai` | `openai-compat` | `DEEPSEEK_API_KEY` | `https://api.deepseek.com/v1` |
| `deepseek-anthropic` | `anthropic` | `DEEPSEEK_API_KEY` | `https://api.deepseek.com/anthropic/v1` |
| `xai` | `xai` | `XAI_API_KEY` | `https://api.x.ai/v1` |
| `grok` | `xai` | OAuth (Grok subscription) | `https://cli-chat-proxy.grok.com/v1` |
| `groq` | `openai-compat` | `GROQ_API_KEY` | `https://api.groq.com/openai/v1` |

<!-- END GENERATED: providers -->

The `api` column is the wire protocol the provider speaks. The accepted values
are `anthropic`, `openai`, `google`, `volcengine`, `zai`, `xai`, and
`openai-compat`. Use `openai-compat` for any third-party endpoint that
implements the OpenAI chat API.

Only some providers ship model metadata out of the box: Anthropic
(`claude-sonnet-4-6`, `claude-opus-4-7`, `claude-haiku-4-5`), OpenAI (`gpt-5-4`,
`gpt-5-5`, `gpt-5-3-codex`), Google (`gemini-3.1-pro-preview`), and DeepSeek
(`deepseek-v4-flash`, `deepseek-v4-pro`). For the others you name the model
yourself; a role binding only requires that the *provider* is known.

> Anthropic-on-Bedrock, Vertex, and Foundry are deliberately out of scope.
> cocode targets Anthropic's first-party API, OpenAI, Google, xAI, ByteDance,
> and generic OpenAI-compatible endpoints.

## Authenticating with an API key

Each provider declares an `env_key` — the environment variable it reads. Export
it and you are done:

```bash
export DEEPSEEK_API_KEY="sk-..."
cocode-cli --models.main deepseek-openai/deepseek-v4-flash
```

To persist it, add it to your shell profile:

```bash
echo 'export DEEPSEEK_API_KEY="sk-..."' >> ~/.zshrc
```

For a local-only machine you may instead write the key into
`~/.cocode/providers.json` as `api_key`. **If both are present, the environment
variable wins** — unset it if you want the file value to take effect:

```jsonc
{
  "deepseek-openai": {
    "api": "openai-compat",
    "env_key": "DEEPSEEK_API_KEY",
    "api_key": "sk-...",          // local-only; env_key still wins if exported
    "base_url": "https://api.deepseek.com/v1",
    "models": { "deepseek-v4-flash": {} }
  }
}
```

Never commit a file containing a real key. For anything shared, use `env_key`
and keep the secret in your environment or a secret manager. There is also an
`api_key_helper` setting that runs a shell command to fetch a key (cached for
five minutes); for safety it is ignored when set from project-level settings.

## Authenticating with a subscription

Three providers support OAuth against a subscription you already have:

| Command | Provider instance | Flow |
| --- | --- | --- |
| `coco login openai` | `openai-chatgpt` | Browser loopback on `127.0.0.1:1455` |
| `coco login gemini` | `gemini-code-assist` | Browser loopback, ephemeral port |
| `coco login grok` | `grok` | **Device code** — no browser needed |

The provider argument accepts shorthands: `openai`, `chatgpt`, `openai-oauth`,
and `oauth` all mean `openai-chatgpt`; `gemini` and `google` mean
`gemini-code-assist`; `grok` and `xai-oauth` mean `grok`. With no argument at
all, `coco login` defaults to `openai-chatgpt`.

On success it prints the account it authenticated and a hint for using it:

```text
✓ Logged in to `grok` (Grok subscription) as you@example.com (plan).
```

Because `grok` uses an RFC 8628 device-code flow, it works on a headless server
or over SSH: cocode prints a URL and a code, you approve it on any device, and
the CLI picks up the token. For the loopback flows, `--no-browser` (alias
`--headless`) prints the authorization URL instead of opening a browser.

Inside a running session, bare `/login` opens a picker of OAuth-capable
providers and completes the flow in-process — no restart, and the model picker
un-gates the provider immediately.

To sign out:

```bash
coco logout grok
```

### Importing an existing login

If you already authenticated the Codex CLI, cocode can adopt that credential
rather than making you log in again:

```bash
coco login openai --import ~/.codex/auth.json
```

The source file is read once and never modified; symlinks and non-regular files
are rejected. Import currently supports only the ChatGPT flow.

### There is no Claude subscription login

Anthropic models are API-key only (`ANTHROPIC_API_KEY`). A Claude Pro/Max OAuth
flow does not exist in cocode today.

## Where credentials are stored

| Backend | Location |
| --- | --- |
| `file` | `~/.cocode/auth/<provider>.json`, directory `0700`, files `0600` |
| `keyring` | The OS keychain, under the service name `cocode Provider Auth` |
| `auto` | Keychain first, file as fallback |
| `ephemeral` | Memory only; nothing persists |

Official release builds default to `auto`. **Any locally-built binary — a
`cargo build --release` included — defaults to `file`**, so a development build
never touches your OS keychain and never triggers a macOS "allow access" prompt.

Force a backend explicitly with `COCO_AUTH_CREDENTIAL_STORE`:

```bash
export COCO_AUTH_CREDENTIAL_STORE=file    # auto | file | keyring | ephemeral
```

The environment variable takes precedence over the `auth_credential_store`
global config value, which takes precedence over the build default. Project
settings deliberately cannot influence it.

## Adding your own provider

Any endpoint that speaks the OpenAI chat API works. The easiest path is the
in-session wizard:

```
/provider
```

It writes the provider into your settings. To do it by hand, add an entry to
`~/.cocode/providers.json`:

```jsonc
{
  "my-endpoint": {
    "api": "openai-compat",
    "env_key": "MY_ENDPOINT_API_KEY",
    "base_url": "https://llm.example.com/v1",
    "wire_api": "chat",              // "chat" (default) or "responses"
    "models": {
      "my-model-id": {}
    }
  }
}
```

Then select it:

```bash
cocode-cli --models.main my-endpoint/my-model-id
```

Providers resolve in layers: the built-in catalog first, then
`~/.cocode/providers.json`, then a `providers` block in `settings.json`. Later
layers override earlier ones, so you can override a built-in provider's base URL
without redefining it.

### Custom model metadata

You usually do not need this — cocode ships metadata for its built-in models.
When an endpoint has no known model, declare it in `~/.cocode/models.json` (or
inline under the provider's `models` map):

```json
{
  "my-model-id": {
    "display_name": "My Model",
    "context_window": 1000000,
    "max_output_tokens": 12288,
    "capabilities": ["text_generation", "streaming", "tool_calling", "parallel_tool_calls"]
  }
}
```

If a provider's catalog id differs from the slug the API expects, map it with
`api_model_name` (this is how `gpt-5-5` reaches the wire as `gpt-5.5`).

## Checking what is configured

The model picker (`/model` with no argument) shows every provider with its
status, and tells you *why* one is unusable — missing base URL, missing API key
(naming the environment variable it wants), not logged in, or no models.

## See also

- [Models and MoA](models-and-moa.md) — roles, fallback chains, Mixture of Agents
- [Configuration](configuration.md) — where settings live and how they merge
- [Troubleshooting](troubleshooting.md) — auth and model errors
