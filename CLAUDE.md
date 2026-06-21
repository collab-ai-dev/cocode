# CLAUDE.md

Multi-provider LLM SDK and CLI. All development in `coco-rs/`.

> `cocode-rs/` and `codex-rs/` in this repo are reference implementations; active development is in `coco-rs/`.
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
just pre-commit   # REQUIRED before commit: quick-check + nextest test run
```

### Pre-commit is expensive — run it ONCE per commit, at the very end

`pre-commit` compiles every test binary in the workspace and runs the full
nextest suite. It is *orders of magnitude* slower than `quick-check`. Treat
it as the **final gate**, not an iteration tool.

**The rule:** every iteration uses `quick-check`; `pre-commit` fires exactly
once, immediately before `git commit`.

```
edit → just quick-check → repeat until green
                                ↓
                       just pre-commit (final gate, ONCE)
                                ↓
                            git commit
```

**Why never run `pre-commit` mid-iteration:** `cargo clippy` / `cargo check`
write `.rmeta` (metadata, no codegen). `cargo test --no-run` writes test-cfg
ELF binaries with dev-deps linked. **These two artifact caches do not share.**
If `pre-commit` is going to fail on a clippy warning, you only find out *after*
nextest has already spent its compile budget on output that gets thrown away.
`quick-check` catches the same warnings without entering the test-compile path.

**Linker speedup (already configured):** `coco-rs/.cargo/config.toml` sets
`mold` as the Linux linker (`-C link-arg=-fuse-ld=mold`). Install once with
`apt install mold`; the config picks it up automatically. Linking is a large
fraction of test-binary build time, so this materially shortens both warm
incremental rebuilds and cold `pre-commit` runs.

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
│  App:         cli, tui, session, query, state                        │
│  Root:        commands, skills, hooks, tasks, memory, plugins,       │
│               keybindings                                             │
│  Core:        tool-runtime → tools, permissions, messages, context, │
│               system-reminder, subagent                               │
│  Services:    inference, compact, mcp, rmcp-client, mcp-types, lsp   │
│  Exec:        shell, sandbox, process-hardening, exec-server,        │
│               apply-patch                                             │
│  Standalone:  bridge, retrieval                                      │
│  Vercel AI:   ai → openai, openai-compatible, google, anthropic,     │
│               bytedance (on provider + provider-utils)                │
│  Common:      types, llm-types, config, error, otel,                 │
│               stack-trace-macro, model-card                           │
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
~/.coco/{provider,model,config}.json + env + CLI overrides
  → Settings → EnvOnlyConfig → RuntimeOverrides → ResolvedConfig
  → GlobalConfig (hot-reload via SettingsWatcher)
  → ModelRoles + ModelAlias + FastModeState
  → BootstrapConfig → app/query → services/inference
```

### Provider Call Chain

```
QueryEngine → ApiClient [services/inference]
  → Arc<dyn LanguageModelV4> → vercel-ai (generate_text / stream_text)
  → provider impl [vercel-ai/{openai,anthropic,google,bytedance,openai-compatible}]
  → HTTP → typed stream → CacheBreakDetector + UsageAccumulator → QueryResult
```

For shell execution, MCP integration, background tasks: see the respective crate's CLAUDE.md (`exec/shell`, `services/mcp`, `tasks`).

## Crate Guide

One-line purposes. For key types and details, open each crate's own `CLAUDE.md`.

### Common

| Crate | Purpose |
|-------|---------|
| `types` | Foundation types incl. Message family + wire-tagged unions. Source-level vercel-ai-free (DTOs reach via `coco-llm-types`). |
| `llm-types` | DTO seam: pure re-export shim for `vercel-ai-provider` shapes (LlmMessage / content parts / StopReason / Usage / …). Paired with `services/inference` to form the dual-seam — SDK upgrade edits both crates. |
| `config` | Layered config: JSON + env + runtime overrides + hot reload |
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
| `vercel-ai-openai-compatible` | Generic OpenAI-compatible provider (xAI, Groq, Together) |
| `vercel-ai-google` | Google Gemini provider (API key → generativelanguage) |
| `vercel-ai-google-codeassist` | Gemini Code Assist subscription transport (OAuth → cloudcode-pa); reuses `vercel-ai-google`'s codec |
| `vercel-ai-anthropic` | Anthropic Claude provider |
| `vercel-ai-bytedance` | ByteDance Seedance video provider |

