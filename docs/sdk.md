# SDK

cocode ships TypeScript and Python SDKs that drive the agent programmatically.
Both work the same way: they spawn the `coco` binary as a subprocess and talk to
it over a JSON-RPC control protocol. This page covers installing them, running
your first query, requesting structured output, and regenerating the protocol
types.

## How it works

Running `coco sdk` puts the binary into **SDK mode**:

> Run in SDK mode — NDJSON over stdio with the JSON-RPC control protocol.
> Intended to be spawned as a subprocess by the Python/TypeScript SDK client.

In this mode `coco` speaks **JSON-RPC 2.0** framed as **NDJSON** — one JSON
message per line — over stdin and stdout. You do not normally run `coco sdk`
by hand. The SDK clients spawn it for you, perform the `initialize` handshake,
start a session, and turn the notification stream into an async iterator.

Because stdout is reserved for the protocol, logs never go there. See
[troubleshooting](troubleshooting.md) for where they land instead.

### Finding the binary

Both SDKs resolve the `coco` executable in the same order:

1. An explicit `binaryPath` / `binary_path` option, if you passed one.
2. The **`COCO_PATH`** environment variable.
3. `coco` on your `PATH`.
4. Well-known install locations: `~/.cargo/bin/coco`, then `/usr/local/bin/coco`.

If none of those hit, you get a `CLINotFoundError`:

```
coco binary not found. Install it or set COCO_PATH environment variable.
```

The SDKs also set `COCO_ENTRYPOINT` on the subprocess (`sdk-ts` or `sdk-py`) so
the runtime knows which client spawned it.

## TypeScript SDK

The package is **`@coco-rs/coco-sdk`**. It is ESM-only (`"type": "module"`) and
requires Node 18 or newer.

```bash
npm install @coco-rs/coco-sdk
```

`query()` is the one-shot entry point. It returns an async generator of protocol
notifications, so you consume it with `for await`:

```ts
import { query, NotificationMethod } from "@coco-rs/coco-sdk";

for await (const event of query("List the Rust crates in this workspace")) {
  if (event.method === NotificationMethod.AGENT_MESSAGE_DELTA) {
    process.stdout.write(event.params.delta);
  }
}
```

The generator finishes when the turn ends — internally it breaks on
`NotificationMethod.TURN_ENDED`, and the subprocess is torn down for you.

`query(prompt, options)` accepts:

| Option | Type | Meaning |
|---|---|---|
| `modelsMain` | `string \| ModelSpec` | The Main model, as `provider/model_id`. Passed to the subprocess as `--models.main`. |
| `maxTurns` | `number` | Maximum agent turns before the run stops. |
| `cwd` | `string` | Working directory for the session. |
| `systemPrompt` | `string` | Replace the built-in system prompt. |
| `appendSystemPrompt` | `string` | Append to the system prompt. |
| `permissionMode` | `PermissionMode` | Starting permission mode. |
| `maxBudgetUsd` | `number` | Spend ceiling for the run. |
| `env` | `Record<string, string>` | Extra environment for the subprocess. |
| `binaryPath` | `string` | Explicit path to the `coco` binary. |
| `signal` | `AbortSignal` | Cancel the run. |

For multi-turn sessions, in-process tools, or hooks, use `CocoClient` instead of
`query()`. The package also exports the error types (`CLINotFoundError`,
`ProcessError`, `TransportClosedError`, and friends), the runtime helpers
(`findCocoBinary`, `resolveCocoRuntime`), and every generated protocol type.

## Python SDK

The distribution is **`coco-sdk`**. It needs Python 3.10 or newer and depends on
`anyio` and `pydantic` v2.

```bash
pip install coco-sdk
```

`query()` is an async iterator over protocol events:

```python
import asyncio

from coco_sdk import NotificationMethod, query


async def main():
    async for event in query("What is 2 + 2?", max_turns=1):
        if event.method == NotificationMethod.AGENT_MESSAGE_DELTA:
            print(event.params.get("delta", ""), end="", flush=True)
        elif event.method == NotificationMethod.ERROR:
            print(f"\nError: {event.params.get('message', 'unknown')}")
        elif event.method == NotificationMethod.TURN_ENDED:
            print("\n--- Turn ended ---")


if __name__ == "__main__":
    asyncio.run(main())
```

