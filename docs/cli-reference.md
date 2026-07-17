# CLI reference

Complete reference for the cocode command-line interface: every global flag, every subcommand, how the binary decides whether to open the terminal UI or run non-interactively, and which flags were removed from earlier versions.

## The three names

cocode ships under three names, and they refer to different things. Knowing which is which saves a lot of confusion:

- **`coco`** is the native binary. This is what the Rust workspace builds and what you actually run.
- **`cocode`** is the display name clap uses. It is what you see in `--help` output and error messages, but it is not an executable.
- **`cocode-cli`** is the command installed by the npm package `@cocode-cli/cocode-cli`. It is a small JavaScript launcher that locates and executes the native `coco` binary. If you installed cocode from npm, `cocode-cli` is your entry point; everything in this page applies unchanged, because the launcher forwards its arguments verbatim.

Throughout this page the examples use `coco`. Substitute `cocode-cli` if that is how you installed it.

## Synopsis

```bash
coco [OPTIONS]
coco [OPTIONS] <SUBCOMMAND>
```

With no subcommand and no arguments, `coco` opens the interactive terminal UI in the current directory. Everything else is a variation on that: a subcommand replaces the default behavior, and certain flags force a non-interactive run instead.

## Global flags

These flags apply to the top-level `coco` invocation. They are accepted before a subcommand, not after it — `coco --cwd /srv/app chat "hello"` is correct, `coco chat --cwd /srv/app "hello"` is not.

<!-- BEGIN GENERATED: cli-flags -->

| Flag | Value | Description |
|------|-------|-------------|
| `-p`, `--prompt` | `<PROMPT>` | Prompt to send. Implies non-interactive (headless) mode. |
| `--models.main` | `<PROVIDER/MODEL_ID>` | Override the model bound to the Main role for this run. |
| `--settings` | `<PATH>` | Load an additional settings file as the Flag layer (see [configuration](configuration.md)). |
| `--event-hub-url` | `<WS_URL>` | Send this session's events to an external Event Hub. Must be `ws://` or `wss://`. Conflicts with `--serve-hub`. |
| `--serve-hub` |  | Start an embedded local Event Hub and point this process at it. Requires a binary built with the `serve-hub` cargo feature. |
| `--hub-port` | `<PORT>` | Port for the embedded Event Hub. Default `8731`. Requires `--serve-hub`. |
| `--max-tokens` | `<N>` | Maximum tokens per model response. |
| `--max-turns` | `<N>` | Maximum agent turns before the run stops. |
| `--permission-mode` | `<MODE>` | Starting permission mode: `default`, `plan`, `acceptEdits`, `bypassPermissions`, `dontAsk`, or `auto`. |
| `-C`, `--cwd` | `<DIR>` | Working directory for the session. Defaults to the process working directory. |
| `-r`, `--resume` | `<SESSION_ID>` | Resume a specific session by ID or title. |
| `--system-prompt` | `<TEXT>` | Replace the built-in system prompt entirely with this text. |
| `--append-system-prompt` | `<TEXT>` | Append this text verbatim to the system prompt. |
| `-c`, `--continue-session` |  | Continue the most recent conversation. Also accepted as `--continue`. |
| `--allowed-tools` | `<TOOL>...` | Allow specific tools. Accepts multiple values after one flag, and the flag may repeat. |
| `--disallowed-tools` | `<TOOL>...` | Deny specific tools. Same repeat semantics as `--allowed-tools`. |
| `--add-dir` | `<DIR>...` | Grant the session access to additional directories outside the working directory. |
| `--dangerously-skip-permissions` |  | Start in `bypassPermissions` mode and unlock it as a reachable target for Shift+Tab and plan-mode exit. |
| `--allow-dangerously-skip-permissions` |  | Unlock `bypassPermissions` as an option without entering it at startup. |
| `--non-interactive` |  | Print the response and exit. Also accepted as `--print`. |
| `--fallback-model` | `<PROVIDER/MODEL_ID>` | Fallback model on capacity errors. Repeat the flag to build a multi-tier chain; tiers are walked in flag order. |
| `--no-session-persistence` |  | Do not persist the session. Only valid in print mode or SDK mode. |
| `--bare` |  | Skip session-start and per-turn background housekeeping (auto-dream, memory extraction, prompt suggestion, stale-directory sweeps). Flag form of `COCO_BARE_MODE=1`. |
| `--json-schema` | `<JSON>` | Inline JSON Schema (not a file path) validating the run's structured output. Honored in print mode and SDK mode; ignored in the TUI. |
| `--include-hook-events` |  | Emit hook lifecycle events (`HookStarted`, `HookProgress`, `HookResponse`) in the stream-json output. |
| `--append-system-prompt-file` | `<PATH>` | Read a file and append its contents to the system prompt. Fails if the file is missing. |
| `--setting-sources` | `<CSV>` | Comma-separated settings layers to load: `user`, `project`, `local`. See [configuration](configuration.md). |
| `--fork-session` |  | Copy the history from `--resume <id>` into a fresh session instead of continuing the original. |
| `--session-id` | `<ID>` | Use an explicit session ID for this run. For deterministic IDs in automation. |
| `--log-level` | `<DIRECTIVE>` | Tracing filter. A bare level (`debug`) expands to `coco=debug,debug`; a full `EnvFilter` directive is passed through. Highest-priority log override. |
| `--log-format` | `<FORMAT>` | `pretty`, `compact`, or `json`. Defaults by mode: `json` for SDK, `compact` for TUI and headless. |
| `--log-file` | `<PATH>` | Override the default rotating log file path. |
| `--log-stderr` |  | Add a stderr log layer alongside the file sink. |
| `--log-location` | `[<BOOL>]` | Show source `file:line` and thread name on each event. Tri-state: bare flag or `=true` forces on, `=false` forces off, omission auto-enables when the resolved filter is `debug` or `trace`. |
| `--log-timezone` | `<TZ>` | `local` or `utc` for log timestamps. Default `local`. |
| `-h`, `--help` |  | Print help. |
| `-V`, `--version` |  | Print version, commit, and build time. |

