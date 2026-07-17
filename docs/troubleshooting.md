# Troubleshooting

Common failures, what actually causes them, and how to fix them. Every error
message quoted here is the real string the code emits, so you can search this
page for what your terminal printed.

## cocode refuses to start: no Main model configured

```text
no Main model configured: set `models.main` in settings.json, pass
`--models.main <provider>/<model_id>`, or set `COCO_MODEL_MAIN=<provider>/<model_id>`
```

**Cause.** There is no default model. cocode is multi-provider and will not
guess a vendor for you, so the `Main` role must be bound explicitly before a
session can start.

**Fix.** Any one of the three the message names. In precedence order, highest
first:

```bash
coco --models.main anthropic/claude-sonnet-4-6      # this run only
export COCO_MODEL_MAIN=anthropic/claude-sonnet-4-6  # this shell
```

```jsonc
// ~/.cocode/settings.json — the durable form
{
  "models": {
    "main": "anthropic/claude-sonnet-4-6"
  }
}
```

The value is always `provider/model_id`, where `provider` is a key in your
provider catalog rather than a vendor's marketing name.

Two related gotchas. There is no `--model` flag — it was renamed to
`--models.main`, and a bare `model` key in `settings.json` is **rejected**
rather than ignored. And `COCO_MODEL_MAIN` is deliberately environment-only: it
is the escape hatch that has to work before `settings.json` is parsed. Other
roles go through `settings.models.*`.

See [models and MoA](models-and-moa.md) for the full role list.

## unknown model

```text
unknown model `<provider>/<model>` — not in builtin registry, models.json, or per-provider models
```

**Cause.** You named a model that cocode cannot find anywhere. The three places
it looks are exactly the three the message lists.

**Fix.** A model is "known" if it appears in any one of:

- the **builtin registry** compiled into the binary;
- **`~/.cocode/models.json`**, the provider-agnostic model catalog;
- the **`models` map of a provider entry** in `~/.cocode/providers.json`, which
  is where per-provider overrides live.

So the usual causes are a typo in the model id, a provider prefix that does not
match any configured provider, or a brand-new model the bundled registry has not
caught up with. For the last case, add it to `models.json` or to that provider's
`models` map and it becomes known immediately.

There is a matching error for the provider half:

```text
unknown provider `<name>` referenced by role binding
```

That one means the left side of `provider/model_id` is not a key in your
provider catalog.

## A provider has no API key

**Cause.** Each provider declares an `env_key` — the environment variable it
reads. If neither that variable nor a fallback key in the config is present, the
provider is unavailable with reason `MissingApiKey { env_key }`.

**Fix.** Export the variable the provider declares:

```bash
export DEEPSEEK_API_KEY="sk-..."
```

You can also store a key in `~/.cocode/providers.json` as `api_key`:

```jsonc
{
  "deepseek-openai": {
    "env_key": "DEEPSEEK_API_KEY",
    "api_key": "sk-..."   // fallback only — the env var still wins
  }
}
```

**The environment variable takes precedence.** Key resolution is literally
"env var, then config `api_key`, then nothing," so exporting a stale value in
your shell will silently shadow the key in your config file. If a key you just
edited into `providers.json` seems to have no effect, check for an exported
variable of the same name first — that is nearly always the answer.

See [providers and authentication](providers-and-auth.md) for the provider
catalog and the `env_key` each one uses.

## NotLoggedIn — a subscription provider is not authenticated

```text
not logged in for provider '<provider>': run `coco login <provider>`
```

**Cause.** Some providers authenticate by OAuth subscription rather than an API
key. When one has no stored credential, it reports `NotLoggedIn`. Setting an API
key will not help — it is the wrong mechanism for that provider.

The inverse mistake has its own message. Running `coco login` against a provider
that uses an API key tells you so rather than starting a pointless OAuth flow:

```text
provider '<name>' authenticates with an API key, not OAuth login — set its env var (or `api_key` in providers.json) instead
```

**Fix.**

```bash
coco login openai      # provider defaults to `openai` if omitted
```

Useful variants: `--no-browser` (aliased `--headless`) prints the authorization
URL instead of opening a browser, which is what you want over SSH; `--import
<PATH>` reads a credential from another tool's auth file once, without modifying
it. `coco logout` clears stored credentials.

Inside a session, bare `/login` opens a picker of OAuth-capable providers.

**Checking state.** `coco status` and `coco doctor` both print a provider-login
block:

```text
provider login:
  [openai] logged in (you@example.com)
  [gemini] not logged in — run `coco login`
  [grok] expired (auto-refresh failed)
```