### Services

| Crate | Purpose |
|-------|---------|
| `inference` | Thin multi-provider wrapper: generic retry, usage aggregation, tool-schema, cache-break. Auth/caching/betas live in provider crates |
| `compact` | Context compaction: full / micro / reactive / session-memory + auto-trigger |
| `mcp` | MCP server lifecycle, OAuth (incl. xaa IDP), elicitation, channel permissions |
| `mcp-types` | Auto-generated MCP message types |
| `rmcp-client` | MCP client: stdio + HTTP/SSE transport, OAuth persistence |
| `lsp` | AI-friendly LSP (query by name+kind, not position); rust-analyzer/gopls/pyright/tsserver |

### Core

| Crate | Purpose |
|-------|---------|
| `tool-runtime` | `Tool` trait, streaming executor, registry, callback handles (interface layer) |
| `tools` | Built-in tool impls (File I/O, Web, Agent, Task, Plan, Shell, MCP mgmt, Scheduling) |
| `permissions` | Evaluator + 2-stage auto-mode/yolo XML-LLM classifier + bypass killswitch |
| `messages` | MessageHistory, normalization/filtering/predicates, cost tracking |
| `context` | System context assembly, CLAUDE.md discovery, attachments, plan-mode reminders |
| `system-reminder` | Dynamic `<system-reminder>` injection: trait-based generators + parallel orchestrator + throttle |
| `subagent` | Pure-logic subagent rules: definition catalog, source precedence, AgentTool prompt rendering, tool filter planning |

### Exec

| Crate | Purpose |
|-------|---------|
| `shell` | Shell execution with security analysis, destructive warnings, sandbox decisions |
| `sandbox` | Three modes: None/ReadOnly/Strict (disabled by default) |
| `process-hardening` | OS-level security (macOS PT_DENY_ATTACH, Linux prctl) |
| `exec-server` | Minimal `ExecutorFileSystem` trait ported from codex-rs |
| `apply-patch` | Unified diff/patch with fuzzy matching (ported from codex-rs) |

### Root Modules

| Crate | Purpose |
|-------|---------|
| `commands` | Slash command registry + built-in command impls (v1/v2/v3) |
| `skills` | Skill markdown workflows (bundled / project / user / plugin) |
| `hooks` | Pre/post event interception with scoped priority, async registry, SSRF guard |
| `tasks` | Three kinds: `running` (bg tasks), `task_list` (durable plan items), `todos` (per-agent) |
| `memory` | Persistent cross-session: CLAUDE.md mgmt, auto-extraction, session memory, KAIROS auto-dream, team sync |
| `plugins` | Plugin system via `PLUGIN.toml` (contributions, marketplace, hot-reload) |
| `keybindings` | Shortcuts with context-based resolution and chord support |
| `output-styles` | Built-in catalog (`Explanatory` / `Learning`) + project / user / managed dir loader + plugin loader (`force-for-plugin`) + system-prompt section + `OutputStyleManager` |

### App

| Crate | Purpose |
|-------|---------|
| `cli` | CLI entry (clap), transports (SSE / WS / NDJSON), server / daemon / SDK modes; binary `coco` |
| `tui` | Terminal UI shell: Elm architecture (TEA) + rust-i18n; owns `AppState`, the AppState→Line projection, and the domain renderers. Drives `coco-tui-ui` for the actual painting |
| `tui-ui` (top-level crate) | Pure, domain-free presentational layer: native-scrollback paint engine (`SurfaceTerminal`, BSU/ESU, cell-diff), generic widgets, theme/style/color, width-aware truncation. Depends only on ratatui/crossterm/unicode/tracing — no `AppState`, no i18n. Seam enforced by `scripts/check-tui-ui-seam.sh` |
| `session` | Session persistence, title generation, transcript recovery |
| `query` | Multi-turn agent loop driver (`QueryEngine`) + budget + command queue |
| `state` | Central `AppState` tree + swarm orchestration modules |

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