<!-- END GENERATED: cli-flags -->

A few flags carry behavior that is easy to get wrong from the table alone:

`--system-prompt` and `--append-system-prompt` are not the same kind of thing. `--system-prompt` **replaces** the entire built-in system prompt, which discards the tool instructions and environment context cocode normally assembles. `--append-system-prompt` leaves the built-in prompt intact and adds your text after it. Reach for the append form unless you specifically intend to run without the standard prompt.

`--fallback-model` is repeatable and order-sensitive. Passing it three times builds a three-tier chain that cocode walks in flag order when the Main model returns capacity errors. A single occurrence produces a one-tier chain, which is the common case.

`--plan-mode-instructions <TEXT>` exists but is hidden from `--help`. It replaces the default plan-mode implementation phases while keeping the read-only preamble and the ExitPlanMode footer, and it is rejected outside print mode.

## Subcommands

The Status column is honest about what is actually wired up. Anything marked "stub" parses and prints something, but does not do the work its name suggests — do not build scripts on it.

<!-- BEGIN GENERATED: cli-subcommands -->

| Subcommand | Mode | Status | Description |
|------------|------|--------|-------------|
| `chat [PROMPT]` | Headless | Working | Run one non-interactive turn. Defaults to the prompt `Hello!` when none is given. |
| `config get <KEY>` | Skip | Working | Print one top-level merged settings value. Lists available keys when the key is absent. |
| `config set <KEY> <VALUE>` | Skip | Works | Writes the key to your user `settings.json`, preserving comments and formatting. Accepts dotted keys (`models.main`). The value is read as JSON when it parses (`true` becomes a boolean), otherwise as a string. Takes effect in the next session. |
| `config list` | Skip | Working | Print the fully merged settings as JSON. |
| `config reset` | Skip | Working | Delete the user settings file. |
| `resume [SESSION_ID]` | TUI | Working | Resume a session in the interactive UI. With no ID, continues the most recent conversation. |
| `sessions` | Skip | Working | List saved sessions with ID, model, creation time, and working directory. |
| `status` | Skip | **Partly hardcoded** | Prints a hardcoded `coco-rs v0.0.0` as the version. The model, provider, and auth lines below it are real; the version is not. Use `coco --version` for the real build. |
| `doctor` | Skip | **Partly hardcoded** | The shell and config lines are printed unconditionally without probing anything. Only the model line and the auth status are resolved for real. |
| `login [PROVIDER]` | Skip | Working | Log in to a provider subscription via OAuth. Defaults to `openai`. `--no-browser` prints the authorization URL instead of opening a browser; `--import <PATH>` imports a credential from another tool's auth file. |
| `logout [PROVIDER]` | Skip | Working | Clear stored provider credentials. Defaults to `openai`. |
| `init` | Skip | Working | Create a `.cocode/` directory in the current directory with an empty `settings.json`. |
| `review [TARGET]` | Headless | Working | Ask the agent to review changes. Defaults to `HEAD`. |
| `mcp list` | Skip | **Stub** | Always prints `MCP servers: (none connected)` regardless of configuration. |
| `mcp add <NAME> [CONFIG]` | Skip | **Stub** | Echoes its arguments. Nothing is registered. |
| `mcp remove <NAME>` | Skip | **Stub** | Echoes its argument. Nothing is removed. |
| `mcp login <NAME>` | Skip | Working | Run the OAuth flow for an MCP server. `--no-browser` prints the URL instead. |
| `mcp logout <NAME>` | Skip | Working | Clear stored OAuth credentials for an MCP server. |
| `plugin list` | Skip | Working | List installed and enabled plugins. |
| `plugin install <NAME>` | Skip | Working | Install from a local directory containing `PLUGIN.toml`, or from a registered marketplace via `<name>[@<marketplace>]`. |
| `plugin uninstall <NAME>` | Skip | Working | Uninstall a plugin by name. |
| `plugin validate <PATH>` | Skip | Working | Validate a `PLUGIN.toml` manifest. |
| `moa list` | Skip | Working | List configured Mixture-of-Agents presets. |
| `moa configure <NAME>` | Skip | Working | Create or replace a MoA preset. See [models and MoA](models-and-moa.md). |
| `moa delete <NAME>` | Skip | Working | Delete a MoA preset. |
| `agents` | Skip | Working | List discovered agent definitions from the built-in catalog and the user and project agent directories. |
| `auto-mode [defaults]` | Skip | **Stub** | Prints a fixed one-line pointer to `/permissions`. It shows no actual defaults. |
| `exec-server [--listen URL]` | Skip | Working | Run a local exec-server over WebSocket or stdio. |
| `ps [--json] [--all]` | Skip | Working | List running background sessions. `--json` emits a JSON array for scripting; `--all` includes completed and failed entries. |
| `release-notes` | Skip | **Wrong target** | Prints the current version and then links to the `anthropics/claude-code` releases page, which is a different project. Ignore the link. |
| `sdk` | SDK | Working | Run in SDK mode: NDJSON over stdio with the JSON-RPC control protocol. Intended to be spawned as a subprocess by the Python or TypeScript SDK. |