The terminal event is **`TURN_ENDED`** (`turn/ended`), which carries a
discriminated `outcome`. Note that the bundled `examples/basic_query.py`
branches on `NotificationMethod.TURN_COMPLETED`, which does not exist in the
generated protocol — that branch is dead code and never fires. Use
`TURN_ENDED`, as above.

`query()` takes the same knobs as the TypeScript version, in snake_case:
`models_main`, `max_turns`, `cwd`, `system_prompt`, `append_system_prompt`,
`permission_mode`, `max_budget_usd`, `env`, and `binary_path`. `models_main`
accepts either a `"<provider>/<model_id>"` string or a `ModelSpec`.

### Multi-turn with CocoClient

`query()` is fire-and-forget. When you need to keep talking to the same session,
use `CocoClient` as an async context manager — `client.events()` drains the first
turn and `client.send(...)` starts a follow-up:

```python
import asyncio

from coco_sdk import CocoClient, NotificationMethod


async def main():
    async with CocoClient(
        prompt="Create a hello world Python script",
        max_turns=3,
    ) as client:
        async for event in client.events():
            if event.method == NotificationMethod.AGENT_MESSAGE_DELTA:
                print(event.params.get("delta", ""), end="", flush=True)

        async for event in client.send("Now add a docstring to the script"):
            if event.method == NotificationMethod.AGENT_MESSAGE_DELTA:
                print(event.params.get("delta", ""), end="", flush=True)


if __name__ == "__main__":
    asyncio.run(main())
```

### In-process tools

The `@tool()` decorator exposes a local Python function to the agent as an
in-process MCP tool — no separate server process:

```python
from coco_sdk import CocoClient, tool


@tool()
def get_weather(city: str) -> str:
    """Get current weather for a city."""
    return f"Sunny, 22C in {city}"


@tool(name="calculate", description="Perform arithmetic")
def calculate(expression: str) -> str:
    """Evaluate a math expression."""
    ...


async with CocoClient(
    prompt="What's the weather in Tokyo? Also, what's 42 * 17?",
    tools=[get_weather, calculate],
    permission_mode="bypassPermissions",
) as client:
    ...
```

The docstring becomes the tool description unless you pass one explicitly, and
the type hints become the schema.

### Examples

The runnable examples live in `coco-sdk/python/examples/`:

| File | Shows |
|---|---|
| `basic_query.py` | One-shot `query()` with delta streaming. Its `TURN_COMPLETED` branch is dead code — see above. |
| `multi_turn.py` | `CocoClient` with a follow-up turn. |
| `tools_example.py` | `@tool()` in-process tools. |
| `hooks_example.py` | `@hook(event="PreToolUse", matcher="Bash")` blocking a dangerous command. |
| `agents_example.py` | Custom agent definitions passed via `CocoClient(agents=...)`. |
| `structured_output.py` | `TypedClient` with a Pydantic model. **Does not currently work** — see [structured output](#from-python--currently-broken). |

Note that `coco-sdk/python/README.md` is currently **empty** — the examples
directory and the type hints are the real documentation for now.

## Structured output

When you need a machine-readable result rather than prose, hand the run a JSON
Schema. The agent then answers by calling a generated **`StructuredOutput`**
tool whose input schema is the one you supplied, and the run is not considered
complete until it does.

### From the CLI

The `--json-schema` flag takes an **inline JSON Schema — not a file path**:

```bash
coco -p "Which crates does this workspace define?" \
  --json-schema '{"type":"object","properties":{"crates":{"type":"array","items":{"type":"string"}}},"required":["crates"]}'
```

It is **only honored in non-interactive sessions** — print mode (`-p`) and SDK
mode. The TUI ignores it entirely: the `StructuredOutput` tool is registered
only on the headless and AppServer paths, so an interactive session never sees
it.

Two failure modes are worth knowing, because both are hard errors at startup
rather than silent fallbacks:

```
--json-schema is not valid JSON: <parse error>
--json-schema rejected: <reason>
```

The second one fires when the JSON parses but the schema itself is unusable —
an invalid shape or an unsupported keyword.

### From TypeScript

`CocoClient` accepts a `jsonSchema` option and forwards it on `session/start`:

```ts
import { CocoClient } from "@coco-rs/coco-sdk";

const client = new CocoClient({
  prompt: "Review src/main.rs and report findings",
  jsonSchema: {
    type: "object",
    properties: {
      summary: { type: "string" },
      issues: { type: "array", items: { type: "string" } },
      score: { type: "integer" },
    },
    required: ["summary", "issues", "score"],
  },
});
```

### From Python — currently broken

The Python SDK exposes `TypedClient` (and `CocoClient(json_schema=...)`), which
is intended to wrap the same mechanism with a Pydantic model. **It does not work
today.** The schema is threaded into the `initialize` request, whose generated
`InitializeParams` model has no `json_schema` field, so Pydantic silently drops
it; `session/start` — the request that actually carries `json_schema` on the
wire — never sends it. The result is that no `StructuredOutput` tool is
registered and the schema is quietly ignored rather than failing loudly.

Until this is fixed, get structured output from Python by driving the CLI's
`--json-schema` in print mode, or use the TypeScript client.

## Useful flags for SDK and headless runs

These are passed to the subprocess as top-level flags, before the `sdk`
subcommand. The SDKs already do this for the options they expose; use the `env`
option or your own spawn if you need others. See the
[CLI reference](cli-reference.md) for the complete list.

| Flag | Why it matters for automation |
|---|---|
| `--no-session-persistence` | Do not write the session to disk. Valid **only** in print mode or SDK mode — the TUI rejects it. |
| `--include-hook-events` | Emit `HookStarted` / `HookProgress` / `HookResponse` in the stream so you can observe hook lifecycle. |
| `--session-id <ID>` | Use an explicit session ID. Makes IDs deterministic in automation. |
| `--fork-session` | With `--resume <id>`, copy that history into a fresh session instead of continuing the original. |
| `--max-turns <N>` | Cap agent turns so a run cannot loop indefinitely. |
| `--max-tokens <N>` | Cap tokens per model response. |
| `--json-schema <JSON>` | Structured output, as above. |

One ordering gotcha if you spawn `coco` yourself: clap parses top-level flags
**before** the subcommand, so the argument vector must be
`[...flags, "sdk"]`. Putting flags after `sdk` produces "unexpected argument"
errors.

## Protocol types are generated

Neither SDK hand-writes its protocol types. They are generated from the Rust
source, through JSON Schema, into each language:

```
coco-rs types  →  coco-sdk/schemas/json/*.json  →  protocol.py / protocol.ts
```

The schemas in `coco-sdk/schemas/json/` are produced by a `schema`-featured
export example in `coco-types` (plus `coco-hooks` for `hook_input.json`, which
is merged into the bundle at script level). `coco_app_server_protocol.schemas.json`
is the combined bundle.

Regenerate everything with:

```bash
./coco-sdk/scripts/generate_all.sh
```

Or run the three stages individually:

```bash
./coco-sdk/scripts/generate_schemas.sh     # coco-rs types → schemas/json/*.json
./coco-sdk/scripts/generate_python.sh      # schemas → generated/protocol.py + stubs
./coco-sdk/scripts/generate_typescript.sh  # schemas → generated/protocol.ts
```

Every script accepts `--check`, which regenerates into a temporary directory and
diffs against what is committed without touching your working tree. This is the
CI mode — `generate_all.sh --check` fails if anything is stale. `generate_schemas.sh`
additionally takes `--force` (regenerate unconditionally; by default it skips
when no input is newer than the bundle) and `--quiet`.

Because these files are generated, **edit the Rust types, not `protocol.py` or
`protocol.ts`** — the next regeneration will overwrite anything you change by
hand.

## See also

- [CLI reference](cli-reference.md) — every flag, and how TUI/headless/SDK mode is chosen.
- [Troubleshooting](troubleshooting.md) — model configuration, auth, and logs.
- [Tools](tools.md) — what the agent can call.
- [MCP](mcp.md) — external tool servers, as opposed to the in-process `@tool()` form.
