# Provider Prompt Role Architecture

> Status: Target design
> Scope: `coco-rs/core/context/`, `coco-rs/app/query/`, `coco-rs/services/inference/`, `coco-rs/vercel-ai/provider/`, `coco-rs/vercel-ai/{openai,anthropic,google,openai-compatible}/`
> Sources: `codex-rs/codex-api`, `codex-rs/core`, TS source, `gemini-cli/packages/core/src`
> Owners: coco-context + coco-query + coco-inference + vercel-ai provider crates

> **Source of truth.** This document owns the cross-provider prompt role
> architecture. Provider-specific wire serialization remains owned by each
> `vercel-ai-*` crate. Model and provider configuration remain owned by
> `crate-coco-config.md` and `multi-provider-plan.md`.
>
> **Design stance.** This is a clean target architecture. The implementation
> should replace monolithic system-prompt assembly with typed prompt semantics
> and provider layout adapters in one coherent change.

## 0. TL;DR — Final Design

**Layout payload travels via `provider_options["prompt_layout"]` namespace.**

```text
app/query: build PromptEnvelope (sections + history)
        ↓
services/inference (layout adapter):
  - route sections by (kind, provider family) per §4 routing table
  - produce typed PromptLayoutOptions {
      instructions, system_blocks, system_instruction,
      layout_warnings, prompt_hash_inputs
    }
        ↓
inference::put_layout_options(&mut call.provider_options, &layout)
  serde_json → provider_options["prompt_layout"] inner map
  (write order: build_call_options first sets ["openai"]/["anthropic"];
   layout adapter then non-destructively sets "prompt_layout")
        ↓
JSON across crate boundary
        ↓
provider crate (vercel-ai-{openai,anthropic,google}):
  local PromptLayoutWire mirror struct via serde_json::from_value
  → write own field to wire body
        ↓
warnings bubble:
  do_generate → LanguageModelV4GenerateResult::warnings
  do_stream   → LanguageModelV4StreamPart::StreamStart { warnings }
        ↓
cache detector: prompt_hash_inputs from "prompt_layout";
  extra_body_hash from other namespaces (excluding "prompt_layout")
  → merged into PromptStateInput
```

**Why this shape (vs alternatives considered):**

| Decision | Rejected alternative | Reason |
|---|---|---|
| Payload in `coco-inference` | Add fields to `vercel-ai-provider::LanguageModelV4CallOptions` | Breaks SDK fidelity contract (`vercel-ai/provider/CLAUDE.md:1-3`: zero coco deps). |
| JSON via `provider_options` namespace | Provider crates import `PromptLayoutOptions` typed | `vercel-ai-*` crates do not depend on `coco-inference`; importing inverts the dep graph. |
| Dedicated `"prompt_layout"` namespace | Per-provider namespaces (ThinkingLevel pattern) | Cache-hash exclusion at namespace granularity; `prompt_hash_inputs` and `layout_warnings` are provider-agnostic and have no natural per-provider home; physical separation between layout-supplied vs user-supplied (e.g., OpenAI `instructions`). |
| `(kind, family)` two-dimensional routing | Fixed kind→slot table | `Environment` is a system block in Anthropic but contextual user input in OpenAI/Gemini per references; no single slot satisfies all three. |
| `CacheHint` independent of routing | `CacheHint::Stable` promotes Memory to system tier | Authority and cache are orthogonal axes; conflating them is a backdoor for user-context to elevate to system. |
| `build_call_options` before layout | Layout first | `build_call_options.rs:148` constructs fresh `ProviderOptions`; reverse order would clobber `"prompt_layout"`. |
| Reminders in `PromptEnvelope.history` | Reminders as envelope sections | Per-turn dynamic + persists across turns; TS emits them as user-role messages with `<system-reminder>` xml. |

**Pattern parity with `ThinkingLevel`:** typed in coco-inference, JSON across
crate boundary. Difference: ThinkingLevel writes to per-API namespace; layout
writes to dedicated namespace because cache-hash semantics need
namespace-level exclusion that per-key exclusion cannot provide cleanly.

**Implementation cost:** ~15 lines `*Wire` mirror struct per provider crate +
one `serde_json::from_value` call. Roundtrip test (§14.8) locks the wire
format. JSON serde overhead is microseconds, irrelevant against LLM RTT.

## 1. Goals

The prompt architecture has four goals, in priority order:

1. **Provider-correct wire shape** - OpenAI Responses, Anthropic Messages, and
   Gemini GenerateContent must receive prompts in the shape their authoritative
   clients use.
2. **Explicit semantic roles** - Coco must model `System`, `Developer`, user
   context, and conversation history as separate concepts instead of flattening
   everything into one string.
3. **Rust-native boundaries** - App crates assemble semantic prompt data;
   inference chooses a provider layout; provider crates serialize their own wire
   requests.
4. **Stable cache inputs** - Prompt cache and cache-break detection must hash the
   same semantic inputs that affect the provider request.

This design does not add a generic raw body escape hatch to query code. Raw
provider JSON remains a provider/config boundary concern.

## 2. Behavioral Sources of Truth

### 2.1 OpenAI Responses

`codex-rs/codex-api` and `codex-rs/core` are authoritative for OpenAI Responses
prompt semantics.

The required shape is:

- `ResponsesApiRequest.instructions` contains model/base instructions.
- `input[]` contains developer messages for application and runtime policy.
- `input[]` contains contextual user messages for project instructions,
  environment, and other user-context sections.
- Conversation history follows the contextual prefix.
- Base instructions are not duplicated into `input[]`.

This mirrors Codex's split between `base_instructions`, developer context, and
contextual user messages.

### 2.2 Anthropic Messages

The TS source is authoritative for Anthropic Messages prompt and cache
behavior. TS file paths are relative to the TS project's `src/` directory.

The required shape is:

- Anthropic receives top-level `system: TextBlock[]`.
- System text remains block-structured, not flattened into one string.
- Static and dynamic system sections keep stable ordering.
- Cache hints map to Anthropic `cache_control` metadata on eligible blocks.
- **Stable** runtime context (project instructions, environment, attribution
  header, fingerprint) is encoded as system blocks. The TS source
  (`utils/sideQuery.ts:142-167`) prepends attribution to `system[]`, not to
  user messages.
- **Per-turn dynamic** injections (system reminders, tool-injected hints,
  side-questions) appear as `user` messages with `<system-reminder>` xml
  framing — recognized by the model via system-prompt convention. This is
  the only category that becomes a meta user message before/within history.
- Prompt cache-break detection includes system block text, cache metadata, tool
  schemas, model, and request-shaping provider options.

Coco should mirror the Anthropic prompt/cache behavior, while not porting
GrowthBook, OAuth, Claude.ai quota handling, policy limits, or internal
first-party operational headers.

### 2.3 Gemini GenerateContent

`gemini-cli/packages/core/src` is authoritative for Gemini
GenerateContent prompt and request-shaping behavior.

The required shape is:

- Gemini receives root `contents[]` plus `GenerateContentConfig`.
- `GenerateContentConfig.systemInstruction` contains the core prompt shell:
  identity, base instructions, developer policy, tool policy, workflow,
  sandbox, repository guidance, and stable memory selected for the system tier.
- `contents[]` uses only Gemini roles: `user` and `model`.
- Session and environment context are prepended as a user content item before
  conversational history.
- Loaded or just-in-time context may be prepended as user context, not as a
  developer or hidden message.
- Function responses are user-role parts. The Gemini API **server-side**
  rejects requests where a function response does not immediately follow the
  matching function call. gemini-cli does not validate this client-side
  (`packages/core/src/core/geminiChat.ts:156-167` only checks role ∈
  {user, model}); it relies on orchestration to produce correct order. Coco
  must be **stricter than the reference** and validate adjacency at emit
  time, because the failure mode is a server 400 with no clean recovery
  mid-turn.
- Tool declarations live in `GenerateContentConfig.tools`, with tool execution
  policy in `toolConfig` when needed.
- Thinking, safety, response modality, cached content, labels, and retrieval
  settings are request options, not prompt text.
- Model-family behavior is feature-driven. Gemini 3-style models may use modern
  prompt sections, `thinkingLevel`, and multimodal function-response parts;
  Gemini 2.5-style models may use budget-based thinking config.

Coco should mirror the Gemini prompt/request behavior, while not porting
Google-internal experiments, auth flows, or product-specific service plumbing.

## 3. Target Architecture

Prompt assembly is split into semantic construction and provider layout:

```text
                  QueryEngine turn setup
                           |
                           v
              +--------------------------+
              |      PromptEnvelope      |
              | semantic prompt sections |
              +------------+-------------+
                           |
                           v
              +--------------------------+
              | services/inference       |
              | PromptLayoutAdapter      |
              | (selects PromptWireFamily|
              |  from fingerprint)       |
              +------------+-------------+
                           |
        +------------------+------------------+------------------+
        |                  |                  |                  |
        v                  v                  v                  v
+---------------+  +----------------+  +---------------------+  +-------------+
| OpenAI        |  | Anthropic      |  | Gemini              |  | Chat-like   |
| Responses     |  | Messages       |  | GenerateContent     |  | fallback    |
| instructions  |  | system[]       |  | systemInstruction   |  | system/dev  |
| + developer   |  | + cache hints  |  | + contents[]        |  | folding     |
+-------+-------+  +-------+--------+  +----------+----------+  +------+------+
        |                  |                   |                  |
        v                  v                   v                  v
  vercel-ai-openai  vercel-ai-anthropic  vercel-ai-google  compatible providers
```

The semantic layer never writes provider wire JSON. **Layout selection lives in
`services/inference`, not `app/query`.** `ProviderClientFingerprint` (provider,
api, wire_api, base_url) — the only signal that determines which adapter runs
— is owned by `services/inference` (`fingerprint.rs`). `app/query` constructs
only the provider-neutral `PromptEnvelope`; `services/inference` runs the
matching layout adapter at call time. On provider fallback, `services/inference`
re-runs layout selection against the fallback fingerprint with the unchanged
envelope — `app/query` is not involved in re-layout.

The layout adapter populates a `PromptLayoutOptions` payload (owned by
`coco-inference`) and writes it into the existing
`LanguageModelV4CallOptions::provider_options` map under a reserved
`"prompt_layout"` namespace. The payload carries both the
provider-native prompt slots (`instructions`, `system_blocks`,
`system_instruction`) and metadata (`layout_warnings`,
`prompt_hash_inputs`); the prompt stream itself stays in
`LanguageModelV4CallOptions::prompt` as today. The matching
`vercel-ai-*` crate reads the namespace payload and writes each
native slot verbatim into the provider-shaped top-level field
(`request.instructions`, `request.system[]`,
`request.systemInstruction`). This makes the slot a **typed contract**
between the inference adapter and the provider crate without
extending the SDK-fidelity types in `vercel-ai-provider`.