<!-- END GENERATED: cli-subcommands -->

The Mode column refers to the execution mode described in the next section. "Skip" means the subcommand does its work and exits without starting an agent session; those subcommands never install a logging subscriber, so their stdout stays clean and pipeable.

## Mode selection

Every invocation resolves to exactly one of four execution modes. Understanding the precedence saves you from wondering why a script hangs waiting for terminal input, or why the UI refused to open.

If a **subcommand** is present, the subcommand alone decides the mode, and no flag overrides it:

- `chat` and `review` run Headless.
- `sdk` runs in SDK mode.
- `resume` opens the TUI.
- Every other subcommand is Skip: it runs and exits.

If **no subcommand** is present, the first matching rule wins, checked in this order:

1. `--non-interactive` (or its alias `--print`) selects Headless.
2. `-p` / `--prompt` selects Headless. **Passing a prompt implies headless.** There is no separate flag to request it.
3. Standard input is not a terminal — Headless. This is what makes `echo "..." | coco` work: the prompt is read from stdin.
4. Standard output is not a terminal — Headless. This is what makes `coco > out.txt` work without painting escape codes into the file.
5. Otherwise, the TUI opens.

**There is no `--no-tui` flag.** It was removed, and a regression test asserts the CLI rejects it as an unknown argument. If you have a script carrying `--no-tui`, replace it with `-p "<your prompt>"`, which is the flag that actually implies headless, or simply pipe input or output — rules 3 and 4 reach headless on their own.

Two flags are validated against the resolved mode and hard-error when they do not match:

- `--no-session-persistence` requires Headless or SDK mode. In the TUI it fails with `--no-session-persistence can only be used in print mode (-p / --print) or SDK mode`.
- `--plan-mode-instructions` requires Headless specifically. SDK mode is not sufficient.

## Removed flags and subcommands

The following were removed and are now rejected as unknown arguments or invalid subcommands. They appear here because older documentation and scripts still reference them, and because a regression test pins their rejection — they are not coming back under these names.

Removed flags: `--no-tui`, `--json`, `--debug`, `--verbose`, `--bg`, `--background`, `--thinking-budget`, `--mcp-config`, `--output-format`, `--effort`, `--worktree`, `--name`, `--agent`, `--max-budget-usd`, `--init-only`, `--input-format`, `--replay-user-messages`, `--include-partial-messages`, `--thinking`, `--max-thinking-tokens`, `--strict-mcp-config`, `--betas`, `--permission-prompt-tool`.

Removed subcommands: `daemon`, `logs`, `attach`, `kill`, `remote-control`, `rc`, `bridge`, `sync`, `upgrade`, `usage`.

Two renames deserve their own note because the old spelling is muscle memory:

- **`--model` is gone; use `--models.main`.** The plain `--model` flag is rejected. The same applies in settings files, where `model` is rejected in favor of `models.main`.
- **`--restore` never existed; use `--resume` or `-r`.**

For `--debug` and `--verbose`, the replacement is `--log-level debug`. For `--output-format json`, the replacement is `coco sdk`, which speaks NDJSON, or `--log-format json` if you only wanted structured logs.