Read this carefully: it only lists providers whose auth mode is **OAuth**.
API-key providers are skipped entirely, so a provider's absence from this list
is not evidence that its key is missing.

## Where the logs are

Logs live in `~/.cocode/logs/`. The filename is **per-process and dated**:

```
~/.cocode/logs/coco.<pid>.log.<YYYY-MM-DD>
```

The PID keeps concurrent cocode sessions from interleaving into one file, and
the date suffix comes from daily rotation. This trips people up because
`--log-file`'s own help text describes the default as `<config_home>/logs/coco.log`
— the directory is right, but the real filename carries the PID and date. To
find the newest log:

```bash
ls -t ~/.cocode/logs/ | head
```

Override the path with `--log-file <PATH>` or `COCO_LOG_FILE`.

**Stdout is reserved** — the TUI paints to it and SDK mode writes NDJSON to it —
so logs never go there. They go to the file sink, and to stderr only when you
ask. Add `--log-stderr` (or `COCO_LOG_STDERR`) to get a stderr layer alongside
the file, which is what you want when debugging a `-p` run and would like the
logs next to the response. Headless mode enables stderr logging by default.

### Filter precedence

The tracing filter resolves highest-priority first:

```
--log-level  >  COCO_LOG  >  RUST_LOG  >  settings.log.level  >  the default
```

The default is:

```text
coco=debug,info
```

`--log-level` accepts either a bare level, which expands to `coco=<level>,<level>`,
or a full `EnvFilter` directive that is passed through untouched. An invalid
directive fails fast rather than falling back:

```text
invalid tracing filter "<yours>": <parse error>
```

```bash
coco --log-level debug -p "why did that tool call fail?"
COCO_LOG='coco_query=trace,coco_inference=debug,info' coco
```

### Format and layout

`--log-format` takes `pretty`, `compact`, or `json`. The default depends on
mode: `json` for SDK, `compact` for TUI and headless. `--log-location` shows
source `file:line` and thread name; it is tri-state, so bare or `=true` forces
it on, `=false` forces it off, and omitting it auto-enables when the resolved
filter is `debug` or `trace`. `--log-timezone` takes `local` (default) or `utc`.

Each of these has an environment counterpart — `COCO_LOG_FORMAT`,
`COCO_LOG_FILE`, `COCO_LOG_STDERR`, `COCO_LOG_LOCATION`, `COCO_LOG_TIMEZONE` —
which sit below the flags and above `settings.log.*`.

## Diagnosing provider wire issues

When a model misbehaves and you need to see what actually went over the wire,
turn on the wire dumper. It is **off by default**.

```jsonc
// ~/.cocode/settings.json
{
  "diagnostics": {
    // off | error | all
    "wire_dump": "error",

    // Max bytes kept per request/response body. Default 1048576 (1 MiB).
    "wire_dump_max_body_bytes": 1048576,

    // Redact known secret patterns before writing. Leave this on.
    "wire_dump_redact": true
  }
}
```

The three accepted levels are:

| Value | Behavior |
|---|---|
| `off` | **Default.** No capture, zero overhead — the recorder is not even constructed. |
| `error` | Persist only calls that failed. The right setting for "it breaks occasionally." |
| `all` | Persist every request and response. Verbose; use for a targeted repro. |

Parsing is tolerant of synonyms: `false`, `0`, `none`, and empty all mean `off`;
`errors` and `error_only` mean `error`; `true`, `1`, and `full` mean `all`.

The environment variable `COCO_DIAGNOSTICS_WIRE_DUMP` overrides the setting and
takes the same tokens. An unrecognized value is ignored with a warning:

```text
ignoring COCO_DIAGNOSTICS_WIRE_DUMP: expected off|error|all
```

**Where the dumps land.** Records are written per session under:

```
~/.cocode/projects/<project-slug>/<session-id>/wire/
```

Inside, `index.jsonl` always gets one line per call — that is your index even at
`error` level. Each persisted call additionally writes a triplet named
`<seq>-turn-<n>-<provider>` with a `.req.json`, `.resp.txt`, and `.meta.json`
alongside it. Subagent traffic is nested under `wire/subagents/agent-<id>/`, so
a misbehaving subagent's calls stay separate from the parent's.

Two behaviors worth knowing. A retried call captures the **final** attempt
rather than a concatenation of all of them, because each new request resets the
response buffers. And `wire_dump_redact` is on by default and should stay on —
it strips known secret patterns before anything touches disk. Turning it off
means writing raw credentials into a file.