### 3.1 Provider Abstraction Review

The abstraction is intentionally semantic, not a least-common-denominator chat
format. Coco must preserve why a prompt fragment exists before deciding where a
provider can place it.

The portable concepts are:

| Concept | Coco abstraction | Why it is portable |
|---|---|---|
| Base identity and model instructions | `PromptSectionKind::{Identity, ModelBaseInstructions}` (system-tier authority) | Every provider has a privileged or system-compatible instruction channel, even if the exact wire field differs. |
| Application and runtime policy | `PromptSectionKind::{DeveloperPolicy, ToolPolicy}` (developer-tier authority) | OpenAI has a native developer role; Anthropic and Gemini do not, but the authority level still matters for ordering, hashing, tests, and future providers. |
| Project, environment, session, and loaded context | `PromptSectionKind::{ProjectInstructions, Environment, Memory, LoadedContext, IdeContext, HookContext, ActiveTopic, UserContext}` (user-tier or system-tier per provider, see §4 routing) | These are contextual facts presented to the model before conversation history; provider-specific routing decides whether each becomes a system block (Anthropic) or a contextual user item (OpenAI/Gemini). |
| Conversation history | normalized `LanguageModelV4Message` list (`PromptEnvelope.history`) | History roles differ by provider, but user, assistant/model, and tool-result semantics are portable. |
| Tool schema and tool execution policy | typed tool definitions plus request options | Providers serialize tools differently, but tools are not prompt text and must not be embedded into instructions. |
| Request shaping | typed call/model options | Thinking, safety, response format, cache handles, and modal settings change provider behavior without becoming prompt sections. |
| Cache semantics | `CacheHint` plus prompt-state hash inputs | Anthropic exposes block cache metadata; other providers still need stable cache-break detection over provider-visible prompt inputs. |

The non-portable concepts are intentionally not modeled as generic prompt text:

- OpenAI Responses top-level `instructions`.
- Anthropic top-level `system[]` text blocks and `cache_control` metadata.
- Gemini root `systemInstruction`, `contents[]`, `toolConfig`,
  `cachedContent`, and `generationConfig.thinkingConfig`.
- Provider auth, beta headers, quota policy, OAuth, retry status handling, and
  raw body escape hatches.

These are layout or provider-crate responsibilities. `app/query` should never
choose a provider wire field directly.

The boundary is:

```text
semantic prompt roles  (app/query: PromptEnvelope only)
        |
        v
services/inference: PromptLayoutAdapter
   selected from ProviderClientFingerprint + ModelInfo
        |
        +--> populates LanguageModelV4CallOptions native slots:
        |        instructions, system_blocks, system_instruction,
        |        plus prompt stream, plus warnings
        |
        v
provider crate wire serialization
   - reads native slot it understands (others = None)
   - serializes prompt stream to messages / input[] / contents[]
   - writes the slot verbatim to top-level instructions / system[] /
     systemInstruction (no rederivation, no string matching)
```

**Crate-layering note.** The native slots are a Coco-internal extension,
not part of the `@ai-sdk/provider` v4 type surface. They are **NOT**
added as fields on `vercel_ai_provider::LanguageModelV4CallOptions` —
that crate is contractually "Standalone type definitions matching
`@ai-sdk/provider` v4. Zero dependencies on other coco crates"
(`vercel-ai/provider/CLAUDE.md:1-3`); polluting it with Coco-specific
extension fields would break the SDK-fidelity contract.

Instead, native slots ride on the existing `ProviderOptions` extension
mechanism that vercel-ai-provider already exposes for cross-layer
extras (`ProviderOptions(HashMap<String, HashMap<String, JSONValue>>)`,
`vercel-ai/provider/src/shared/v4/provider_options.rs:14-18`, with
`get`/`set` over a JSON inner map — no Coco-specific accessor is
added to the SDK crate).

The inference adapter writes the layout payload into a reserved
`"prompt_layout"` namespace using helpers owned by `coco-inference`.
The namespace name is **functional, not project-branded**: it
identifies the data as prompt-shape outputs of the layout adapter
(distinct from request-shaping `extra_body` keys living under
`"openai"` / `"anthropic"` / `"google"`). No `coco_*` prefix — the
namespace map is the contract surface, and the contract is "this
key holds layout-adapter prompt slots", not "this key holds
project-Coco extras":

```rust
// In coco-inference (NEW types, owned here, not in vercel-ai-provider):
pub struct PromptLayoutOptions {
    /// OpenAI Responses top-level `instructions`.
    pub instructions: Option<String>,
    /// Anthropic top-level `system[]` blocks with cache_control
    /// pre-attached.
    pub system_blocks: Option<Vec<AnthropicSystemBlock>>,
    /// Gemini `GenerateContentConfig.systemInstruction`.
    pub system_instruction: Option<String>,
    /// Layout warnings raised during text-only collapse (see §5).
    pub layout_warnings: Vec<Warning>,
    /// Prompt-content-derived hash inputs (see §11).
    pub prompt_hash_inputs: Option<PromptHashInputs>,
}

pub struct AnthropicSystemBlock {
    pub text: String,
    pub cache_control: Option<AnthropicCacheControl>,
}

// Inference-only helpers (in coco-inference). Provider crates DO NOT
// import these — they parse the wire JSON directly (see below).
pub fn put_layout_options(
    opts: &mut vercel_ai_provider::ProviderOptions,
    layout: &PromptLayoutOptions,
);
pub fn take_layout_options(
    opts: &vercel_ai_provider::ProviderOptions,
) -> Option<PromptLayoutOptions>;
```

**Provider crates do NOT depend on `coco-inference` and do NOT import
`PromptLayoutOptions`.** They parse the JSON shape stored under the
`"prompt_layout"` namespace using a local serde mirror struct kept in
sync with the wire format:

```rust
// In vercel-ai-openai (deps unchanged: only vercel-ai-provider):
#[derive(serde::Deserialize)]
struct PromptLayoutWire {
    #[serde(default)] instructions: Option<String>,
    #[serde(default)] system_blocks: Option<Vec<AnthropicSystemBlockWire>>,
    #[serde(default)] system_instruction: Option<String>,
    #[serde(default)] layout_warnings: Vec<WarningWire>,
    #[serde(default)] prompt_hash_inputs: Option<PromptHashInputsWire>,
}

let layout: Option<PromptLayoutWire> = options
    .provider_options
    .as_ref()
    .and_then(|po| po.get("prompt_layout"))
    .and_then(|inner_map| {
        // ProviderOptions inner is `HashMap<String, JSONValue>`; the
        // namespace stores fields directly. Reconstruct the wire
        // struct via serde_json::Value.
        serde_json::to_value(inner_map).ok()
            .and_then(|v| serde_json::from_value(v).ok())
    });
```

The wire format (the JSON shape under `"prompt_layout"`) is the
cross-layer contract. The `PromptLayoutOptions` Rust struct in
`coco-inference` and the per-provider `*Wire` mirror structs MUST
serialize/deserialize identically; tests in §14.8 enforce this.

**Storage shape inside `ProviderOptions`.** `ProviderOptions` is
`HashMap<String, HashMap<String, JSONValue>>`
(`vercel-ai/provider/src/shared/v4/provider_options.rs:14-15`); the
outer key is the namespace, the inner map is field→JSON value.
`PromptLayoutOptions` fields are stored as **separate inner keys** (one
per field), not as a single serialized blob:

```text
provider_options
├─ "openai" → { "thinking": {...}, "extra_body_*": ... }   // build_call_options
├─ "anthropic" → { "contextManagement": {...}, ... }       // build_call_options
└─ "prompt_layout" → {                                        // layout adapter
     "instructions": <Value>,           // String → JSONValue::String
     "system_blocks": <Value>,          // Vec<AnthropicSystemBlock> → array
     "system_instruction": <Value>,
     "layout_warnings": <Value>,
     "prompt_hash_inputs": <Value>
   }
```