## Examples

Run a single prompt and print the answer. The `-p` flag is what makes this non-interactive:

```bash
coco -p "Explain what this repository does"
```

Pipe a prompt in from a file or another command. Standard input not being a terminal is enough to select headless mode, so no flag is needed:

```bash
cat task.md | coco
```

Continue the most recent conversation in the interactive UI, or resume a specific session:

```bash
coco --continue
coco --resume 11111111-2222-3333-4444-555555555555
```

Fork a session instead of continuing it — the history is copied into a brand-new session and the original is left untouched:

```bash
coco --resume 11111111-2222-3333-4444-555555555555 --fork-session
```

Run against a different project without changing your shell's directory:

```bash
coco --cwd /srv/app -p "Summarize the test failures"
```

Override the Main model for one run, with two fallback tiers for capacity errors. Fallbacks are tried in flag order:

```bash
coco --models.main anthropic/claude-sonnet-4-6 \
     --fallback-model openai/gpt-5 \
     --fallback-model google/gemini-2.5-pro \
     -p "Refactor the auth module"
```

Turn up logging while debugging a print-mode run. `--log-stderr` puts the logs on stderr next to the response, so the response itself stays clean on stdout:

```bash
coco --log-level debug --log-stderr -p "Why is the build failing?"
```

Ask for a full `EnvFilter` directive when a bare level is too broad. A bare level expands to `coco=<level>,<level>`; anything more specific is passed through verbatim:

```bash
coco --log-level 'coco=trace,coco_inference::stream=trace,info' -p "..."
```

Constrain the tool surface for an automated run, and request structured output validated against an inline schema:

```bash
coco -p "List the crates in this workspace" \
     --allowed-tools Read Glob Grep \
     --json-schema '{"type":"object","properties":{"crates":{"type":"array","items":{"type":"string"}}},"required":["crates"]}'
```

Give a session access to a directory outside the working tree:

```bash
coco --add-dir /srv/shared-assets --add-dir /opt/vendor
```

Restrict which settings layers load. This is useful in CI, where you want the project's checked-in settings but not a developer's machine-level ones. Note that Flag and Policy layers always load regardless:

```bash
coco --setting-sources project -p "Run the test suite and report failures"
```

## `--version` output

`coco --version` prints three lines: the semantic version, the commit that produced the build with its date and subject line, and the build timestamp. The commit and build metadata are stamped in at compile time — the short hash and commit date come from `git` at build time, and the build time is a UTC timestamp. Each component falls back to `unknown` when git is unavailable.

The shape is:

```
<semver>
commit: <short-hash> (<YYYY-MM-DD>) <commit subject>
built:  <YYYY-MM-DD HH:MM:SS UTC>
```

Use this rather than `coco status`, whose version line is hardcoded to `coco-rs v0.0.0` and tells you nothing about the build you are running.

## Cargo features that change the binary

cocode's binary is built from the `coco-cli` crate, and four cargo features change what the resulting `coco` can do. If you build from source, these are opt-in; a distribution build may enable some of them. A build that lacks a feature still accepts the corresponding flags — it fails at runtime or degrades instead.

**`jemalloc`** switches the global allocator to jemalloc, tuned for a long-running process, and enables the TUI's end-of-turn arena purge. It is off for plain `cargo build` so development builds stay fast, and it is never active on Windows. This changes memory behavior only; no flag or command depends on it.

**`voice`** compiles the real cpal microphone backend for voice input. Without it, the voice subsystem is still present but reports that no microphone is available. Note that the `voice` feature gate in settings is a separate thing and is off by default regardless — see [configuration](configuration.md).

**`voice-local`** implies `voice` and additionally compiles the on-device Whisper backend. It is off by default because it needs a C++ toolchain (cmake and libclang) at build time.

**`serve-hub`** embeds the local Event Hub server. **This one has a visible failure mode**: `--serve-hub` is always accepted by the argument parser, but on a binary built without the feature it hard-errors at startup with a message telling you to rebuild:

```
This `coco` build was not compiled with the `serve-hub` feature. Rebuild with
`cargo build -p coco-cli --features serve-hub`. Alternatively, run a separate
`coco-hub-server serve` and pass `--event-hub-url ws://127.0.0.1:8731/v1/connect`.
```

As the message suggests, running a standalone hub and pointing `--event-hub-url` at it works on any build.

## See also

- [Configuration](configuration.md) — settings files, merge order, feature gates, environment variables.
- [Providers and authentication](providers-and-auth.md) — what `coco login` talks to.
- [Models and MoA](models-and-moa.md) — what `--models.main` and `coco moa` configure.
- [Permissions](permissions.md) — what `--permission-mode` and `--dangerously-skip-permissions` control.
- [Sandbox](sandbox.md) — sandboxed shell execution.