## macOS keeps prompting for keychain access

**Cause.** You are almost certainly running a locally built binary that is
reaching for the OS keychain, or you have `COCO_AUTH_CREDENTIAL_STORE` set to
force it.

Official release builds default to the **`auto`** backend, which tries the
keychain first and falls back to a file. **Any locally built binary — including
`cargo build --release` — defaults to `file` instead**, precisely so a
development build never touches your keychain and never triggers an "allow
access" prompt. If a build of your own is prompting, something is overriding
that default.

**Fix.** Force the backend explicitly:

```bash
export COCO_AUTH_CREDENTIAL_STORE=file
```

| Value | Behavior |
|---|---|
| `auto` | Keychain first, file as fallback. Default for official builds. |
| `file` | Only `~/.cocode/auth/<provider>.json`, directory `0700`, files `0600`. Default for local builds. |
| `keyring` | OS keychain only, under the service name `cocode Provider Auth`. Errors if unavailable. |
| `ephemeral` | In memory only. Nothing persists — you re-authenticate every run. |

Values are case-insensitive. Precedence is environment variable, then
`auth_credential_store` in `global.json`, then the build default; project
settings deliberately cannot influence it. An unrecognized value logs a warning
and falls through to the next layer rather than failing.

## --serve-hub fails at startup

```text
This `coco` build was not compiled with the `serve-hub` feature. Rebuild with
`cargo build -p coco-cli --features serve-hub`. Alternatively, run a separate
`coco-hub-server serve` and pass `--event-hub-url ws://127.0.0.1:8731/v1/connect`.
```

**Cause.** `--serve-hub` is always accepted by the argument parser, but the
embedded Event Hub server is behind an optional cargo feature that is not
compiled into every build. The flag parses and then hard-errors.

**Fix.** Either rebuild with the feature, or do what the message suggests and
run a standalone hub, pointing `--event-hub-url` at it. The second option works
on any build and is usually the faster path.

## Context and compaction

Two different commands, frequently confused:

- **`/context`** — "Show context window usage breakdown". Read-only. It tells
  you what is consuming your context window right now. Start here.
- **`/compact`** — "Compact conversation to reduce context usage". This
  *changes* your conversation by summarizing it. It takes optional
  instructions, so `/compact focus on the auth refactor` steers what the summary
  preserves.

Reach for `/context` first to see whether compaction is warranted, then
`/compact` if it is. Compaction also happens automatically when a turn would
otherwise overflow.

If `/compact` is missing from your session, check whether
`COCO_COMPACT_DISABLE` is set — the command hides itself when that variable is
truthy.

## Reporting a bug

Run **`/feedback`** in a session. It generates a prefilled GitHub issue URL
against the project's tracker with your version, commit, build time, OS, arch,
and a timestamp already filled in:

```
https://github.com/collab-ai-dev/cocode/issues/new
```

Logs are **excluded by default**. `/feedback --with-logs` attaches a
best-effort redacted tail of the current log — review it before submitting,
since redaction is best-effort and not a guarantee.

There is no `/bug` command; `/feedback` is the only spelling.

## A note on `doctor` and `status`

Each of these names covers **two different implementations**, and the difference
matters.

The bare **CLI subcommands** are thin. `coco doctor` prints two lines that are
hardcoded literals with no check behind them — they print no matter what:

```text
[ok] Shell: available
[ok] Config: loaded
```

`coco status` opens with a **hardcoded `coco-rs v0.0.0`** that is not derived
from the build at all and should simply be ignored. In both commands, only the
resolved model line and the provider-login block do real work.

The **slash commands** run inside a session and are the real thing. `/doctor`
actually shells out to check `git`, your `$SHELL`, the home and config
directories, and optionally `gh` and `node`. `/status` reports live session
state: resolved Main and Fast models, permission mode, effective thinking level,
the plan-mode gate, and connected MCP servers.

So: prefer `/doctor` and `/status` from within a session. Treat the bare CLI
subcommands as a quick model-and-auth echo, not a health check.

For an actual version, use `coco --version`, which reports the real version,
commit, and build time. For real diagnosis, the log file and the wire dumper
above are what will tell you something.

## See also

- [Configuration](configuration.md) — settings files, merge order, environment variables.
- [Providers and authentication](providers-and-auth.md) — the provider catalog and credential storage.
- [Models and MoA](models-and-moa.md) — model roles and fallback chains.
- [CLI reference](cli-reference.md) — every flag, and how the run mode is chosen.