### Utils

Reusable primitives. **Check here first** before implementing any basic utility.

| Crate | Purpose |
|-------|---------|
| `absolute-path` | Absolute path types with deserialization support |
| `async-utils` | Async runtime utilities and cancellation helpers |
| `cache` | LRU cache with Tokio mutex protection |
| `cargo-bin` | Cargo binary helpers for test harnesses |
| `common` | Shared cross-crate utility functions |
| `cursor` | Text cursor with kill ring (Ctrl+K/Y), word boundaries, UTF-8 safe |
| `file-encoding` | File encoding and line-ending detection/preservation |
| `file-ignore` | .gitignore-aware file filtering (unified ignore service) |
| `file-search` | Fuzzy file search with nucleo and gitignore support |
| `file-watch` | Generic reusable file-watch infrastructure |
| `frontmatter` | YAML frontmatter parser for skills, commands, agents, memory files |
| `git` | Git operations wrapper |
| `image` | Image processing utilities |
| `json-to-toml` | JSON to TOML conversion |
| `keyring-store` | Secure credential storage using system keyring |
| `pty` | Pseudo-terminal handling |
| `readiness` | Readiness flag with token-based auth and async waiting |
| `rustls-provider` | TLS provider init via rustls crypto ring |
| `secret-redact` | Secret redaction (OpenAI, Anthropic, GitHub, Slack, AWS, bearer tokens) |
| `shell-parser` | Shell command parsing and security analysis |
| `sleep-inhibitor` | Cross-platform sleep prevention (macOS/Linux/Windows) |
| `stdio-to-uds` | Bridge stdio streams to Unix domain sockets |
| `stream-parser` | Stream parsing (text, citation, inline hidden tag, proposed plan, UTF-8) |
| `string` | String truncation and boundary utilities |
| `symbol-search` | Symbol search for code navigation |
| `test-harness` | Test harness utilities for integration tests |

## Key Design Patterns

| Pattern | Where | Details |
|---------|-------|---------|
| **Builder** | Most crates | `QueryEngine::new()`, `SearchRequest` (fluent), `RetrievalFacade` |
| **Arc-heavy sharing** | core/, root, app/ | `Arc<Mutex<T>>` / `Arc<RwLock<T>>` for registries/managers |
| **Event-driven** | query, tui, tasks | `mpsc::Sender<CoreEvent>` sinks; `with_event_sink()` opt-in emitters |
| **3-layer event dispatch** | types, query | `CoreEvent::Protocol`/`Stream`/`Tui` — emit once, consumers pick layer |
| **Cancellation** | All async | `CancellationToken` threaded through all layers |
| **Registry** | tool-runtime, commands, skills, plugins, mcp | `ToolRegistry`, `CommandRegistry`, `SkillManager`, `PluginLoader`, `DiscoveryCache` |
| **State Machine** | query, permissions, mcp | `ContinueReason`, `AutoModeState`, `RmcpClient` |
| **Callback decoupling** | core/tool-runtime | `AgentHandle`, `HookHandle`, `MailboxHandle`, `McpHandle`, `ToolPermissionBridge` — avoid tool→subsystem cycles; `NoOp*` test doubles |
| **Permission pipeline** | permissions, tool-runtime | `check_permission()` → auto-mode/yolo XML classifier → `DenialTracker` + killswitch |
| **Facade** | retrieval | Single `RetrievalFacade` hides search + index + repomap + reranker |
| **Elm (TEA)** | tui | Model (`AppState`) + Message (`TuiEvent`) + Update + View |
| **Middleware** | vercel-ai | `FnOnce` + `BoxFuture` callbacks for `do_generate` / `do_stream` |
| **Typed extension slots** | vercel-ai providers | `ProviderOptions` / `ProviderMetadata` = `serde_json::Value` on purpose |
| **Isolated event stream** | retrieval | `RetrievalEvent` intentionally not bridged into `CoreEvent` |

## Error Handling

Three tiers, each with one allowed error library. Pick by layer, not taste.

