# CLAUDE.md

Multi-provider LLM SDK and CLI. All development in `coco-rs/`.

>
> **Each crate has its own `CLAUDE.md`** with key types, invariants, and design notes — read it when working in that crate. This root file covers conventions and high-level structure only.

## Commands

Run from `coco-rs/` directory.

**Default to the composite recipes — don't chain `check` + `clippy` + `test`
manually.** Clippy is a superset of `cargo check` (it runs the same
type-check pass plus lints), and `test` (nextest) compiles every
`--tests` target clippy already linted, so the manual chain duplicates
work. Pick one of:

```bash
just fmt          # After Rust changes (auto-approve)
just quick-check  # Iteration: fmt + seam guard + check-error-policy + incremental clippy. NO tests.
just pre-commit   # REQUIRED before commit: quick-check + check-docs + nextest + Python SDK checks
```

### Pre-commit is expensive — run it ONCE per commit, at the very end

Every iteration uses `quick-check`; `pre-commit` (full test-binary compile +
nextest run, orders of magnitude slower) fires exactly once, immediately
before `git commit` — never mid-iteration. Why: clippy/check artifacts
(`.rmeta`) and test binaries are separate cargo caches that do not share, so
a clippy failure inside `pre-commit` surfaces only after nextest has already
burned its compile budget. `quick-check` catches the same warnings without
entering the test-compile path.

Linker: `coco-rs/.cargo/config.toml` sets `mold` on Linux (`apt install mold`
once); linking dominates test-binary build time.

Scoped helpers (use only when you genuinely need a single piece):

```bash
just test               # Full nextest run; needed when you changed shared crates
just test-crate <name>  # Scope tests to one crate
just fix -p <name>      # Auto-fix clippy warnings for one crate
just clippy             # Force full-workspace clippy (rare; clippy-affected is the default)
just check              # Bare cargo check (rarely useful — clippy already does this)
just help               # All commands
```

**Path conventions** (avoids the `coco-rs/coco-rs/...` mistake):
- `just` / `cargo` commands: run from `coco-rs/`; paths are workspace-relative (`app/query/src/...`).
- `git` commands: also fine to run from inside `coco-rs/` — paths stay workspace-relative (`app/query/src/...`), NOT prefixed with `coco-rs/`.
- The `coco-rs/` prefix only appears when viewing output from the repo root (e.g. session-start `gitStatus`, or `git` run from the repo root). Don't copy those paths verbatim into commands run from `coco-rs/`.

## Code Style

### Format and Lint