This matches how `extra_body` stores keys per-field under the canonical
provider namespace today (`build_call_options.rs:111-148`). The
`put_layout_options` helper serializes each `PromptLayoutOptions` field
into a `JSONValue` and inserts into the inner map; absent fields
(layout family doesn't apply, e.g. `system_blocks` on OpenAI) are
omitted, not stored as `JSONValue::Null`.

There is no `RequestPromptOptions` enum, no Coco-shaped extension
fields on AI SDK v4 types, and no Coco-typed accessor on
`ProviderOptions`. Native prompt slots travel as JSON under the
existing namespace map.

This design supersedes both the earlier "derive at wire time" sketch
and the second-round "extend `LanguageModelV4CallOptions`" sketch.
Today's OpenAI crate writes top-level `instructions` from
`openai_options.instructions`
(`vercel-ai/openai/src/responses/openai_responses_language_model.rs:300`).
After this design, the OpenAI crate first reads the `"prompt_layout"`
namespace; if the parsed `instructions` is `Some`, that wins; otherwise
it falls back to `openai_options.instructions` for externally supplied
instructions. **If both are present, the layout slot wins and the
provider crate emits a `Warning::Other` documenting the override.**
Tests assert this precedence (§14.7).

Provider capabilities are degradation rules, not semantic rules. For example:

- If OpenAI Responses supports `developer`, kinds with developer-tier
  authority (`DeveloperPolicy`, `ToolPolicy`) route to native developer
  messages.
- If Anthropic lacks a developer role, those kinds route to ordered
  top-level system blocks following the system-tier kinds.
- If Gemini lacks a developer role, those kinds route into
  `systemInstruction` after the system-tier text.
- If a chat-compatible provider lacks both native developer and system
  instruction support, the layout adapter folds the sections into the closest
  provider-supported privileged channel and records that behavior in tests.

This keeps Coco's internal contract stable while allowing each provider to
mirror its authoritative client.

## 4. PromptEnvelope

`PromptEnvelope` is the provider-neutral input to layout adapters. It is a
**single ordered list of sections** plus the normalized conversation
history. There are no per-slot Vecs because cross-slot ordering would be
unrecoverable once Anthropic / Gemini merge system + developer into a
single wire stream:

```rust
pub struct PromptEnvelope {
    /// Single authoring-order list of all prompt-shell sections.
    /// Layout adapters route per (kind, provider family); they MUST
    /// preserve the relative order within any provider's destination
    /// slot.
    pub sections: Vec<PromptSection>,
    /// Already-normalized full conversation history (user / assistant
    /// / tool) including any meta-user reminders that the
    /// system-reminder pipeline injected. Layout adapters append this
    /// after the per-provider context prefix; they do not re-normalize.
    pub history: Vec<LanguageModelV4Message>,
}

pub struct PromptSection {
    pub kind: PromptSectionKind,
    pub content: Vec<PromptPart>,
    pub cache: CacheHint,
    pub source: PromptSource,
}

// Re-export via the coco-inference seam (per `services/inference/src/lib.rs:5-9`,
// upper layers reach AI SDK types via `coco_inference::*`, not via
// `vercel_ai_provider::*` directly). `coco_inference::UserContentPart`
// is the seam re-export at `services/inference/src/lib.rs:97` (today
// resolves to `vercel_ai_provider::UserContentPart`). Carries Text and
// File (images, PDFs, etc.).
pub type PromptPart = coco_inference::UserContentPart;
```

Section kinds are a closed enum:

```rust
pub enum PromptSectionKind {
    Identity,
    ModelBaseInstructions,
    DeveloperPolicy,
    ToolPolicy,
    ProjectInstructions,
    Environment,
    Memory,
    LoadedContext,
    SkillListing,
    McpInstructions,
    IdeContext,
    HookContext,
    ActiveTopic,
    UserContext,
}
```

**Kind → provider slot routing is two-dimensional, not global.** A single
kind can route differently per provider, because the references disagree:
`Environment` is a system block in Anthropic
(`utils/sideQuery.ts:142-167`) but a user-role
`contents[]` prepend in Gemini
(`gemini-cli/packages/core/src/utils/environmentContext.ts:96-100`,
`getInitialChatHistory`) and a contextual user `input[]` item in OpenAI
Responses (§2.1: "input[] contains contextual user messages for project
instructions, environment, and other user-context sections"). A global
fixed slot table cannot satisfy all three.

Authoring rule: place sections in `PromptEnvelope.sections` in the
intended wire order. Layout adapters route by `(kind, family)` per the
table below and MUST preserve the relative order within each provider's
destination slot.

| Kind | Anthropic Messages | OpenAI Responses | Gemini GenerateContent |
|---|---|---|---|
| `Identity` | `system[]` block | `instructions` | `systemInstruction` |
| `ModelBaseInstructions` | `system[]` block | `instructions` | `systemInstruction` |
| `DeveloperPolicy` | `system[]` block (degraded) | `input[].role: developer` | `systemInstruction` |
| `ToolPolicy` | `system[]` block (degraded) | `input[].role: developer` | `systemInstruction` |
| `SkillListing` | `system[]` block | `input[].role: developer` | `systemInstruction` |
| `McpInstructions` | `system[]` block | `input[].role: developer` | `systemInstruction` |
| `ProjectInstructions` | `system[]` block | `input[].role: user` (contextual) | `contents[].role: user` (prepended) |
| `Environment` | `system[]` block | `input[].role: user` (contextual) | `contents[].role: user` (prepended) |
| `Memory` | `system[]` block | `input[].role: user` (contextual) | `contents[].role: user` (prepended) |
| `LoadedContext` | meta user before history | `input[].role: user` | `contents[].role: user` |
| `IdeContext` | meta user before history | `input[].role: user` | `contents[].role: user` |
| `HookContext` | meta user before history | `input[].role: user` | `contents[].role: user` |
| `ActiveTopic` | meta user before history | `input[].role: user` | `contents[].role: user` |
| `UserContext` | meta user before history | `input[].role: user` | `contents[].role: user` |

`SkillListing` and `McpInstructions` route to `developer` on OpenAI
Responses (matching codex `apps_instructions` / personality push to
`developer_sections` in `codex-rs/core/src/session/mod.rs:2627-2640`
and the `build_developer_update_item` shape in
`codex-rs/core/src/context_manager/updates.rs:178`). They are application
policy, not baked base instructions, so `instructions` is reserved for
`Identity` + `ModelBaseInstructions` only on OpenAI Responses.

The chat-like fallback collapses Anthropic columns: developer kinds fold
to system tier when the provider lacks a developer role, and contextual
kinds become user messages.

**Routing is determined by `(kind, family)` only — never by `CacheHint`.**
The "stable vs dynamic" axis controls cache metadata and hash inputs;
it does NOT promote a kind into a higher authority tier. `Memory` is a
user-tier contextual section on OpenAI and Gemini regardless of whether
its `CacheHint` is `Stable` or default. A `Stable` memory section is
still a contextual user item on OpenAI; the `Stable` hint only affects
how the cache detector treats its hash and (on Anthropic) whether
`cache_control` is attached to the system block. Conflating cache and
authority would make `CacheHint::Stable` a backdoor to elevate
user-context into system-tier prompt content, which is wrong.

Per-turn `<system-reminder>` injections are not envelope sections.
Reminders enter `MessageHistory` via the existing `coco-system-reminder`
pipeline (`core/system-reminder/src/inject.rs:218-228`) as
`Message::Attachment(AttachmentMessage::api(kind, llm))` and reach the
layout adapter as ordinary user-role messages in `PromptEnvelope.history`.
The xml framing is recognized by the model; the system-prompt shell
teaches the convention. Layout adapters need no special handling for
reminders.

**Internal vs wire representation.** `UserMessage` no longer carries an
`is_meta` field (post-Phase-2; see `core/messages/src/predicates.rs:36`).
The "is_meta" semantic now lives on the surrounding wrapper:
`SystemReminder { is_meta }` in the reminder generator config
(`core/system-reminder/src/types.rs:610`), and on the `AttachmentKind`
variant chosen by the inject pipeline. `is_meta_message(&Message)`
inspects the wrapper, not a field on the inner LLM message
(`core/messages/src/predicates.rs:40`). When the layout adapter walks
`PromptEnvelope.history`, every reminder is already a user-role
`LlmMessage` ready for the wire — the meta flag is for UI/transcript
filtering, not for wire shaping.

`MessageHistory` also carries a separate `Message::System` variant
(`core/messages/src/types/message.rs:404-414` —
`SystemMessage::{Informational, ApiError, CompactBoundary, ...}`) used for
session-internal notifications (compact boundaries, error displays, local
command echoes). These are filtered out during normalization
(`core/messages/src/normalize.rs:345-360`, `extract_llm_message` returns
`None` for `Message::System`) and never reach `PromptEnvelope.history`.
The two paths never overlap on the wire.

This split reflects the envelope's purpose: a static, provider-neutral prompt
shell built once per turn. Reminders are dynamic per-turn injections that
also persist in history across turns, which would force the envelope to encode
"this turn's new reminders" vs "historical reminders already in history" — a
stateful slice of history rather than a prompt-shell description. Anthropic
and Gemini also lack a `developer` role, so routing reminders through `user`
is the only cross-provider consistent choice.

Prompt source and cache behavior are also typed:

```rust
pub enum PromptSource {
    Builtin,
    ModelConfig,
    UserConfig,
    ProjectFile,
    Runtime,
    Tooling,
}

pub enum CacheHint {
    None,
    Stable,
    Ephemeral,
    Breakpoint,
}
```

`PromptEnvelope` preserves section order. It does not own model selection, tool
schemas, retry policy, or provider authentication.

**Section ordering invariant.** Layout adapters MUST preserve the
envelope-authored relative order **within each emitted destination
slot**. Adapters MAY split the single `sections` list across multiple
destination slots per the §4 routing table — that is routing, not
reordering. They MUST NOT shuffle sections that share a destination slot
(e.g. two `system[]` blocks on Anthropic stay in their authored
relative order). If a different relative order is needed for cache
stability, re-section before envelope build — do not "optimize" inside
the adapter.

**`PromptSource` for builtin per-model base instructions.** Today's
`ModelInfo.base_instructions` (`common/config/src/model/instructions.rs`,
embedded via `include_str!`) is `PromptSource::ModelConfig`, not `Builtin`.
The `include_str!` is just the default-value source; the field remains
overridable via `ProviderModelOverride`, so its authoritative origin is
model config.

**Migration of today's `SystemPromptBlock::CacheBreakpoint`.**
`core/context/src/prompt.rs` today inserts a positional `CacheBreakpoint`
between text blocks. In the new envelope, this maps to `CacheHint::Breakpoint`
on the **previous** section — i.e. "cut the cache after this section." The
adapter translates this hint into Anthropic `cache_control` placement on the
corresponding emitted block.

## 5. First-Class Developer Role

`LanguageModelV4Message` must include `Developer` as a first-class role:

```rust
pub enum LanguageModelV4Message {
    System {
        content: Vec<UserContentPart>,
        provider_options: Option<ProviderOptions>,
    },
    Developer {
        content: Vec<UserContentPart>,
        provider_options: Option<ProviderOptions>,
    },
    User {
        content: Vec<UserContentPart>,
        provider_options: Option<ProviderOptions>,
    },
    Assistant {
        content: Vec<AssistantContentPart>,
        provider_options: Option<ProviderOptions>,
    },
    Tool {
        content: Vec<ToolContentPart>,
        provider_options: Option<ProviderOptions>,
    },
}
```

`System` changes from today's `content: String` to `Vec<UserContentPart>` so
all five roles share the same multimodal shape. Two collapse paths exist
and BOTH MUST emit a `Warning::Unsupported` for any non-text part —
silent drop is forbidden:

- **Layout-driven path (envelope sections).** The layout adapter in
  `services/inference` flattens `PromptSection.content` into the
  text-only system / developer / `instructions` / `system_blocks` /
  `systemInstruction` slots. Warnings go into
  `PromptLayoutOptions::layout_warnings` and bubble through both
  request-shape paths (per §6): `LanguageModelV4GenerateResult::warnings`
  for `do_generate` (`generate_result.rs:21`); the first
  `LanguageModelV4StreamPart::StreamStart { warnings }` part for
  `do_stream` (`stream.rs:178-180`). Streaming requests MUST NOT
  drop warnings just because there is no terminal result struct.

- **Legacy converter path (direct messages).** Callers that build
  `LanguageModelV4Message::System { content: Vec<UserContentPart>, .. }`
  directly (without going through `PromptEnvelope`) still flow through
  provider converters
  (`vercel-ai/{openai,anthropic,google,openai-compatible}/.../convert_*`).
  Those converters MUST emit `Warning::Unsupported` into the standard
  per-call warnings sink when collapsing non-text parts. This keeps
  test harnesses, MCP-injected provider calls, and any future
  non-envelope caller from silently losing content.

The collapse pattern is the same in both paths:

```rust
fn flatten_system_text(
    parts: &[UserContentPart],
    warnings: &mut Vec<Warning>,
) -> String {
    let mut text = String::new();
    for part in parts {
        match part {
            UserContentPart::Text(t) => text.push_str(&t.text),
            other => warnings.push(Warning::Unsupported {
                feature: format!("non-text part in System role: {:?}", other.kind()),
                details: Some("dropped — provider does not accept multimodal System content".into()),
            }),
        }
    }
    text
}
```

Until a provider explicitly supports multimodal System content, callers
SHOULD construct System with text-only parts. The `Vec<UserContentPart>`
shape future-proofs the role; the Warning ensures non-text content cannot
silently disappear during today's text-only collapse.

This also affects `services/inference/src/client.rs:608-620` `extract_system_text`,
which today reads `content: &String`; the migration MUST update it to iterate
parts using the same flatten helper, so cache-hash inputs stay consistent
with the wire body.

The semantic meaning is:

| Role | Meaning |
|---|---|
| `System` | Provider-native system instruction or assistant identity. |
| `Developer` | Application, runtime, and tool policy instructions controlled by Coco. |
| `User` | User input or contextual user information. |
| `Assistant` | Assistant history. |
| `Tool` | Tool results and tool-related content. |

`Developer` is required because relying on `System` plus provider-specific
conversion hides prompt intent from query code, cache detection, tests, and
future provider adapters.

## 6. Provider Layout Mapping

Provider layout adapters map semantic prompt sections into wire families:

```rust
pub enum PromptWireFamily {
    OpenAiResponses,
    AnthropicMessages,
    GeminiGenerateContent,
    ChatLike,
}
```

**Native slots ride on `provider_options["prompt_layout"]`.** They are
NOT added as fields on `LanguageModelV4CallOptions` (see §3 layering
note). The payload type and helpers live in `coco-inference`:

```rust
// In coco-inference:
pub struct PromptLayoutOptions {
    pub instructions: Option<String>,
    pub system_blocks: Option<Vec<AnthropicSystemBlock>>,
    pub system_instruction: Option<String>,
    pub layout_warnings: Vec<Warning>,
    pub prompt_hash_inputs: Option<PromptHashInputs>,
}

pub struct AnthropicSystemBlock {
    pub text: String,
    pub cache_control: Option<AnthropicCacheControl>,
}
```

Layout adapter ownership is `services/inference`. Selection inputs are
`ProviderClientFingerprint` (provider, api, wire_api, base_url) and
`ModelInfo`. `app/query` does not see the family, the layout, or the
native slots; it constructs only `PromptEnvelope`. The adapter
serializes `PromptLayoutOptions` into the `"prompt_layout"` namespace via
`coco-inference`'s `put_layout_options` helper (see §3).

**Write order with `build_call_options`.** Today's
`build_call_options` (`services/inference/src/build_call_options.rs:148`)
constructs a fresh `ProviderOptions` and `set`s the canonical
provider namespace (e.g. `"openai"`, `"anthropic"`). To avoid clobbering
the layout namespace, the inference flow MUST be:

1. `build_call_options` runs first. It produces base
   `LanguageModelV4CallOptions` with `provider_options` populated for
   the canonical provider namespace (thinking + per-call extra_body +
   Anthropic `context_management`).
2. The layout adapter runs second. It calls `put_layout_options`,
   which reads the existing `provider_options`, ensures the inner
   namespace map is allocated, and inserts the `"prompt_layout"` entry
   non-destructively. `put_layout_options` MUST NOT replace the
   whole `ProviderOptions`; it MUST merge into the existing map.
3. Cache hashing runs last (see §11), reading the merged result.

If the order is reversed (layout writes `prompt_layout` first, then
`build_call_options` constructs a fresh `ProviderOptions` and sets
the canonical namespace) the layout payload is lost. The
implementation plan (§13 step 12) encodes the order; tests in §14.8
assert it.

Provider crates consume the `PromptLayoutOptions` payload from
`provider_options["prompt_layout"]`:
- `vercel-ai-openai` reads `PromptLayoutOptions::instructions` and writes
  it into `body["instructions"]` (the write site already exists at
  `vercel-ai/openai/src/responses/openai_responses_language_model.rs:300`,
  currently sourced from `openai_options.instructions` — the layout
  payload becomes the new authoritative source; the
  `openai_options.instructions` path remains for externally supplied
  instructions only when the layout slot is `None`. If both are
  present, the layout slot wins and a `Warning::Other` is emitted).
- `vercel-ai-anthropic` reads `PromptLayoutOptions::system_blocks` and
  writes it into `body["system"]` with `cache_control` metadata
  pre-attached per block.
- `vercel-ai-google` reads `PromptLayoutOptions::system_instruction`
  and writes it into `GenerateContentConfig.systemInstruction`.
- All three propagate `PromptLayoutOptions::layout_warnings` into
  `LanguageModelV4GenerateResult::warnings`.
- Provider crates do NOT re-derive these slots from the prompt stream;
  the message stream is for `messages` / `input[]` / `contents[]` only.

The mapping is:

| Semantic Layer | OpenAI Responses | Anthropic Messages | Gemini GenerateContent | Chat-like Providers |
|---|---|---|---|---|
| Base instructions | top-level `instructions` | top-level `system[]` | `systemInstruction` | system/developer-capable message |
| Developer policy | `input[].role = "developer"` | top-level `system[]` | `systemInstruction` | developer if supported, else system |
| Project instructions | contextual user | top-level `system[]` or meta user by policy | system tier or user context by semantic placement | user/system fallback |
| Environment | contextual user | top-level `system[]` or meta user by policy | user context before history | user context |
| Runtime user context | user before history | meta user before history | user before history | user before history |
| Cache hints | prompt-state hash | Anthropic `cache_control` metadata | prompt-state hash plus `cachedContent` option when configured | prompt-state hash |

Provider layout is selected from `ProviderClientFingerprint`, `ProviderApi`, and
`WireApi`. The selected layout is part of request construction, so fallback
providers build their own prompt layout.

Wire-format decisions per `(provider, model_id)` — system-instruction fallback
(Gemma-style), developer-message support (early OpenAI Responses models),
function-response part shape (Gemini 2.5 vs Gemini 3), thinking-config shape
(`thinkingBudget` vs `thinkingLevel`), modern-vs-legacy prompt features — are
owned entirely by the matching `vercel-ai-*` crate and resolved internally
from `model_id`. The layout adapter passes the envelope and the resolved
`ModelInfo` (carrying the `Capability` set) through; per-provider wire
encoding is the provider crate's responsibility.

There is no `PromptModelCapabilities` struct and no `ThinkingConfigShape`
enum at the cross-crate boundary. `coco_types::Capability` continues to
describe model bits that affect **prompt content selection or runtime
behavior** (e.g. `Vision`, `ToolCalling`, `ExtendedThinking`,
`ParallelToolCalls`); it does not describe wire-format encoding bits, which
are provider-internal.

### 6.1 Tool Definitions and Tool Config

Tool schemas, tool execution policy, and tool result content are **outside**
the `PromptEnvelope`. They flow through `services/inference::build_call_options`
as typed `Vec<LanguageModelV4Tool>` plus per-call options.

| Concern | Owner | Notes |
|---|---|---|
| Tool schema (`Vec<LanguageModelV4Tool>`) | `app/query::build_tool_definitions` builds; `services/inference::build_call_options` passes through | Today's path; unchanged by this design. |
| Per-provider tool serialization (`tools[].function`, OpenAI `parallel_tool_calls`, Anthropic `tool_use_id`, Gemini `functionDeclarations`) | each `vercel-ai-*` crate | Provider-internal. |
| Gemini `toolConfig` ↔ `tools` coupling | `vercel-ai-google` | The two fields are tightly coupled (selecting tools constrains config). The coupling lives in one place. |
| Per-turn `toolConfig` mutation by hooks (gemini-cli `geminiChat.ts:658-673`) | not yet ported; future hook design | Out of scope of this architecture. |
| Tool-call / tool-result content in conversation | `LanguageModelV4Message::Assistant` (`ToolCallPart`) and `Tool` (`ToolResultPart`) | Already typed; unchanged. |

The envelope describes only **prompt content**; tools are a sibling concept
to the envelope, not a section of it. This matches the §3.1 portable-concept
table and prevents tool schemas from being embedded in prompt text.

## 7. OpenAI Responses Contract

OpenAI Responses follows the Codex contract.

Layout rules (executed in `services/inference`'s OpenAI Responses adapter,
walking `PromptEnvelope.sections` in authoring order and dispatching by
`(kind, OpenAiResponses)` per the §4 routing table):

1. Sections routing to `instructions` (Identity, ModelBaseInstructions
   only) — collapse to text per §5 (warnings emitted on non-text parts)
   and concatenate into `PromptLayoutOptions::instructions`. `CacheHint`
   does not affect this routing.
2. Sections routing to `input[].role: developer` (DeveloperPolicy,
   ToolPolicy, SkillListing, McpInstructions) — emit as
   `LanguageModelV4Message::Developer` in the prompt stream, before
   contextual user items. This matches codex's `developer_sections`
   handling (`codex-rs/core/src/session/mod.rs:2627-2640`).
3. Sections routing to `input[].role: user` contextually
   (ProjectInstructions, Environment, Memory, LoadedContext, IdeContext,
   HookContext, ActiveTopic, UserContext) — emit as
   `LanguageModelV4Message::User` items in the prompt stream, before
   history. Routing is by kind only; `CacheHint::Stable` does not
   promote any of these into `instructions`.
4. Append `PromptEnvelope.history` to the prompt stream after the
   contextual user items.
5. Do not place base instructions in `input[]` — the `instructions`
   slot is the single source of truth for them.

The `vercel-ai-openai` crate reads `PromptLayoutOptions::instructions`
from the `provider_options["prompt_layout"]` namespace and writes it into
the request body's top-level `instructions` field at the existing write
site (`openai_responses_language_model.rs:300`), serializing the prompt
stream to `input[]` via the existing converter
(`convert_to_responses_input.rs:88-95`). If `openai_options.instructions`
is also present, the `prompt_layout` slot wins and the provider crate
emits a `Warning::Other` (per §6 precedence rule).

Wire result:

```json
{
  "model": "gpt-5",
  "instructions": "...base instructions...",
  "input": [
    { "role": "developer", "content": [{ "type": "input_text", "text": "..." }] },
    { "role": "user", "content": [{ "type": "input_text", "text": "..." }] }
  ]
}
```

Layout selection happens in `services/inference` based on
`ProviderClientFingerprint` (`provider == ProviderApi::Openai &&
wire_api == WireApi::Responses`). The OpenAI provider crate consumes
the typed `instructions` slot from `provider_options["prompt_layout"]` and
writes it into the request body. Neither `app/query` nor the OpenAI
crate constructs or branches on `ProviderApi`.

## 8. Anthropic Messages Contract

Anthropic Messages follows the TS prompt/cache contract.

Layout rules (executed in `services/inference`'s Anthropic Messages adapter,
walking `PromptEnvelope.sections` in authoring order and dispatching by
`(kind, AnthropicMessages)` per the §4 routing table):

1. Sections routing to `system[]` (Identity, ModelBaseInstructions,
   DeveloperPolicy, ToolPolicy, ProjectInstructions, Environment, Memory,
   SkillListing, McpInstructions) — collapse to text per §5, build
   `AnthropicSystemBlock { text, cache_control }` per section preserving
   envelope order, with `cache_control` populated from each section's
   `CacheHint`. DeveloperPolicy / ToolPolicy follow the system blocks in
   order (Anthropic has no `developer` role; authority degrades to
   system-block ordering).
2. Preserve static/dynamic boundaries via `CacheHint::Breakpoint`. The
   adapter places `cache_control` on the block whose hint requests a
   breakpoint after it.
3. Write the resulting `Vec<AnthropicSystemBlock>` to
   `PromptLayoutOptions::system_blocks` under the
   `provider_options["prompt_layout"]` namespace.
4. Sections routing to "meta user before history" (LoadedContext,
   IdeContext, HookContext, ActiveTopic, UserContext) — emit as
   user-role `LanguageModelV4Message::User` items in the prompt stream,
   before history.
5. Append `PromptEnvelope.history` to the prompt stream after the meta
   user prefix.

The `vercel-ai-anthropic` crate reads `PromptLayoutOptions::system_blocks`
from `provider_options["prompt_layout"]` and writes it directly into the
request body's `system` field. It does NOT iterate
`LanguageModelV4Message::System` entries from the prompt stream to
derive `system[]` — that derivation already happened in the layout
adapter.

Wire result:

```json
{
  "model": "claude-sonnet-4-6",
  "system": [
    { "type": "text", "text": "...identity..." },
    {
      "type": "text",
      "text": "...stable policy...",
      "cache_control": { "type": "ephemeral" }
    },
    { "type": "text", "text": "...dynamic context..." }
  ],
  "messages": [
    {
      "role": "user",
      "content": [{ "type": "text", "text": "<system-reminder>...</system-reminder>" }]
    }
  ]
}
```

The Anthropic provider owns validation for cache marker count and marker
position. Query code only supplies semantic cache hints.

## 9. Gemini GenerateContent Contract

Gemini GenerateContent follows the `gemini-cli` prompt/request contract.

Layout rules (executed in `services/inference`'s Gemini adapter, walking
`PromptEnvelope.sections` in authoring order and dispatching by
`(kind, GeminiGenerateContent)` per the §4 routing table):

1. Sections routing to `systemInstruction` (Identity, ModelBaseInstructions,
   DeveloperPolicy, ToolPolicy, SkillListing, McpInstructions) — collapse
   to text per §5 and concatenate preserving envelope order; write to
   `PromptLayoutOptions::system_instruction` under the
   `provider_options["prompt_layout"]` namespace. Do not create synthetic
   Gemini developer or hidden messages.
2. Sections routing to user-role `contents[]` prepend (ProjectInstructions,
   Environment, Memory, LoadedContext, IdeContext, HookContext,
   ActiveTopic, UserContext) — emit as user-role
   `LanguageModelV4Message::User` items in the prompt stream, before
   history. Session/environment context follows gemini-cli's
   `getInitialChatHistory` shape
   (`gemini-cli/packages/core/src/utils/environmentContext.ts:96-100`):
   environment text becomes the first user content item.
3. Append `PromptEnvelope.history` to the prompt stream after the
   contextual user items, using only Gemini `user` and `model` roles.
5. Convert tool results into user-role function response parts and emit
   them in adjacency-correct order (function response immediately after the
   matching function call).
6. Emit tool declarations through provider tool conversion into
   `GenerateContentConfig.tools`.
7. Emit tool execution policy through `toolConfig` when present.
8. Keep `thinkingConfig`, `safetySettings`, `cachedContent`, response
   modalities, labels, retrieval config, and model fallback settings out of the
   prompt envelope. They are request/model options.
9. Gemini-family differences (Gemini 2.5 budget thinking, Gemini 3
   `thinkingLevel`, multimodal function-response parts, system-instruction
   fallback for any future model that lacks it) are resolved inside
   `vercel-ai-google` from `model_id`. The layout adapter does not branch
   on these. (Note: gemini-cli sends `systemInstruction` to all current
   Gemini and Gemma chat models via `chat-base-3` config — the fallback
   capability exists for forward compatibility, not for any specific
   shipping model today.)

**Function-call adjacency enforcement.** The Gemini API server-side rejects
out-of-order `contents[]`. gemini-cli relies on orchestration to keep order
(`packages/core/src/core/geminiChat.ts:156-167` only validates role).
Today's coco Google converter
(`vercel-ai/google/src/convert_to_google_generative_ai_messages.rs:264-294`)
maps `ToolCallPart` and `ToolResultPart` without final adjacency validation.

Adjacency validation lives in **`vercel-ai-google` post-conversion, before
HTTP**. After `convert_to_google_generative_ai_messages` produces
`Vec<GoogleGenerativeAIContent>`, a validator walks `contents[]` and
asserts that every `FunctionResponse` part is immediately preceded by the
matching `FunctionCall` (same `name`, in the previous content's parts or
the current content's prior part). Violations return an emit-time error,
not a server 400. The validator runs after normalization-driven reordering
(reminder injection, history filtering) so it sees the exact sequence sent
on the wire.

Wire result:

```json
{
  "model": "gemini-2.5-pro",
  "contents": [
    {
      "role": "user",
      "parts": [{ "text": "<session_context>...</session_context>" }]
    }
  ],
  "systemInstruction": {
    "parts": [{ "text": "...base + developer + stable context..." }]
  },
  "tools": [{ "functionDeclarations": [] }],
  "toolConfig": {},
  "generationConfig": {
    "temperature": 1,
    "topP": 0.95,
    "thinkingConfig": {
      "includeThoughts": true,
      "thinkingBudget": 8192
    }
  }
}
```

For Gemini 3-style models, the resolved model config may instead produce:

```json
{
  "generationConfig": {
    "thinkingConfig": {
      "thinkingLevel": "HIGH"
    }
  }
}
```

The Google provider owns final serialization:

- `systemInstruction` is omitted for models that do not support it, with system
  text folded into the first user content item only as an explicit provider
  fallback.
- `contents[]` never contains `system` or `developer` roles.
- Provider metadata for thought signatures remains on assistant parts and is
  preserved during history normalization.
- Raw provider options may still shallow-merge at the provider boundary, but app
  and query code should use typed model/request settings for Coco-produced
  values.

## 10. Chat-like Contracts

Chat-like providers use the clean semantic roles but map them conservatively:

1. If the provider supports developer messages, serialize `Developer` natively.
2. If it does not, fold `System` and `Developer` sections into the provider's
   system-compatible channel.
3. Serialize user context as a user message before history.
4. Keep cache behavior limited to prompt-state hashing unless the provider has a
   typed cache feature.

### 10.1 `vercel-ai-openai-compatible` Routing

**Today's state.** `services/inference/src/fingerprint.rs:66-72` only
preserves `wire_api` for `ProviderApi::Openai`; for
`ProviderApi::OpenaiCompatible` it is hardcoded `None`.
`vercel-ai/openai-compatible/src/chat/openai_compatible_chat_language_model.rs:79`
explicitly always uses `role: system` and has no Developer-message path.
There is no Responses path in the openai-compatible crate today.

**Scoped state for this architecture.** openai-compatible inherits the
§10 chat-like contract only. `WireApi::Responses` routing for
openai-compatible is **out of scope** for this design and is not part of
the implementation plan in §13.

To enable openai-compatible Responses in a future iteration, three pieces
of work are required and are not part of this architecture:
1. `ProviderClientFingerprint::compute` extends `wire_api` preservation
   to `ProviderApi::OpenaiCompatible`.
2. `vercel-ai-openai-compatible` gains a Responses code path mirroring
   `vercel-ai-openai`'s `convert_to_responses_input` and
   `openai_responses_language_model`, including
   `SystemMessageMode::{System, Developer, Remove}` routing.
3. Layout adapter selection in `services/inference` adds a branch for
   `(OpenaiCompatible, Responses)` that populates the same
   `LanguageModelV4CallOptions` native slots as the OpenAI Responses
   adapter.

Until those land, all openai-compatible providers use the chat-like
contract regardless of the provider config's declared `wire_api`.

### 10.2 Out-of-Scope Providers

`vercel-ai-bytedance` is a video-generation (Seedance) provider. It has no
chat / prompt surface and is **out of scope** for this architecture. No
prompt-role mapping or layout adapter is required for it.

## 11. Cache and Prompt-State Invariants

**`PromptHashInputs` covers prompt-content only.** All `extra_body` /
`provider_options` shaping is merged in `build_call_options.rs:112-148`
**after** the layout adapter runs, so its hash cannot be computed inside
layout. The cache detector remains responsible for hashing those merged
fields at hash-call time.

```rust
// Prompt-content-derived only. Computed by the layout adapter and
// written into PromptLayoutOptions::prompt_hash_inputs. The cache
// detector at hash-call time combines these with:
//   - extra_body_hash / extra_body_serialized (computed AFTER
//     build_call_options.rs:112-148 has merged thinking + per-call
//     extra_body + Anthropic context_management into provider_options)
//   - non-prompt request state: model, query_source, agent_id,
//     fast_mode, betas, effort_value, global_cache_strategy,
//     auto_mode_active, is_using_overage, cached_mc_enabled
// to form the full PromptStateInput shape at
// services/inference/src/cache_detection.rs:329-365.
pub struct PromptHashInputs {
    /// Hash of the system-tier text (Anthropic system blocks /
    /// OpenAI instructions / Gemini systemInstruction). Computed
    /// AFTER multimodal-to-text flattening (with Warning emission
    /// per §5).
    pub system_text_hash: u64,
    pub system_char_count: i64,
    /// Hash of cache-control metadata across system blocks
    /// (Anthropic only — `Some` only for AnthropicMessages family).
    /// Replaces today's hardcoded `cache_control_hash: 0` at
    /// `services/inference/src/client.rs:587`.
    pub cache_control_hash: Option<u64>,
    /// Hash of developer-tier text (OpenAI Responses developer
    /// input items; folded into system_text_hash for Anthropic /
    /// Gemini where developer authority degrades to system).
    pub developer_text_hash: Option<u64>,
    /// Hash of contextual user prefix (the user-role items emitted
    /// before `history` per §7-§9). Captures
    /// ProjectInstructions/Environment/Memory drift on OpenAI and
    /// Gemini, where these route to user input rather than system.
    pub contextual_user_text_hash: u64,
    pub contextual_user_char_count: i64,
    /// Hash of tool schemas, in stable order.
    pub tools_hash: u64,
    /// Per-tool hashes for cache-break attribution.
    pub per_tool_hashes: Vec<(String, u64)>,
    pub tool_names: Vec<String>,
}
```

`extra_body_hash` and `extra_body_serialized` are **not** in
`PromptHashInputs` — they belong to the request-shaping layer that
runs after layout. The cache detector reads the merged
`call.provider_options` at hash time and computes those hashes itself,
preserving the existing field layout in `PromptStateInput`
(`cache_detection.rs:329-365`).

**`extra_body_hash` MUST exclude the `"prompt_layout"` namespace.** Since
the layout adapter writes into the same `provider_options` map, a naive
"hash all of `provider_options`" implementation would re-hash the
prompt slots, layout warnings, and `prompt_hash_inputs` as if they were
request-shaping extra body — double-counting prompt content in the
cache hash and conflating cache-detection inputs. The correct rule:
`extra_body_hash` covers only canonical provider request-shaping
namespaces (`"openai"`, `"anthropic"`, `"google"`, etc.) and skips any
reserved prompt-layout namespaces. The reserved set today is `{"prompt_layout"}`;
new prompt-layout-tier namespaces added in the future MUST also be excluded. Tests
in §14.7 assert that adding/removing a `"prompt_layout"` entry leaves
`extra_body_hash` unchanged.

Non-prompt-derived fields stay where they are today: the cache detector
constructs the full `PromptStateInput` by combining `PromptHashInputs`
with `(model, query_source, agent_id, fast_mode, betas, effort_value,
global_cache_strategy, auto_mode_active, is_using_overage,
cached_mc_enabled, extra_body_hash, extra_body_serialized)` — these come
from `ApiClient` / `QueryParams` / merged `provider_options` /
runtime state, not from prompt content.

**Field merge mapping into existing `PromptStateInput`** (today's
shape at `cache_detection.rs:329-365`). The migration mostly renames
and converts shapes; new prompt-content fields gain merge sites:

| `PromptStateInput` field | Type | Source | Conversion |
|---|---|---|---|
| `system_hash` | `u64` | `PromptHashInputs.system_text_hash` | direct (rename) |
| `system_char_count` | `i64` | `PromptHashInputs.system_char_count` | direct |
| `cache_control_hash` | `u64` | `PromptHashInputs.cache_control_hash: Option<u64>` | `unwrap_or(0)` |
| `tools_hash` | `u64` | `PromptHashInputs.tools_hash` | direct |
| `tool_names` | `Vec<String>` | `PromptHashInputs.tool_names` | direct |
| `per_tool_hashes` | `HashMap<String, u64>` | `PromptHashInputs.per_tool_hashes: Vec<(String, u64)>` | `into_iter().collect()` |
| `extra_body_hash` | `u64` | merged `provider_options` excluding `"prompt_layout"` | computed at hash time |
| `extra_body_serialized` | `Option<String>` | merged `provider_options` excluding `"prompt_layout"` | computed at hash time |
| `model`, `query_source`, `agent_id`, `fast_mode`, `betas`, `effort_value`, `global_cache_strategy`, `auto_mode_active`, `is_using_overage`, `cached_mc_enabled` | (existing) | `ApiClient` / `QueryParams` / runtime | unchanged |
| *(new — not in current `PromptStateInput`)* `developer_text_hash`, `contextual_user_text_hash`, `contextual_user_char_count` | — | `PromptHashInputs` | requires adding fields to `PromptStateInput` (§13 step 12) |

The `developer_text_hash` / `contextual_user_text_hash` /
`contextual_user_char_count` fields are **new on `PromptStateInput`**
because today's struct collapses everything into `system_hash` —
losing the OpenAI/Gemini contextual user prefix as a distinct hash
input. The migration adds those three fields; `cache_detection.rs`
state versioning bumps on schema change.

The migration replaces today's `build_prompt_state_input`
(`services/inference/src/client.rs:552-608`) prompt-content extraction
with reading from `provider_options["prompt_layout"].prompt_hash_inputs`.
The hardcoded `cache_control_hash: 0` is wired to the actual block-level
cache-control hash for the Anthropic family.

Prompt-state hashing serves a different purpose for each provider family.
The hash inputs are similar but the **role of the hash** differs:

| Provider family | Hash purpose | Hash inputs |
|---|---|---|
| Anthropic Messages | **Predict** client-side cache misses caused by `cache_control` marker movement, scope/TTL flips, or upstream content edits. Today's `cache_detection.rs` uses this to warn the user before a wasted request. | system block text (content-only hash), cache metadata (separate hash), contextual user prefix, tools, model, betas, request-shaping provider options |
| OpenAI Responses | **Explain** server-reported cache regressions after the fact. OpenAI Responses uses a server-side prefix cache (no client `cache_control`); `prompt_tokens_details.cached_tokens` reports actual hit rate. The hash answers "which input changed when the hit rate dropped." | top-level instructions, developer messages, contextual user prefix, tools, model, request-shaping provider options |
| Gemini GenerateContent | **Coco-only telemetry.** `cachedContent` is a server-side cache *handle*; gemini-cli does not hash. Coco may hash for the same explainability role as OpenAI; it is not required by the provider. | systemInstruction text, contextual user prefix, contents role sequence, tools, toolConfig, model, `cachedContent` handle, thinking request options |
| Chat-like | Same as OpenAI Responses (explainability). Most chat-like providers do not expose cache markers. | system/developer folded prompt, contextual user prefix, tools, model |

The three roles produce three slightly different implementation shapes:

- **Anthropic detector** runs **pre-send**, warns on predicted cache break,
  may suggest re-grouping. Already implemented in
  `services/inference/src/cache_detection.rs`.
- **OpenAI / Chat-like detector** runs **post-send**, compares declared hash
  against the previous turn's, surfaces a diff for telemetry. No predictive
  warning.
- **Gemini detector** is optional and coco-internal; do not block ship on it.

Hashing must use typed semantic inputs before provider serialization where
possible, plus provider-layout cache metadata where it affects the wire prompt.
Cache hints must never be represented as literal prompt text.

## 12. Crate Ownership

| Crate | Owns |
|---|---|
| `services/inference` | `PromptEnvelope` (single ordered `sections` + `history`), `PromptSection`, `PromptSectionKind`, `PromptSource`, `CacheHint`, `PromptLayoutOptions`, `AnthropicSystemBlock`, `PromptHashInputs`, `PromptWireFamily`, `(kind, family)` routing tables, layout adapter selection and execution from `ProviderClientFingerprint`, prompt-state hashing |
| `core/context` | Semantic prompt **builders** that produce `coco_inference::PromptEnvelope` (consumes the type from inference; does not own it — placing the type in `core/context` would create a `coco-context → coco-inference → ... → coco-context` cycle through `coco-messages`, which today already routes `coco-context → coco-messages → coco-inference` per `core/context/Cargo.toml:12` and `core/messages/Cargo.toml:12`) |
| `vercel-ai/provider` | `LanguageModelV4Message::Developer`. **No** Coco-extended fields on `LanguageModelV4CallOptions`. Native slots ride on the existing `ProviderOptions` namespace mechanism (`provider_options["prompt_layout"]`), preserving the "Standalone type definitions matching `@ai-sdk/provider` v4" contract (`vercel-ai/provider/CLAUDE.md:1-3`). |
| `app/query` | turn-specific envelope construction via `core/context` builders (envelope only — no layout invocation, no provider-shape derivation) |
| `vercel-ai/openai` | Read `PromptLayoutOptions::instructions` from `provider_options["prompt_layout"]`; write to `body["instructions"]`; precedence over `openai_options.instructions` with `Warning::Other` on conflict; serialize prompt stream to `input[]`; bubble `layout_warnings` into result (generate: `LanguageModelV4GenerateResult::warnings`; stream: `StreamStart` part); OpenAI request body |
| `vercel-ai/anthropic` | Read `PromptLayoutOptions::system_blocks`; write to `body["system"]`; serialize prompt stream to `messages`; bubble `layout_warnings`; Anthropic request body; cache marker validation |
| `vercel-ai/google` | Read `PromptLayoutOptions::system_instruction`; write to `GenerateContentConfig.systemInstruction`; serialize prompt stream to `contents[]`; **post-conversion adjacency validation**; bubble `layout_warnings`; Google request body |
| `vercel-ai/openai-compatible` | OpenAI-compatible chat-like prompt role mapping and request body (Responses path out of scope per §10.1) |

Lower-layer crates must not depend on app-layer prompt assembly.

## 13. Implementation Plan

Implement the target architecture directly:

1. Change `LanguageModelV4Message::System` to carry `Vec<UserContentPart>`,
   add `LanguageModelV4Message::Developer` (same shape) to `vercel-ai-provider`.
   These are SDK-fidelity changes (mirror `@ai-sdk/provider` v4).
2. Add `PromptEnvelope` (single ordered `sections: Vec<PromptSection>`
   plus `history: Vec<LanguageModelV4Message>`), `PromptSection` (with
   `content: Vec<PromptPart>`), `PromptSectionKind`, `PromptSource`,
   `CacheHint`, `PromptLayoutOptions`, `AnthropicSystemBlock`,
   `PromptHashInputs`, and the `PromptPart` alias (resolving via
   `coco_inference::UserContentPart`) to **`coco-inference`**, not to
   `core/context` (avoids the dep cycle described in §12). There is no
   fixed kind→slot assignment at envelope-construction time; routing is
   per `(kind, provider family)` at the layout adapter. Routing is by
   kind only — `CacheHint` does NOT promote authority tier.
3. Add `coco-inference` helpers to read/write `PromptLayoutOptions` under
   the `provider_options["prompt_layout"]` namespace
   (`put_layout_options` / `take_layout_options`). The wire format (JSON
   shape under `"prompt_layout"`) is the cross-layer contract; provider
   crates depend only on `vercel-ai-provider` and parse the namespace
   payload.
4. Update all provider prompt converters
   (`vercel-ai-{openai,anthropic,google,openai-compatible}`) to handle
   the new `System` shape and `Developer` exhaustively. Text-only
   collapse MUST emit `Warning::Unsupported` for non-text parts via
   **both** paths (per §5): layout-driven collapse routes warnings
   through `PromptLayoutOptions::layout_warnings`; direct-converter
   callers (legacy / non-envelope code paths, e.g. test harnesses or
   any pre-envelope `LanguageModelV4Message::System` construction) emit
   through the converter's own per-call warnings sink. Neither path
   may silent-drop.
5. Add a new direct dependency edge `coco-context → coco-inference` in
   `core/context/Cargo.toml` (today: `coco-types`, `coco-messages`,
   `coco-config`, `coco-otel` only — `core/context/Cargo.toml:11-15`).
   This edge is cycle-free because `coco-messages` already deps
   `coco-inference`; the new edge just makes the existing transitive
   path explicit. Add semantic prompt builders to `core/context` that
   construct a `coco_inference::PromptEnvelope`. `core/context`
   consumes the type from `coco-inference`; it does not own it.
6. Replace query prompt construction with structured envelope
   construction via `core/context` builders. `app/query` builds
   `PromptEnvelope` only — no layout invocation, no provider-shape
   derivation. `PromptEnvelope.history` carries the full
   already-normalized conversation (including reminder messages from the
   `coco-system-reminder` pipeline).
7. Add `PromptWireFamily` and the `(kind, family)` routing tables to
   `services/inference`. Layout selection routes from
   `ProviderClientFingerprint`. There is no `RequestPromptOptions` enum
   and no Coco-extended fields on `LanguageModelV4CallOptions`; layout
   adapters write to `provider_options["prompt_layout"]`.
8. Implement the OpenAI Responses layout adapter in `services/inference`,
   populating `PromptLayoutOptions::instructions` from kinds routed to
   `instructions` (Identity + ModelBaseInstructions only — NOT
   SkillListing/McpInstructions/Memory), emitting `Developer`/`User`
   prompt items per the §7 rules. Wire `vercel-ai-openai` to consume
   `PromptLayoutOptions::instructions` at the existing
   `body["instructions"]` write site
   (`openai_responses_language_model.rs:300`), with precedence over
   `openai_options.instructions` and `Warning::Other` on conflict.
9. Implement the Anthropic Messages layout adapter, populating
   `PromptLayoutOptions::system_blocks` with `cache_control` pre-attached
   per block. Wire `vercel-ai-anthropic` to consume the slot directly
   into `body["system"]`.
10. Implement the Gemini GenerateContent layout adapter, populating
    `PromptLayoutOptions::system_instruction`. Wire `vercel-ai-google` to
    consume the slot and to run **post-conversion adjacency validation**
    before HTTP.
11. Implement chat-like layout for openai-compatible (Responses path
    out of scope per §10.1).
12. Wire prompt-state hashing to read prompt-content-derived fields from
    `PromptLayoutOptions::prompt_hash_inputs` and merge them at hash-call
    time with the request-shaping `extra_body_hash` /
    `extra_body_serialized` (computed AFTER `build_call_options` merge,
    `build_call_options.rs:112-148`) plus the existing non-prompt
    request-state fields in `PromptStateInput`. Remove the hardcoded
    `cache_control_hash: 0` at `services/inference/src/client.rs:587`.
    Update `extract_system_text` (line 608) to iterate
    `Vec<UserContentPart>` via the §5 flatten helper as a fallback path
    when `prompt_layout` is absent (e.g. legacy callers).
13. Provider crates bubble `PromptLayoutOptions::layout_warnings` into
    `LanguageModelV4GenerateResult::warnings` so callers see dropped non-text
    parts and other layout issues.
14. Add abstraction-boundary tests proving `app/query` produces only
    `PromptEnvelope`, that layout invocation lives in
    `services/inference`, that `provider_options["prompt_layout"]` is
    populated only by the layout adapter, and that
    `core/context → core/messages → coco-inference` direction stays
    cycle-free.
15. Update provider, inference, context, and query tests.

## 14. Tests

### 14.1 OpenAI Responses

Tests must assert:

- Only `Identity` and `ModelBaseInstructions` route to top-level
  `instructions`. `SkillListing`, `McpInstructions`, `DeveloperPolicy`,
  `ToolPolicy` route to `input[].role: developer` (matching codex's
  `developer_sections` shape).
- `ProjectInstructions`, `Environment`, `Memory`, `LoadedContext`,
  `IdeContext`, `HookContext`, `ActiveTopic`, `UserContext` route to
  `input[].role: user` regardless of `CacheHint`. A
  `Memory { CacheHint::Stable }` section MUST appear in `input[]` as
  user, NOT in `instructions`.
- `Developer` serializes as `input[].role = "developer"`.
- Contextual user sections serialize before conversation history.
- Base instructions do not appear in `input[]`.
- Changing top-level instructions changes prompt-state hash.
- **Layout slot vs `openai_options` precedence.** When both
  `provider_options["prompt_layout"].instructions` and
  `openai_options.instructions` are populated, the layout slot wins
  and a `Warning::Other` appears in
  `LanguageModelV4GenerateResult::warnings`.

### 14.2 Anthropic Messages

Tests must assert:

- `System` and `Developer` serialize into ordered top-level `system[]`.
- Cache hints become Anthropic `cache_control` metadata.
- `CacheHint::Breakpoint` affects block grouping, not literal text.
- User context prepends as a meta user message before history.
- Changing system block text or cache metadata changes prompt-state hash.

### 14.3 Gemini GenerateContent

Tests must assert:

- `System` and `Developer` sections serialize into `systemInstruction`, not
  `contents[]`.
- `contents[]` contains only `user` and `model` roles.
- Session/environment context prepends as user content before history.
- Tool results serialize as user-role function response parts.
- **Function-call/function-response adjacency is enforced at emit time.**
  An out-of-order envelope produces an emit-time error, not a server 400.
- Gemini 2.5 model config emits budget-based `thinkingConfig`.
- Gemini 3 model config emits level-based `thinkingConfig` when configured.
- Future Gemini-family models without system-instruction support fold system
  text into the first user content item as an explicit provider fallback
  (capability hook in `vercel-ai-google`'s `ConvertOptions`; no current
  shipping model exercises this path).
- Changing `systemInstruction`, `toolConfig`, `cachedContent`, or thinking
  request options changes prompt-state hash.

### 14.4 Fallback Layout

Tests must assert:

- A fallback provider rebuilds prompt layout for its own wire family.
- OpenAI Responses fallback to Anthropic produces Anthropic `system[]`, not
  OpenAI `instructions`.
- Anthropic fallback to OpenAI Responses produces top-level `instructions`, not
  Anthropic system blocks.
- **Cache state on fallback is cold.** `CacheBreakDetector` is per-source
  (per `ProviderClientFingerprint`); a fallback to a different provider
  produces no cache-break warning because there is no prior baseline. Tests
  must assert no spurious warning is raised on the first fallback turn and
  that subsequent turns on the fallback provider establish a fresh baseline.

### 14.5 Provider Exhaustiveness

Tests must assert:

- Every provider converter handles `Developer`.
- Chat-like providers either serialize developer natively or fold it into their
  system-compatible prompt path.
- Unknown provider families cannot silently drop developer sections.

### 14.6 Abstraction Boundaries

Tests must assert:

- `app/query` constructs `PromptEnvelope` (single ordered `sections` list
  plus `history`), not OpenAI, Anthropic, or Gemini wire fields.
  `app/query` does not invoke the layout adapter and does not read
  `LanguageModelV4CallOptions` native slots.
- Layout adapter selection and execution live in `services/inference`. The
  inference layer routes from `ProviderClientFingerprint` and populates
  `LanguageModelV4CallOptions` native slots.
- Provider crates consume the `PromptLayoutOptions` payload from
  `provider_options["prompt_layout"]` directly: `vercel-ai-openai` reads
  `instructions`, `vercel-ai-anthropic` reads `system_blocks`,
  `vercel-ai-google` reads `system_instruction`. None of them re-derive
  these from the prompt stream.
- Provider crates bubble `PromptLayoutOptions::layout_warnings` into
  `LanguageModelV4GenerateResult::warnings`.
- Provider fallback re-runs layout selection in `services/inference` against
  the fallback fingerprint with the unchanged envelope, populating fresh
  native slots shaped for the fallback family.
- Same kind in different providers may route differently: a test that
  places an `Environment` section MUST produce an Anthropic system block
  AND a Gemini user-role `contents[]` prepend AND an OpenAI
  `input[].role: user` contextual item.
- Tool schemas and tool execution policy are emitted as typed tool/request
  options through `services/inference::build_call_options`, not embedded into
  prompt text and not part of the envelope.
- Request-shaping options such as thinking, safety settings, response modality,
  and cached content do not appear inside prompt sections.

### 14.7 Hash / Wire Coherence

Tests must assert:

- `PromptLayoutOptions::prompt_hash_inputs.system_text_hash` (read from
  `provider_options["prompt_layout"]`) matches a hash recomputed over the
  wire body's system-tier text (Anthropic `body["system"]` joined,
  OpenAI `body["instructions"]`, Gemini `body["systemInstruction"]`).
- `prompt_hash_inputs.contextual_user_text_hash` covers the contextual
  user prefix on OpenAI/Gemini and the meta-user prefix on Anthropic.
- For Anthropic, `prompt_hash_inputs.cache_control_hash` is `Some` and
  reflects the actual `cache_control` metadata on `system_blocks`. A
  test that flips a single block's cache_control TTL must change this
  hash.
- For OpenAI Responses and Gemini, `cache_control_hash` is `None`
  (provider-side cache, no client markers).
- `extra_body_hash` is computed by the cache detector AFTER
  `build_call_options` merges thinking + per-call extra_body +
  Anthropic `context_management`
  (`build_call_options.rs:112-148`); it is NOT in
  `PromptHashInputs`. A test that mutates per-call `extra_body` MUST
  change `extra_body_hash` but leave `prompt_hash_inputs` unchanged.
- **Type-shape conversions on merge.** Tests assert the
  `PromptHashInputs → PromptStateInput` mapping in §11:
  `cache_control_hash: Option<u64>` collapses via `unwrap_or(0)` for
  OpenAI/Gemini (no client cache markers); `per_tool_hashes:
  Vec<(String, u64)>` converts to `HashMap<String, u64>` via
  `into_iter().collect()`; `system_text_hash` is the same value as
  the existing `system_hash` field.
- **Streaming warning bubble.** A streaming request whose layout
  produces a `Warning::Unsupported` MUST surface that warning in the
  first `LanguageModelV4StreamPart::StreamStart` part (`stream.rs:178-180`).
  A test that consumes the stream MUST observe the warning before any
  text/tool delta arrives.
- **`extra_body_hash` excludes `"prompt_layout"`.** A test that
  populates `provider_options["prompt_layout"]` (with `instructions`,
  `system_blocks`, `layout_warnings`, etc.) MUST NOT change
  `extra_body_hash`. Adding any future prompt-layout-tier namespace MUST
  also leave `extra_body_hash` unchanged.
- **Write-order coherence with `build_call_options`.** A test that
  runs `build_call_options` (which constructs a fresh
  `ProviderOptions` and `set`s the canonical provider namespace,
  `build_call_options.rs:148`) followed by the layout adapter's
  `put_layout_options` MUST observe both namespaces present at
  hash time. Reversing the order MUST be either rejected at the API
  boundary or covered by `put_layout_options` merging
  non-destructively.
- The cache detector merges `prompt_hash_inputs` with `extra_body_hash`
  / `extra_body_serialized` and the non-prompt-derived fields (model,
  query_source, agent_id, fast_mode, betas, effort_value,
  global_cache_strategy, auto_mode_active, is_using_overage,
  cached_mc_enabled) into `PromptStateInput`. A test that changes only
  `query_source` MUST still produce a different `PromptStateInput`
  hash even though `prompt_hash_inputs` is unchanged.
- Multimodal-collapse warning bubbles end-to-end: a System section
  containing `[Text("a"), File(image), Text("b")]` produces hash inputs
  over `"ab"`, raises a `Warning::Unsupported` in
  `PromptLayoutOptions::layout_warnings`, and the warning appears in
  `LanguageModelV4GenerateResult::warnings` after the request completes.

### 14.8 Crate Layering

Tests / build assertions must enforce:

- `coco-inference` does NOT depend on `coco-context`. The dependency
  direction is `coco-context → coco-messages → coco-inference`
  (`core/context/Cargo.toml:12`, `core/messages/Cargo.toml:12`).
  `PromptEnvelope` and `PromptLayoutOptions` live in `coco-inference`;
  `core/context` only contributes builders.
- `vercel-ai-provider` does NOT depend on `coco-inference` or any
  other coco crate. The Coco-specific layout payload travels in
  `provider_options["prompt_layout"]` as JSON; provider crates parse it
  without importing coco types.
- Provider crates (`vercel-ai-{openai,anthropic,google,openai-compatible}`)
  do NOT depend on `coco-inference`. They depend on
  `vercel-ai-provider` only and parse `prompt_layout` namespace payload
  via local serde-deserializable structs (or a future `prompt_layout`
  extension trait on `ProviderOptions`).

## 15. Rust Invariants

The implementation must enforce these invariants:

1. `PromptSectionKind`, `PromptSource`, `CacheHint`, and `PromptWireFamily` are
   enums, not raw strings.
2. Provider wire keys are written only by provider layout adapters or provider
   crates.
3. `app/query` does not construct OpenAI, Anthropic, Google, or compatible raw
   request JSON.
4. `Developer` is never silently dropped.
5. Base instructions are never duplicated between OpenAI `instructions` and
   OpenAI `input[]`.
6. Anthropic system blocks are never flattened before cache hints are applied.
7. Gemini `contents[]` never contains synthetic system or developer roles.
8. Gemini function responses preserve function-call adjacency. The Google
   adapter validates this at emit time (stricter than gemini-cli, which
   relies on orchestration); an out-of-order envelope is a programming
   error, not a runtime fallback.
9. Semantic authority levels are not inferred from provider wire role names.
10. Provider-shape (top-level `instructions`, `system[]`, `systemInstruction`)
    is produced by the **layout adapter in `services/inference`** as a
    typed `PromptLayoutOptions` payload written under
    `provider_options["prompt_layout"]`, and consumed verbatim by the
    matching `vercel-ai-*` crate. `app/query` never constructs them;
    provider crates never re-derive them from the prompt stream. The
    native-slot types live in `coco-inference` — not on
    `LanguageModelV4CallOptions` in `vercel-ai-provider` — to preserve
    that crate's "Standalone @ai-sdk/provider v4 type definitions" contract.
11. Cache hints are metadata, not prompt text.
12. Prompt-state hashes include all provider-visible prompt-shaping inputs.
13. Provider fallback always rebuilds prompt layout.
14. Per-turn `<system-reminder>` injections enter `MessageHistory` as
    `Message::Attachment(AttachmentMessage::api(...))` via the
    `coco-system-reminder` pipeline (`core/system-reminder/src/inject.rs:218-228`)
    and reach the layout adapter as ordinary user-role `LlmMessage`s in
    `PromptEnvelope.history`. They are not envelope sections. `UserMessage`
    has no `is_meta` field (post-Phase-2,
    `core/messages/src/predicates.rs:36`); meta semantics live on the
    `AttachmentKind` wrapper, not on the LLM message.
15. `provider_options` set on `Developer` (and `System`) messages flow through
    every provider converter unchanged; converters MUST handle `Developer`
    exhaustively.
16. Layout adapters MUST preserve envelope-authored order **within each
    provider's destination slot**. They MAY (and must) split the single
    `sections` list across multiple destination slots per the §4 routing
    table — that is routing, not reordering. Reordering for cache
    stability or other reasons happens upstream of envelope construction.
17. Tests use companion `*.test.rs` files for Rust modules.
18. Layout adapter selection lives in `services/inference`, never in
    `app/query`. Provider fallback re-runs layout selection in
    `services/inference` against the fallback fingerprint with the
    unchanged envelope.
19. Prompt-content-derived cache-hash inputs are read from
    `PromptLayoutOptions::prompt_hash_inputs` under
    `provider_options["prompt_layout"]`, not by re-walking the prompt
    stream. The cache detector merges these with `extra_body_hash`
    (computed after `build_call_options` merge) and other non-prompt
    request fields. Hash and wire body share the same intermediate
    representation; `cache_control_hash` reflects actual Anthropic
    block-level cache_control metadata, not a hardcoded zero.
20. Layout adapters that flatten multimodal `Vec<UserContentPart>` to
    text (for system / developer / cached text-only slots) MUST emit
    `Warning::Unsupported` into `PromptLayoutOptions::layout_warnings`
    (under `provider_options["prompt_layout"]`) for any non-text part
    dropped. Provider crates bubble those warnings into
    `LanguageModelV4GenerateResult::warnings`. Silent drop at any layer is a
    correctness violation.
21. `PromptSectionKind` has NO global slot assignment. Routing is
    per-`(kind, provider family)` per the §4 routing table; the same
    kind can route differently across providers (e.g. `Environment` →
    Anthropic system block, OpenAI/Gemini contextual user). Layout
    adapters MUST follow the routing table; ad-hoc deviations are
    forbidden.
22. Routing is by `(kind, family)` only — never by `CacheHint`.
    `CacheHint::Stable` does NOT promote `Memory` (or any other
    user-tier kind) into `instructions` / system tier on OpenAI or
    Gemini. Authority and cache are independent axes.
23. Layout-derived native slots live on `coco_inference::PromptLayoutOptions`
    and ride on `provider_options["prompt_layout"]`. They are NOT added
    as fields on `vercel_ai_provider::LanguageModelV4CallOptions` —
    that crate is contractually "Standalone type definitions matching
    `@ai-sdk/provider` v4. Zero dependencies on other coco crates"
    (`vercel-ai/provider/CLAUDE.md:1-3`).
24. `PromptEnvelope` and related types (`PromptSection`,
    `PromptSectionKind`, `PromptSource`, `CacheHint`,
    `PromptLayoutOptions`, `AnthropicSystemBlock`, `PromptHashInputs`)
    live in `coco-inference`, not in `core/context`. Placing them in
    `core/context` would form a cycle through `coco-context →
    coco-messages → coco-inference` (`core/context/Cargo.toml:12`,
    `core/messages/Cargo.toml:12`). `core/context` consumes the type
    via builders.
25. Layout adapters surface section-level issues (non-text content
    dropped from system/developer text-only collapse, malformed cache
    hints, etc.) via `PromptLayoutOptions::layout_warnings`. Provider
    crates bubble these into `LanguageModelV4GenerateResult::warnings`. Silent
    loss of content at any layer is a correctness violation.
26. `PromptHashInputs` carries only the **prompt-content-derived**
    inputs to the cache detector. Request-shaping inputs
    (`extra_body_hash`, `extra_body_serialized`) are computed by the
    cache detector AFTER `build_call_options` merge
    (`build_call_options.rs:112-148`), not by the layout adapter.
    Non-prompt-derived state fields (model, query_source, agent_id,
    fast_mode, betas, effort_value, global_cache_strategy,
    auto_mode_active, is_using_overage, cached_mc_enabled) remain
    owned by the cache detector and are merged at hash-call time.
27. When both `PromptLayoutOptions::instructions` and
    `openai_options.instructions` are present in a call, the layout
    slot wins and the provider crate emits a `Warning::Other`
    documenting the override. Tests assert this precedence (§14.7).
28. `extra_body_hash` MUST exclude reserved prompt-layout namespaces
    (currently `{"prompt_layout"}`; any future prompt-layout-tier namespace
    is added to the exclusion set on introduction). Hashing
    `provider_options` blindly would double-count prompt content as
    request-shaping extra body; the cache detector iterates only the
    canonical provider request-shaping namespaces.
29. `build_call_options` runs **before** the layout adapter; the
    layout adapter merges `"prompt_layout"` non-destructively into the
    existing `provider_options` map. `put_layout_options` MUST NOT
    replace the entire `ProviderOptions`. Reversing the order
    requires `build_call_options` to accept and preserve a passed-in
    `ProviderOptions` instead of constructing a fresh one
    (`build_call_options.rs:148`).
30. Provider crates parse the `"prompt_layout"` namespace via local
    serde-deserializable mirror structs, NOT by importing
    `PromptLayoutOptions` from `coco-inference`. The wire-format JSON
    shape under `"prompt_layout"` is the cross-layer contract; the Rust
    type lives only in `coco-inference`. Provider crates depend on
    `vercel-ai-provider` only.
31. The text-only multimodal collapse Warning emission has TWO paths,
    BOTH MUST emit `Warning::Unsupported` for non-text parts (see §5
    and §13 step 4): the layout-driven path writes to
    `PromptLayoutOptions::layout_warnings`; the direct-converter path
    (legacy / non-envelope callers building `LanguageModelV4Message::System`
    or `Developer` directly) emits via the converter's per-call
    warnings sink. Silent drop on either path is a correctness
    violation.
32. Warnings reach callers via TWO request-shape sinks: `do_generate`
    requests surface them in `LanguageModelV4GenerateResult::warnings`
    (`generate_result.rs:21`); `do_stream` requests surface them in
    the first `LanguageModelV4StreamPart::StreamStart { warnings }`
    part (`stream.rs:178-180`). Provider crates must populate the
    correct sink for the request shape; streaming callers do not have
    a terminal result struct, so dropping warnings there is silent.
33. `PromptLayoutOptions` fields are stored as **separate inner keys**
    under `provider_options["prompt_layout"]`, not as a single nested
    blob. This matches `extra_body` storage shape under canonical
    provider namespaces and makes the `extra_body_hash` namespace
    exclusion (invariant 28) operate at the outer key granularity.
34. The `PromptHashInputs → PromptStateInput` merge MUST apply the
    type-shape conversions in §11 (`Option<u64>` → `u64` via
    `unwrap_or(0)`; `Vec<(String, u64)>` → `HashMap`). The new fields
    `developer_text_hash`, `contextual_user_text_hash`,
    `contextual_user_char_count` are added to `PromptStateInput`;
    `cache_detection.rs` state schema version bumps on this change.
35. `coco-context` gains a direct `coco-inference` dependency edge to
    construct `PromptEnvelope` from builders. Cycle-free because
    `coco-messages` already deps `coco-inference`.