| Tier | Where | Library | Notes |
|------|-------|---------|-------|
| **3 (main trunk)** | `common/`, `core/`, `services/`, root modules (`commands`, `skills`, `hooks`, `tasks`, `memory`, `plugins`, `keybindings`), `app/query` | **snafu + `coco-error`** | Required when the error crosses ≥2 layers, drives retry / classification, or surfaces to users. Implement `ErrorExt` and pick a `StatusCode`. |
| **2 (boundary)** | `vercel-ai/*`, `utils/*` (libraries), `retrieval`, `bridge` | **thiserror** | Leaf libraries. No `coco-error` dep — main-trunk callers convert at the boundary via `boxed(err, StatusCode::X)`. |
| **1 (terminal)** | `app/cli` `main`, `app/tui`, `exec/shell`, `exec/exec-server`, tests, `[dev-dependencies]` | **anyhow** | Entry points and tests where errors are printed and discarded. Never appears in a public lib API. |

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

## TUI Conventions (ratatui)

- Stylize helpers: `"text".dim().bold().cyan()` — avoid manual `Style`
- Simple conversions: `"text".into()`, `vec![...].into()`
- Runtime-computed style: `Span::styled` or `Span::from(t).set_style(s)` is OK
- Avoid `.white()`; prefer default foreground
- Don't refactor between equivalent forms without a readability gain
- Prefer forms that stay on one line after rustfmt
- Text wrapping: `textwrap::wrap` with `initial_indent`/`subsequent_indent` — don't roll your own

## Async Conventions

- `tokio::task::spawn_blocking` for blocking ops
- Prefer `tokio::sync` primitives in async contexts
- `Send + Sync` bounds on traits used with `Arc<dyn Trait>`

## Tracing & Logging

The global `tracing` subscriber is installed once from the binary
(`app/cli/src/main.rs::main`) via `coco_otel::subscriber::init_subscriber`.
**Without that install every `tracing::*` call is a no-op** — library /
test code MUST NOT install one. Tests that need to assert on logs use
`coco_otel::subscriber::init_for_tests` (`OnceLock`-guarded).

Filter resolution: `--log-level` > `COCO_LOG` > `RUST_LOG` >
`subscriber::DEFAULT_FILTER` (`coco=debug,info`). File sink is rotating
daily under `<config_home>/logs/coco.log` in TUI / SDK / Headless modes.
Stdout is reserved (TUI ratatui screen, SDK NDJSON RPC) — logs land on
stderr only when explicitly opted in via `--log-stderr` (Headless does
this by default).

Levels, span-anchor list (the seven canonical anchors), `#[instrument]`
policy, standard field names, and the secret-redaction rule for HTTP
bodies — see `common/otel/CLAUDE.md` "Logging conventions". Adopt those
field names verbatim across crates so ops can pivot.

## Dependencies

| Purpose | Crate |
|---------|-------|
| Async runtime | `tokio` |
| HTTP | `reqwest` |
| JSON | `serde_json` |
| Errors | `anyhow`, `snafu`, `thiserror` |
| Logging | `tracing` |
| Testing | `pretty_assertions`, `insta`, `wiremock` |
| TUI | `ratatui`, `crossterm` |
| MCP | `rmcp` |

Prefer well-maintained crates; check security advisories; use workspace deps.

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

- **Models are `(provider, api, model_id)`, never a bare string.** Always go through `coco_config::ModelRoles::get(ModelRole::X)`. The canonical `ModelRole` owner is `coco_types::ModelRole` / `docs/coco-rs/crate-coco-types.md`; current roles are `Main`, `Fast`, `Plan`, `Explore`, `Review`, `Subagent`, `Memory`, and `HookAgent`. `Subagent` is the default LLM role for generic/custom subagent execution; built-in subagent types may resolve to narrower roles such as `Explore`, `Plan`, or `Review`. There is no `Compact` model role in the current enum. Never add `title_model: String`; expose a `bool` flag and route via the appropriate role. Add a new `ModelRole` variant rather than a raw string.

- **Compaction — three generic strategies only:** micro-compact (clear old tool results), full LLM summarization, reactive (on `prompt_too_long`). `HISTORY_SNIP` and `CONTEXT_COLLAPSE` are not implemented — cache-aware optimizations of that kind belong in the `vercel-ai-*` provider crates.