- When using `format!` and you can inline variables into `{}`, always do that: `format!("{name} is {age}")`, never `format!("{} is {}", name, age)`
- Collapse `if` per [collapsible_if](https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if)
- Prefer method references over closures per [redundant_closure_for_method_calls](https://rust-lang.github.io/rust-clippy/master/index.html#redundant_closure_for_method_calls)
- Make `match` exhaustive; avoid wildcard arms

### Integer Types

- Use `i32`/`i64` instead of `u32`/`u64` unless bit-pattern required — avoids overflow bugs, matches common APIs

### String Slicing — UTF-8 safety

Slicing a `&str` by a **computed byte offset** panics when the offset lands
inside a multi-byte char (CJK, box-drawing glyphs, emoji) — this has shipped
as a live crash more than once. **Never** write `&s[..n]` / `s.truncate(n)`
with a computed index, and never hand-roll `is_char_boundary` loops. Blessed
forms:

- `coco_utils_string::{take_bytes_at_char_boundary, take_last_bytes_at_char_boundary}`
  (borrowed prefix/suffix); `truncate_str` / `truncate_for_log` (owned + `...`).
- `s.floor_char_boundary(n)` for an in-place truncate without adding a dep:
  `content.truncate(content.floor_char_boundary(n))` — never `content.truncate(n)`.
- Truncating to **terminal columns** (display width) is a different problem —
  use `coco_tui_ui::truncate::truncate_to_width` (width-aware, CJK = 2 cols).

Raw slicing is only safe at a literal index on known-ASCII, or at a
`.find()`/`rfind()` result for a single-byte char like `\n`.

### Error Handling

- Never `.unwrap()` in non-test code. Use `?` or `.expect("reason")`
- Avoid mixing `Result` and `Option` without clear conversion
- Per-layer error types: see [Error Handling](#error-handling) below

### Serde Conventions

- `#[serde(default)]` for optional config fields
- `#[derive(Default)]` for structs used with `..Default::default()`
- `#[serde(rename_all = "snake_case")]` for enums

### Parameter Design

- Avoid bool/ambiguous `Option` params that produce opaque callsites like `foo(false)` or `bar(None)`
- Prefer enums, named methods, newtypes

### Argument Comments

When a positional literal is unavoidable, add `/*param_name*/` matching the callee signature:

```rust
connect(/*timeout*/ None, /*retries*/ 3, /*verbose*/ false)
```

Add for `None` / booleans / numeric literals. Skip for string/char literals unless the comment adds real clarity.

### Module Size

- Target Rust modules under 800 LoC (excluding tests)
- Files > ~1600 LoC: create a new module instead of extending

### Comments

- Concise; describe purpose, not implementation
- Field docs: 1-2 lines, no example configs
- Code comments: only when intent is non-obvious

### Commit Messages (Conventional Commits)

- Subject: `<type>(<scope>): <summary>` — imperative mood, ≤72 chars, no period.
  Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `perf`, `ci`, `build`, `style`, `revert`.
- Body (optional): wrap at 72 cols, blank line after subject. One short bullet per
  logical change — explain *why*, not *what* the diff already shows. Skip
  per-file recaps, test counts, and rote "verified" lines unless load-bearing.
- Footers: `BREAKING CHANGE:` and `Co-Authored-By:` only.
- Squash commits: keep subject ≤72 chars; body = 4–8 bullets max grouped by
  theme. Don't paste full per-commit bodies — synthesize.

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│  App:         cli, agent-host, sdk-server, runtime, server,          │
│               server-client, server-transport, tui, session, query   │
│  Hub:         protocol, connector, server                            │
│  Root:        commands, skills, hooks, tasks, memory, maintenance,   │
│               coordinator, plugins, keybindings, output-styles,      │
│               skill-learn, journey                                   │
│  Core:        tool-runtime → tools, permissions, messages, context,  │
│               system-reminder, subagent, goals, goal-runtime,        │
│               workflow, workflow-runtime                             │
│  Services:    inference, compact, mcp, mcp-skills, mcp-types,        │
│               rmcp-client, provider-auth, lsp, session-trace,        │
│               wire-dump                                              │
│  Exec:        shell, sandbox, process-hardening, exec-server,        │
│               apply-patch                                            │
│  Standalone:  bridge, retrieval, voice, tui-ui, tui-markdown,        │
│               tui-mermaid                                            │
│  Vercel AI:   ai → openai, openai-compatible, google,                │
│               google-codeassist, xai, groq, anthropic, bytedance     │
│               (on provider + provider-utils)                         │
│  Common:      types, llm-types, config, config-reload, error, otel,  │
│               stack-trace-macro, model-card                          │
│  Utils:       see Utils table below                                  │
└──────────────────────────────────────────────────────────────────────┘
```

Lower layers depend on nothing above them. See each crate's `CLAUDE.md` for its key types and invariants.

**Before implementing any basic capability, scan the Utils table below** — if a crate already provides it (path handling, caching, git, encoding, ignore rules, fuzzy search, frontmatter, secret redaction, …), use it rather than rolling your own.

## Key Data Flows

### Agent Turn Lifecycle (driven by `app/query::QueryEngine`)

```
User input → ConversationContext → MessageHistory + attachments
  → ApiClient.query → vercel-ai stream
  → StreamAccumulator → StreamingToolExecutor (safe concurrent / unsafe queued)
  → Hook orchestration (Pre/PostToolUse)
  → Tool results → MessageHistory → emit CoreEvent (Protocol/Stream/Tui)
  → Check ContinueReason → maybe compact (micro/full/reactive)
  → Drain CommandQueue → loop if tool calls remain
```

### Configuration Resolution

```
<config_home>/settings.json (default ~/.cocode) + project .cocode/settings.json
  + managed/policy files + env + CLI overrides
  → Settings → EnvOnlyConfig → RuntimeOverrides → ResolvedConfig
  → GlobalConfig (hot-reload via SettingsWatcher)
  → ModelRoles + ModelAlias + FastModeState
  → BootstrapConfig → app/query → services/inference
```

### Provider Call Chain

```
QueryEngine → ApiClient [services/inference]
  → Arc<dyn LanguageModelV4>.do_generate / do_stream (production bypasses
    vercel-ai's generate_text / stream_text — see vercel-ai/ai/CLAUDE.md)
  → provider impl [vercel-ai/{openai,anthropic,google,google-codeassist,xai,groq,bytedance,openai-compatible}]
  → HTTP → typed stream → CacheBreakDetector + UsageAccumulator → QueryResult
```

For shell execution, MCP integration, background tasks: see the respective crate's CLAUDE.md (`exec/shell`, `services/mcp`, `tasks`).

## Crate Guide

One-line purposes. For key types and details, open each crate's own `CLAUDE.md`.

### Common

| Crate | Purpose |
|-------|---------|
| `types` | Foundation types incl. Message family + wire-tagged unions. Source-level vercel-ai-free (DTOs reach via `coco-llm-types`). |
| `llm-types` | DTO seam: pure re-export shim for `vercel-ai-provider` shapes; SDK upgrades edit this + `services/inference` (dual-seam) |
| `config` | Layered config: JSON + env + runtime overrides + hot reload |
| `config-reload` | Settings hot-reload watcher (`RuntimeReloader` over settings files + `managed-settings.d`) |
| `error` | Unified errors with `StatusCode` classification (snafu + virtstack) |
| `otel` | OpenTelemetry tracing and metrics |
| `stack-trace-macro` | `#[stack_trace_debug]` proc macro for snafu enums |
| `model-card` | Vendor-defined model facts (knowledge cutoff, pricing, deprecation). Exact-id lookup; no substring matching |

### Vercel AI

| Crate | Purpose |
|-------|---------|
| `vercel-ai-provider` | Standalone types matching `@ai-sdk/provider` v4 (no coco deps) |
| `vercel-ai-provider-utils` | Utilities for AI SDK v4 providers (Fetch, ResponseHandler, Schema) |
| `vercel-ai` | High-level SDK matching `@ai-sdk/ai` (generate_text, stream_text, …) |
| `vercel-ai-openai` | OpenAI provider (Chat + Responses + Embeddings + Image) |
| `vercel-ai-openai-compatible` | Generic OpenAI-compatible provider (Together, Fireworks, DeepSeek, GLM, Ollama) |
| `vercel-ai-google` | Google Gemini provider (API key → generativelanguage) |
| `vercel-ai-google-codeassist` | Gemini Code Assist subscription transport (OAuth → cloudcode-pa); reuses `vercel-ai-google`'s codec |
| `vercel-ai-anthropic` | Anthropic Claude provider |
| `vercel-ai-xai` | xAI (Grok) provider: Chat + Responses + multimodal incl. streaming STT; Grok subscription transport |
| `vercel-ai-groq` | Groq provider: Chat + transcription; `x_groq.usage` streaming usage; `browser_search` tool |
| `vercel-ai-bytedance` | ByteDance Seedance video provider (coco-original; no TS counterpart) |

### Services

| Crate | Purpose |
|-------|---------|
| `inference` | Thin multi-provider wrapper: generic retry, usage aggregation, tool-schema, cache-break. Auth/caching/betas live in provider crates |
| `compact` | Context compaction: full / micro / reactive / session-memory + auto-trigger |
| `mcp` | MCP server lifecycle, OAuth (incl. xaa IDP), elicitation, channel permissions |
| `mcp-types` | Auto-generated MCP message types (regenerate, never hand-edit) |
| `mcp-skills` | MCP-provided skills bridge (`skill://` sources) |
| `rmcp-client` | MCP client: stdio + HTTP/SSE transport, OAuth persistence |
| `provider-auth` | Provider OAuth flows (device flow, PKCE), token store/refresh, cross-process lock |
| `lsp` | AI-friendly LSP (query by name+kind, not position); rust-analyzer/gopls/pyright/tsserver |
| `session-trace` | Non-invasive session event tee + replay bundles |
| `wire-dump` | Redacted HTTP wire capture for diagnostics (`COCO_DIAGNOSTICS_WIRE_DUMP`) |

### Core

| Crate | Purpose |
|-------|---------|
| `tool-runtime` | `Tool` trait, streaming executor, registry, callback handles (interface layer) |
| `tools` | Built-in tool impls (File I/O, Web, Agent, Task, Plan, Shell, MCP mgmt, Scheduling) |
| `permissions` | Evaluator + 2-stage auto-mode/yolo XML-LLM classifier + bypass killswitch |
| `messages` | MessageHistory, normalization/filtering/predicates, cost tracking |
| `context` | System context assembly, CLAUDE.md discovery, attachments, plan-mode reminders |
| `system-reminder` | Dynamic `<system-reminder>` injection: trait-based generators + parallel orchestrator (cadence is generator-owned) |
| `subagent` | Pure-logic subagent rules: definition catalog, source precedence, AgentTool prompt rendering, tool filter planning |
| `goals` | Pure goal domain: `decide` reducer + `GoalSnapshot` state machine, sealed completion authorization. No I/O |
| `goal-runtime` | Host orchestration over `coco-goals`: durable-before-visible transactions, `GoalStore` seam, completion gate, supervisor |
| `workflow` | Dynamic workflow source loading/validation: size cap, source-kind precedence, meta/script parsing |
| `workflow-runtime` | Embedded QuickJS sandbox executing workflow scripts (single-threaded engine on a `LocalSet`; determinism shim) |

### Exec

| Crate | Purpose |
|-------|---------|
| `shell` | Shell execution with security analysis, destructive warnings, sandbox decisions |
| `sandbox` | `SandboxMode`: `read_only` / `workspace_write` / `full_access` / `external_sandbox` (gated by `features.sandbox`, off by default) |
| `process-hardening` | OS-level security (macOS PT_DENY_ATTACH, Linux prctl) |
| `exec-server` | Minimal `ExecutorFileSystem` trait for local and remote execution |
| `apply-patch` | Unified diff/patch application with fuzzy matching |

### Root Modules

| Crate | Purpose |
|-------|---------|
| `commands` | Slash command registry + built-in command impls (v1/v2/v3) |
| `skills` | Skill markdown workflows (bundled / project / user / plugin) |
| `hooks` | Pre/post event interception with scoped priority, async registry, SSRF guard |
| `tasks` | Three kinds: `running` (bg tasks), `task_list` (durable plan items), `todos` (per-agent) |
| `memory` | Persistent cross-session: CLAUDE.md mgmt, auto-extraction, session memory, KAIROS auto-dream, team sync |
| `maintenance` | Cross-process maintenance lock + fail-closed write-fence for memory/skill upkeep |
| `skill-learn` | Skill review and curator workflows |
| `coordinator` | Multi-agent spawn, mailbox, teammate, worktree, and team lifecycle orchestration |
| `plugins` | Plugin system via `PLUGIN.toml` (contributions, marketplace, hot-reload) |
| `keybindings` | Shortcuts with context-based resolution and chord support |
| `output-styles` | Output-style catalog + loaders (built-in / project / user / managed / plugin) + system-prompt section |
| `journey` | Learning-timeline read-side assembler over the append-only journals (pure, no I/O; consumed by `app/cli`) |

### App

| Crate | Purpose |
|-------|---------|
| `cli` | Thin `coco` entry: clap schema, subcommand/process policy, mode selection, and TUI surface loop |
| `agent-host` | Agent-session host: session composition, local AppServer facade + handlers, headless ops, runtime integrations |
| `sdk-server` | SDK JSON-RPC/NDJSON adapter: stdio/sidecar transport, outbound writer, callback correlation, and AppServer bridge wiring |
| `runtime` | Transport-independent process/project resources, workspace paths, and session bootstrap contracts |
| `server` | Multi-session lifecycle/routing registry plus local and JSON-RPC adapters |
| `server-client` | Remote AppServer JSON-RPC client, per-surface demux, and transport owners; independent of the server implementation |
| `server-transport` | JSON-RPC framing and concrete stream/listener transports |
| `tui` | Terminal UI shell (TEA + rust-i18n): owns `AppState` and the AppState→Line projection; drives `coco-tui-ui` for painting |
| `tui-ui` (top-level crate) | Pure, domain-free presentational layer: scrollback paint engine, widgets, theme; no `AppState`, no i18n (seam-guarded) |
| `tui-markdown` (top-level crate) | Markdown → ratatui `Line`s renderer (LeadMarker structural contract; optional mermaid feature) |
| `tui-mermaid` (top-level crate) | Mermaid → ratatui `Line`s renderer (never panics — `catch_unwind`; graceful text fallback) |
| `session` | Session persistence, title generation, transcript recovery |
| `query` | Multi-turn agent loop driver (`QueryEngine`) + budget + command queue |

### Hub

| Crate | Purpose |
|-------|---------|
| `protocol` | Event Hub wire types, subprotocol constants, and envelopes |
| `connector` | Agent-side Event Hub boundary for future aggregation and WS sending |
| `server` | Hub-side Axum server, `EventStore` read model, local JSONL adapter, and web UI |

### Standalone

| Crate | Purpose |
|-------|---------|
| `bridge` | IDE bridge (VS Code/JetBrains), REPL bridge, JWT auth, trusted-device store |
| `retrieval` | BM25 + vector + AST + RepoMap (PageRank) via Facade; isolated `RetrievalEvent` stream |
| `voice` | Voice input (STT dictation): `VoiceEngine` trait, remote / local Whisper backends; isolated `VoiceEvent` stream; `Feature::Voice` |

### Utils

Reusable primitives. **Check here first** before implementing any basic utility.

| Crate | Purpose |
|-------|---------|
| `absolute-path` | Absolute path types with deserialization support |
| `async-utils` | Async runtime utilities and cancellation helpers |
| `audio-capture` | Microphone capture (cpal, feature-gated) → 16 kHz mono WAV for voice STT |
| `cache` | LRU cache with Tokio mutex protection |
| `cargo-bin` | Cargo binary helpers for test harnesses |
| `coco-cron` | Cron-subset parsing + scheduling in local timezone (missed-fire detection) |
| `coco-paths` | Canonical project-directory slug layout (Claude Code `sanitizePath` parity — never reimplement slug logic) |
| `common` | Shared cross-crate utility functions (incl. `COCO_CONFIG_DIR_NAME` config home, default `~/.cocode`) |
| `cursor` | Text cursor with kill ring (Ctrl+K/Y), word boundaries, UTF-8 safe |
| `download` | Streaming download-to-disk primitive (replaces buffered-in-memory fetches) |
| `file-encoding` | File encoding and line-ending detection/preservation |
| `file-ignore` | .gitignore-aware file filtering (unified ignore service) |
| `file-search` | Fuzzy file search with nucleo and gitignore support |
| `file-watch` | Generic reusable file-watch infrastructure |
| `frontmatter` | YAML frontmatter parser for skills, commands, agents, memory files |
| `git` | Git operations wrapper |
| `image` | Image processing utilities |
| `jemalloc` | jemalloc global-allocator wrapper (isolates the `unsafe`; `MALLOC_CONF` env gotcha) |
| `json-repair` | Tolerant JSON repair for LLM tool-call arguments (`llm_json` wrapper; post-stream only) |
| `json-to-toml` | JSON to TOML conversion |
| `keyring-store` | Secure credential storage using system keyring |
| `path-uri` | Typed path↔URI conversion (`PathUri` normalization, legacy-compat, bad-prefix sentinel) |
| `pty` | Pseudo-terminal handling |
| `readiness` | Readiness flag with token-based auth and async waiting |
| `rustls-provider` | TLS provider init via rustls crypto ring |
| `secret-redact` | Secret redaction (OpenAI, Anthropic, GitHub, Slack, AWS, bearer tokens) |
| `shell-discovery` | Login-shell / pwsh / Git-Bash discovery (platform-asymmetric strategies) |
| `shell-parser` | Shell command parsing and security analysis |
| `sleep-inhibitor` | Cross-platform sleep prevention (macOS/Linux/Windows) |
| `stdio-to-uds` | Bridge stdio streams to Unix domain sockets |
| `stream-parser` | Stream parsing (text, citation, inline hidden tag, proposed plan, UTF-8) |
| `string` | UTF-8-safe truncation / byte-boundary slicing — see [String Slicing](#string-slicing--utf-8-safety); never raw `&s[..n]` |
| `symbol-search` | Symbol search for code navigation |

(Workspace members outside these tables: test harnesses `tests/harness`, `tests/cassette`, `tests/live`, and the docs-gen build tool `xtask`.)

## Error Handling

Three tiers, each with one allowed error library. Pick by layer, not taste.

| Tier | Where | Library | Notes |
|------|-------|---------|-------|
| **3 (main trunk)** | `common/`, `core/`, `services/`, root modules, `app/query` | **snafu + `coco-error`** | Required when the error crosses ≥2 layers, drives retry / classification, or surfaces to users. Implement `ErrorExt` and pick a `StatusCode`. |
| **2 (boundary)** | `vercel-ai/*`, `utils/*` (libraries), `retrieval`, `bridge` | **thiserror** | Leaf libraries. No `coco-error` dep — main-trunk callers convert at the boundary via `boxed(err, StatusCode::X)`. |
| **1 (application)** | `app/cli`, `app/agent-host`, `app/tui`, `exec/shell`, `exec/exec-server`, tests, `[dev-dependencies]` | **anyhow** | Process/surface composition where errors are printed or translated to protocol results. Domain crates below this boundary keep typed errors. |

**Hard rules:**
- **No `pub fn ... -> anyhow::Result<_>` in `utils/*` or `vercel-ai/*`** — these are libraries; their public API must be a typed `Result<T, CrateError>`. Enforced by `just check-error-policy`.
- `[dev-dependencies]` is exempt — anyhow in tests is fine.
- A crate that depends on a third-party API returning `anyhow::Result` (e.g. `utils/pty` → `portable-pty`) may keep `anyhow` in `[dependencies]` for internal use, but its **own** public API must still return its own `thiserror` enum.

**StatusCode categories:** General (00-05), Config (10), Provider (11), Resource (12), SystemReminder (13). See [common/error/README.md](coco-rs/common/error/README.md).

## Testing

### Assertions

- Use `pretty_assertions::assert_eq` for clearer diffs
- Compare entire objects over individual fields
- Avoid mutating process env; pass flags/dependencies

### Organization — MANDATORY

Never inline `#[cfg(test)] mod tests { ... }`. Always companion file:

```rust
#[cfg(test)]
#[path = "implementation.test.rs"]
mod tests;
```

Tests go in `implementation.test.rs` alongside the source. Integration tests in `tests/`. Name descriptively: `test_<function>_<scenario>_<expected>`.

### Workflow

- Changed one crate: `just test-crate coco-<name>`
- Changed shared (common/, core/, services/): `just test`
- Clippy fix: `just fix -p coco-<name>`

- `cargo test` accepts only one positional test filter; use a shared substring/module prefix or run separate commands instead of passing multiple test names.

### Snapshot Tests (insta)

- UI changes require `insta` snapshot coverage
- Generate: `cargo test -p coco-tui`; Pending: `cargo insta pending-snapshots -p coco-tui`
- Review `*.snap.new` directly or `cargo insta show`; Accept: `cargo insta accept -p coco-tui`

## Async Conventions

- `tokio::task::spawn_blocking` for blocking ops
- Prefer `tokio::sync` primitives in async contexts
- `Send + Sync` bounds on traits used with `Arc<dyn Trait>`

## Tracing & Logging

The global `tracing` subscriber is installed once from the binary
(`app/cli/src/main.rs::main`). **Without that install every `tracing::*`
call is a no-op** — library / test code MUST NOT install one; tests use
`coco_otel::subscriber::init_for_tests` (`OnceLock`-guarded). Stdout is
reserved (TUI screen, SDK NDJSON RPC) — logs go to file/stderr sinks.

Filter/sink resolution, levels, span anchors, `#[instrument]` policy,
standard field names, secret redaction: see `common/otel/CLAUDE.md`
"Logging conventions" — adopt those field names verbatim so ops can pivot.

## Dependencies

Standard picks: `tokio` / `reqwest` / `serde_json` / `tracing`; errors per the
tier table above; testing with `pretty_assertions` + `insta` + `wiremock`;
TUI on `ratatui` + `crossterm`; MCP via `rmcp`. Prefer well-maintained
crates; check security advisories; use workspace deps.

## Design Decisions

### Code Hygiene

| Rule | Note |
|------|------|
| No deprecated code | Delete outright. No `#[deprecated]`, no backward-compat shims |
| No inline tests | Use `#[path = "<name>.test.rs"]` always |
| No `unsafe` | All safe Rust. Wrap unsafe deps in own crate. Truly unavoidable? Discuss first |
| No single-use helpers | Inline at the call site |
| Env vars use `COCO_*` | All coco-owned environment variables MUST use the `COCO_` prefix (third-party / SDK-vendor names like `ANTHROPIC_API_KEY` are exempt). Rename any legacy `CLAUDE_*` / `CLAUDE_CODE_*` / unprefixed names (e.g. `DISABLE_COMPACT`, `USE_API_CLEAR_TOOL_RESULTS`) to a namespaced `COCO_<DOMAIN>_<NAME>` form (e.g. `COCO_COMPACT_DISABLE`, `COCO_COMPACT_API_CLEAR_TOOL_RESULTS`). Add the variant to `coco_config::EnvKey`; never call `std::env::var` ad-hoc inside crates. |

### Type Safety

**No hardcoded strings for closed sets** (tool names, event types, config keys, protocol discriminators). Preference order:

1. **Enum + `.as_str()`** — e.g. `CommandBase::Read.as_str()`, `HookEventType::PreToolUse.as_str()`
2. **Module constants** (`pub const X: &str = "..."`) when the canonical enum lives in an inaccessible crate
3. **Typed struct** instead of `serde_json::Value` map

Raw strings only for unconstrained input (user text, opaque external IDs, third-party wire formats).

**Typed structs over `serde_json::Value`** when the payload is both produced *and* consumed inside coco-rs. Use `Option<T>` + `#[serde(default, skip_serializing_if = "Option::is_none")]` for optional fields, `#[serde(tag = "type")]` for variants.

*Exception:* `vercel-ai-*` provider-extension slots (`ProviderOptions`, `ProviderMetadata`, raw provider responses, model-specific blobs) keep `Value` — deliberate pass-through. Unpack to typed structs at the coco-rs boundary; never let `Value` leak inward.

### Multi-Provider Boundaries

- **Provider concerns stay in provider crates.** OAuth, API-key helpers, prompt-cache breakpoint detection, beta headers, 529-capacity retry, rate-limit messaging, Claude.ai/Anthropic policy limits live in `vercel-ai-<provider>` — **not** `services/inference`. `services/inference` owns only generic concerns. **Anthropic cloud-credential routes (Bedrock / Vertex / Foundry) are explicit non-goals** — coco-rs targets Anthropic FirstParty, OpenAI, Google Gemini, ByteDance, and generic OpenAI-compatible providers. `services/inference/src/auth.rs` keeps env-based detection only for diagnostic clarity; `model_factory.rs` does not and will not dispatch on these variants.

- **Models are `(provider, api, model_id)`, never a bare string.** Always go through `coco_config::ModelRoles::get(ModelRole::X)`. The canonical `ModelRole` owner is `coco_types::ModelRole` / `docs/internal/crate-coco-types.md`; current roles are `Main`, `Fast`, `Plan`, `Explore`, `Review`, `Subagent`, `Memory`, and `HookAgent`. `Subagent` is the default LLM role for generic/custom subagent execution; built-in subagent types may resolve to narrower roles such as `Explore`, `Plan`, or `Review`. There is no `Compact` model role in the current enum. Never add `title_model: String`; expose a `bool` flag and route via the appropriate role. Add a new `ModelRole` variant rather than a raw string.

- **Compaction — three generic strategies only:** micro-compact (clear old tool results), full LLM summarization, reactive (on `prompt_too_long`). `HISTORY_SNIP` and `CONTEXT_COLLAPSE` are not implemented — cache-aware optimizations of that kind belong in the `vercel-ai-*` provider crates.

- **Plan Mode — skip Ultraplan only.** Port core lifecycle, Pewter-ledger (Phase-4 variants `null`/`trim`/`cut`/`cap`), Interview phase — gate on `settings.json` (`plan_mode.phase4_variant`, `plan_mode.workflow`), not GrowthBook or `USER_TYPE=ant`. Skip every `feature('ULTRAPLAN')` path (needs CCR backend coco-rs doesn't ship).

### Config & Feature Gates

- **Consume `RuntimeConfig`, never raw `Settings`/env.** All layering (settings.json → `EnvOnlyConfig` → `RuntimeOverrides`) is folded once in `coco_config::build_runtime_config`. Leaf crates read the resolved sub-config (`tool`, `shell`, `sandbox`, `memory`, `mcp`, `compact`, `web_*`, `paths`, …) and `features` / `tool_overrides` off `RuntimeConfig` — they never re-merge `Partial*` overlays or call `std::env::var`.
- **Feature is a coarse capability gate, not a sub-toggle.** `coco_types::Feature` is a closed enum — see `common/types/src/features.rs` for the current variant set (don't enumerate it in docs; it grows). Sub-toggles (`MemoryConfig.extraction_enabled`, `SandboxConfig.mode`, `RetrievalConfig.reranker.enabled`, …) stay inside their `*Config`. Enterprise policy and "configured = enabled" subsystems (hooks, plugins, skills, telemetry) are **not** Features.
- **Three resolution layers, single merge site.** `Features::with_defaults()` → `apply_map(settings.features)` → `apply_map(env COCO_FEATURE_*)` → `RuntimeOverrides.feature_overrides`. Never bypass: no ad-hoc `COCO_DISABLE_*` env, no `Features::default()` (the type intentionally has no `Default` impl — pick `with_defaults()` or `empty()`).
- **Gate at the right layer.** Tool-level gate → implement `Tool::is_enabled(ctx) { ctx.features.enabled(Feature::X) }` (Layer 1 of the 5-layer filter pipeline). Subsystem-level gate (`AutoMemory`, `Retrieval`, `Sandbox`) → check at the subsystem entry point, not in tool registry. Subagents inherit parent `Arc<Features>` and **must never widen**.

### Event System

- **Single `CoreEvent` enum, three dispatch layers:** `Protocol` (SDK NDJSON), `Stream` (agent content), `Tui` (terminal). Emit once; consumers pick a layer. `QueryEngine::emit_*` is the reference emitter.
- **Opt-in lifecycle emitters.** Background subsystems (`TaskManager`, future retrieval) expose `with_event_sink(mpsc::Sender<CoreEvent>)` — zero overhead when not subscribed.
- **Isolated event streams stay isolated.** `RetrievalEvent` and `vercel-ai` callbacks (`OnStartEvent` etc.) are **not** bridged into `CoreEvent`. Need cross-subsystem progress? Add a single aggregate variant through an opt-in sink — don't bridge the full taxonomy.

## Specialized Documentation

Every workspace crate has its own `CLAUDE.md` (path = `coco-rs/<layer>/<crate>/CLAUDE.md`) — the per-layer crate tables above double as the index, so the lists are not repeated here.

- **Error codes**: [common/error/README.md](coco-rs/common/error/README.md)
- **Vercel AI TS-port lineage** (baseline commit, mirror scope, deviations): [coco-rs/vercel-ai/README.md](coco-rs/vercel-ai/README.md)
- **Event Hub design**: [docs/internal/event-hub/spec.md](docs/internal/event-hub/spec.md)
- **User docs**: [docs/](docs/) — getting-started, configuration, providers-and-auth,
  models-and-moa, cli-reference, slash-commands, tools, permissions, sandbox, mcp,
  memory, extending, subagents-and-teams, sdk, troubleshooting. Tables marked
  `<!-- BEGIN GENERATED: ... -->` are produced by `just docs-gen` — edit the
  generator in `coco-rs/xtask/`, not the table. `just check-docs` gates drift.
- **Internal design notes**: [docs/internal/](docs/internal/) — historical, may be stale;
  the code wins over anything in there
