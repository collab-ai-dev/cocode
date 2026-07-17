# Getting started

This page takes you from nothing installed to a working agent doing real work in
your repository.

## 1. Install

```bash
npm install -g @cocode-cli/cocode-cli
cocode-cli --version
```

The npm package installs a small JavaScript launcher plus the native `coco`
binary for your platform. Prebuilt binaries exist for Linux x86_64, Linux
aarch64, and macOS Apple Silicon. macOS Intel and Windows are not published — on
those, build from source.

The binary is self-contained: it does not need `ripgrep` or any other CLI
installed alongside it.

### From source

You need Rust 1.93.1, which `rust-toolchain.toml` pins and `rustup` installs
automatically, plus [`just`](https://github.com/casey/just).

```bash
git clone https://github.com/collab-ai-dev/cocode.git
cd cocode/coco-rs
just coco               # build and launch
just coco -- --version  # pass arguments through
```

### About the names

The binary is `coco`. The npm launcher is `cocode-cli`. `--help` prints
`cocode`. They are the same program — this page uses `cocode-cli` for
npm-installed setups and `coco` for from-source ones.

## 2. Choose a model

cocode ships no default model, and refuses to start without one:

```text
no Main model configured: set `models.main` in settings.json, pass
`--models.main <provider>/<model_id>`, or set COCO_MODEL_MAIN=<provider>/<model_id>
```

This is deliberate. cocode is multi-provider, and guessing a vendor for you
would be a worse default than asking.

Pick one of the two authentication paths.

### Path A — an API key

```bash
export DEEPSEEK_API_KEY="sk-..."
cocode-cli --models.main deepseek-openai/deepseek-v4-flash
```

Any of these work the same way, each reading its own environment variable:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."   # anthropic/claude-sonnet-4-6
export OPENAI_API_KEY="sk-..."          # openai/gpt-5-5
export GOOGLE_API_KEY="..."             # google/gemini-3.1-pro-preview
export XAI_API_KEY="..."                # xai/grok-code-fast-1
export GROQ_API_KEY="gsk_..."           # groq/<model>
```

### Path B — a subscription you already pay for

```bash
coco login openai     # ChatGPT subscription  → openai-chatgpt/gpt-5.5
coco login gemini     # Gemini Code Assist    → gemini-code-assist/gemini-2.5-pro
coco login grok       # Grok subscription     → grok/grok-code-fast-1
```

`coco login grok` uses a device-code flow, so it works over SSH with no local
browser. The other two open a browser to a loopback callback; add `--no-browser`
to print the URL instead.

Anthropic models are API-key only — there is no Claude subscription login.

## 3. Make it permanent

Create `~/.cocode/settings.json`:

```jsonc
{
  "models": {
    // Required. Every other role falls back to this one.
    "main": "deepseek-openai/deepseek-v4-flash"
  }
}
```

Settings files are JSONC, so comments are allowed and encouraged.

Now `cocode-cli` starts with no arguments.

## 4. First run

```bash
cd /your/project
cocode-cli
```

You get a full-screen TUI. Type a request in plain language:

```text
What does this repository do? Read the top-level README and the main entry point.
```

A few things worth knowing on your first session:

| Key | Does |
| --- | --- |
| `Enter` | Send |
| `Esc` | Cancel the current turn |
| `Shift+Tab` | Cycle permission mode |
| `Tab` | Toggle plan mode |
| `Ctrl+T` | Cycle thinking level |
| `Ctrl+O` | Toggle the transcript view |
| `Ctrl+C` twice | Quit |

When the agent wants to run a command or edit a file, it asks. That prompt is
the permission system doing its job — see [permissions](permissions.md) to make
it less chatty in ways you control.

## 5. Your first real task

Two habits make cocode much more useful than a chat window.

**Use plan mode for anything non-trivial.** Press `Tab`, or:

```text
/plan Add retry with exponential backoff to the HTTP client
```

In plan mode the agent researches read-only and proposes a plan before it is
allowed to change anything. You approve, then it executes. If you configure a
`plan` role, it can even think with a stronger model while planning and drop
back to a cheaper one to execute.

**Give it a project brief.** Run:

```text
/init
```

This writes a `CLAUDE.md` describing your project — build commands, conventions,
architecture. cocode discovers it automatically on every future session in that
directory, so you stop re-explaining your repo. See [memory](memory.md).

## 6. Non-interactive use

`-p` runs one prompt and exits, which is what makes cocode scriptable:

```bash
cocode-cli -p "List every crate in this workspace and its purpose"
```

```bash
# Review a diff in CI
git diff main | cocode-cli -p "Review this diff for correctness bugs"
```

Non-interactive mode also engages automatically when stdin or stdout is not a
TTY, so pipes work as you would expect. There is no `--no-tui` flag.

For structured output, pass an inline JSON Schema:

```bash
cocode-cli -p "Summarize this repo" --json-schema '{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]}'
```

## Where to go next

- [Configuration](configuration.md) — every settings file and feature gate
- [Providers and authentication](providers-and-auth.md) — add your own provider
- [Models and MoA](models-and-moa.md) — roles, fallbacks, Mixture of Agents
- [Slash commands](slash-commands.md) — everything you can type
- [Permissions](permissions.md) — tune how often it asks
- [Troubleshooting](troubleshooting.md) — when something goes wrong