- **Plan Mode — skip Ultraplan only.** Port core lifecycle, Pewter-ledger (Phase-4 variants `null`/`trim`/`cut`/`cap`), Interview phase — gate on `settings.json` (`plan_mode.phase4_variant`, `plan_mode.workflow`), not GrowthBook or `USER_TYPE=ant`. Skip every `feature('ULTRAPLAN')` path (needs CCR backend coco-rs doesn't ship).

### Config & Feature Gates

- **Consume `RuntimeConfig`, never raw `Settings`/env.** All layering (settings.json → `EnvOnlyConfig` → `RuntimeOverrides`) is folded once in `coco_config::build_runtime_config`. Leaf crates read the resolved sub-config (`tool`, `shell`, `sandbox`, `memory`, `mcp`, `compact`, `web_*`, `paths`, …) and `features` / `tool_overrides` off `RuntimeConfig` — they never re-merge `Partial*` overlays or call `std::env::var`.
- **Feature is a coarse capability gate, not a sub-toggle.** `coco_types::Feature` is closed (`WebSearch`, `WebFetch`, `Sandbox`, `AutoMemory`, `Retrieval`, `AgentTeams`, `Worktree`, `Lsp`). Sub-toggles (`MemoryConfig.extraction_enabled`, `SandboxConfig.mode`, `RetrievalConfig.reranker.enabled`, …) stay inside their `*Config`. Enterprise policy and "configured = enabled" subsystems (hooks, plugins, skills, telemetry) are **not** Features.
- **Three resolution layers, single merge site.** `Features::with_defaults()` → `apply_map(settings.features)` → `apply_map(env COCO_FEATURE_*)` → `RuntimeOverrides.feature_overrides`. Never bypass: no ad-hoc `COCO_DISABLE_*` env, no `Features::default()` (the type intentionally has no `Default` impl — pick `with_defaults()` or `empty()`).
- **Gate at the right layer.** Tool-level gate → implement `Tool::is_enabled(ctx) { ctx.features.enabled(Feature::X) }` (Layer 1 of the 5-layer filter pipeline). Subsystem-level gate (`AutoMemory`, `Retrieval`, `Sandbox`) → check at the subsystem entry point, not in tool registry. Subagents inherit parent `Arc<Features>` and **must never widen**.

### Event System

- **Single `CoreEvent` enum, three dispatch layers:** `Protocol` (SDK NDJSON), `Stream` (agent content), `Tui` (terminal). Emit once; consumers pick a layer. `QueryEngine::emit_*` is the reference emitter.
- **Opt-in lifecycle emitters.** Background subsystems (`TaskManager`, future retrieval) expose `with_event_sink(mpsc::Sender<CoreEvent>)` — zero overhead when not subscribed.
- **Isolated event streams stay isolated.** `RetrievalEvent` and `vercel-ai` callbacks (`OnStartEvent` etc.) are **not** bridged into `CoreEvent`. Need cross-subsystem progress? Add a single aggregate variant through an opt-in sink — don't bridge the full taxonomy.

## Specialized Documentation

Every crate in `coco-rs/` has its own `CLAUDE.md` (path = `coco-rs/<layer>/<crate>/CLAUDE.md`).

- **Common**: types, llm-types, config, error ([codes](coco-rs/common/error/README.md)), otel, stack-trace-macro
- **Vercel AI**: ai, provider, provider-utils, openai, openai-compatible, google, anthropic, bytedance
- **Services**: inference, compact, mcp, lsp
- **Core**: tool-runtime, tools, permissions, messages, context, system-reminder, subagent
- **Exec**: shell, sandbox, process-hardening, exec-server, apply-patch
- **Root**: commands, skills, hooks, tasks, memory, plugins, keybindings, output-styles
- **App**: cli, tui, query, state, session
- **Hub**: protocol, connector, server. Design: [docs/coco-rs/event-hub/spec.md](docs/coco-rs/event-hub/spec.md)
- **Standalone**: bridge, retrieval
- **Utils**: each of the 26 utils/ crates has one
- **User docs**: [docs/](docs/) — getting-started.md, config.md, sandbox.md
