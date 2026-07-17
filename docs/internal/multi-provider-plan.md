# Multi-LLM Provider Plan

> Status: Updated 2026-04-26 (rewrite + Rust-invariants pass — three-layer boundary model with type-level guarantees)
> Scope: `coco-rs/common/config/`, `coco-rs/services/inference/`, `coco-rs/app/cli/src/model_factory.rs`, `coco-rs/core/tool-runtime/src/context.rs`, `coco-rs/vercel-ai/{anthropic,openai,google,openai-compatible}/`
> Owners: coco-config + coco-inference + cli model_factory + each vercel-ai provider crate

> **Source of truth.** Enum and struct definitions live in `crate-coco-types.md` (enums) and `crate-coco-config.md` (structs). Each `vercel-ai-*` crate's `CLAUDE.md` is authoritative for its provider implementation. This doc covers cross-cutting flow only and does not redefine those types.
>
> **Current vs Target.** Items marked **🎯 Target** describe the intended design; the current code does not yet implement them. Items marked **✅ Implemented** are reflected in code today. Search for these markers when porting.

> **Rust invariants.** Type-level guarantees that the implementation must enforce are listed in §15. Each invariant has at least one corresponding test. Reviewers: any change here that loosens an invariant must update §15 first.

## 1. Goals

The architecture is shaped by three goals, in priority order:

1. **Multi-provider** — Support Anthropic, OpenAI (Chat + Responses), Gemini, Volcengine, Z.AI, and arbitrary OpenAI-compatible gateways via the `vercel-ai-*` SDKs. Provider-specific concerns (auth, base URL, headers, beta headers, rate limit messaging, OAuth) live in their respective `vercel-ai-*` crates — `services/inference` is generic.
2. **Per-(provider, model) configuration** — Each provider declares which models it serves; per-(provider, model) routing (`api_model_name`), `ToolOverrides`, and provider-shaped request extras attach at that level. Built-in `ModelInfo` defaults layer underneath.
3. **User config never reverse-perceives provider namespace** — `ModelInfo` carries a flat `extra_body: BTreeMap<String, JSONValue>` that is provider-agnostic. The Coco boundary layer (model_factory + call-options builder) translates that to the `vercel-ai` SDK's namespaced `ProviderOptions[<provider_name>]` shape. Users never write a provider key in their config.

## 2. The Three-Layer Boundary Model

Vercel AI SDK v4 (both TS upstream and the Rust port) namespaces per-call provider options by **provider instance name** — each language model implementation reverse-perceives its own namespace key (`AnthropicMessagesLanguageModel` reads `provider_options["anthropic"]`, `GoogleGenerativeAILanguageModel` reads `["google"]` or `["vertex"]`, OpenAI-compatible reads its configured `provider_options_name`). This is **upstream v4 spec** (`@ai-sdk/provider/src/shared/v4/shared-v4-provider-options.ts:13-24`) and cannot be changed without forking the SDK.

That namespace is a **transport contract**, not a user concept. We isolate it inside Coco's boundary layer:

```
┌─────────────────────────────────────────────────────────────────────┐
│ Layer 1 — User-facing config (provider-agnostic)                     │
│   What this model is and what wire body it wants                     │
│                                                                       │
│   ModelInfo {                                                         │
│     model_id,                                                         │
│     context_window: PositiveTokens, max_output_tokens: PositiveTokens,│
│     temperature: Option<f32>,            // Lane A typed              │
│     ...                                                               │
│     extra_body: BTreeMap<String, JSONValue>,  // flat camelCase keys  │
│     tool_overrides: Option<ToolOverrides>, capabilities, ...          │
│   }                                                                   │
│                                                                       │
│   ProviderConfig {                                                    │
│     name,                                  // = parent map key        │
│     api: ProviderApi, base_url, env_key,                              │
│     api_key: Option<RedactedSecret>,                                  │
│     client_options: ProviderClientOptions, // typed, not loose map    │
│     models: BTreeMap<String, ProviderModelOverride>,                  │
│   }                                                                   │
└──────────────────────┬──────────────────────────────────────────────┘
                       │  build_call_options(model_info, provider_name, …)
                       │  build_language_model_from_runtime(provider_cfg, …)
                       ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Layer 2 — Coco boundary (translates to vercel-ai contract)           │
│   Knows about provider namespacing; users do not                     │
│                                                                       │
│   call_options.temperature       = info.temperature                  │
│   call_options.max_output_tokens = Some(info.max_output_tokens.into())│
│   call_options.provider_options  = {                                 │
│     let mut po = ProviderOptions::default();                         │
│     po.set(&provider_cfg.name, info.extra_body.clone());             │
│     Some(po)                                                          │
│   };                                                                  │
└──────────────────────┬──────────────────────────────────────────────┘
                       │  language_model.do_generate(call_options)
                       ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Layer 3 — vercel-ai provider (upstream contract, untouched)          │
│   provider_options[ self.provider() ]                                │
│       │                                                               │
│       ├── typed-known keys  → serde to TypedProviderOptions struct   │
│       │     → structured wire-body mapping                           │
│       │     (e.g. Anthropic.thinking → body["thinking"] = json!{…})  │
│       │                                                               │
│       └── leftover keys     → shallow-merge into wire body           │
│             (uniform across all providers — see §7.3)                │
└─────────────────────────────────────────────────────────────────────┘
```

The three layers are decoupled:

- **Layer 1 is provider-portable.** A `ModelInfo` for `gpt-5` is identical whether served by `openai-direct`, `azure-openai-east`, or `internal-gateway`. Users move models between providers by changing role bindings, not by rewriting model entries.
- **Layer 2 owns namespace translation.** It reads `ProviderConfig.name` (the runtime instance identifier) and wraps Layer 1's flat `extra_body` into the `provider_options[<name>]` key. Users never type that name in a `ModelInfo`.
- **Layer 3 is upstream.** Each `vercel-ai-*` crate implements its own typed-known + leftover convention; we do **not** modify the upstream call-options shape.

### 2.1 Why namespace stays inside Layer 2 — not exposed to users

| Property | Outcome |
|---|---|
| User config simpler | One mental model: "this model wants these wire fields." No "anthropic" / "openai" / "azure-east" key ceremony. |
| Model-portable | Switching `gpt-5`'s Main role from `openai` to `azure-east` does not edit `models.json`. |
| Round-trip safety | `ProviderConfig.name` = parent map key by construction (§5.1.1); set in `from_partial`, used in `build_call_options`. The legacy "provider name backfill" hack in `runtime.rs:185-187` is deleted because the divergence it guarded against is now unrepresentable. |
| Type clarity | `ModelInfo.extra_body: BTreeMap<String, JSONValue>` is one level deep. `LanguageModelV4CallOptions.provider_options: ProviderOptions(HashMap<String, HashMap<String, JSONValue>>)` is two levels deep. The shapes are deliberately different. |
| Upstream parity preserved | We can drop in the latest `@ai-sdk/provider` v4 spec without changing config schema. |

### 2.2 The `ProviderApi` enum is wire-protocol family, not instance identifier

Two distinct concepts must not be confused:

| Concept | Type | Cardinality | Owner | Used for |
|---|---|---|---|---|
| **Wire-protocol family** | `ProviderApi` enum (`Anthropic`, `Openai`, `Gemini`, `OpenaiCompat`, `Volcengine`, `Zai`) | Closed set, defined in `coco-types` | `crate-coco-types.md` | `model_factory` dispatch — picks which `vercel-ai-*` crate to instantiate |
| **Provider instance identifier** | `String` (e.g. `"anthropic"`, `"azure-east"`, `"internal-router"`) | Open set, user-extensible | `ProviderConfig.name` field | `provider_options` namespace key in vercel-ai contract; `model.provider()` return value |

A single `ProviderApi::OpenaiCompat` may back N instance identifiers (`xai`, `groq`, `azure-east`, `internal-router`, …). A single instance identifier always maps to exactly one `ProviderApi`. The vercel-ai outer namespace key is `String` (not `ProviderApi`) because the 1-to-N relation between API and instance is the explicit reason the spec admits arbitrary names.

## 3. Architecture Overview

```
                  ~/.coco/  (settings.json + providers.json + models.json)
          ┌───────────────────────────────────────────────────┐
          │  settings.json    role bindings, features         │ 6-source layered
          │  providers.json   provider catalog                 │ single-source
          │  models.json      model catalog (provider-agnostic)│ single-source
          └────────────────────────┬──────────────────────────┘
                                   ▼ build_runtime_config()
          ┌───────────────────────────────────────────────────┐
          │              RuntimeConfig (Layer 1 snapshot)      │
          │  providers      : BTreeMap<String, ProviderConfig> │
          │  model_roles    : ModelRoles                       │
          │  features       : Features                         │
          │  model_registry : Arc<ModelRegistry>          🎯  │
          │  tool_overrides : Arc<ToolOverrides>          🎯  │
          └────────────────────────┬──────────────────────────┘
                                   │ Layer 2 boundary
                ┌──────────────────┼──────────────────┐
                ▼                  ▼                  ▼
       ┌───────────────┐ ┌────────────────┐ ┌────────────────────┐
       │ model_factory │ │ build_call_   │ │ ToolUseContext      │
       │ Lane C build  │ │  options(...)  │ │ tool filter Layer 2 │
       │ vercel-ai     │ │  Lane A typed +│ │ uses tool_overrides │
       │ provider +    │ │  Lane B wrap   │ │                     │
       │ language_model│ │  extra_body    │ │                     │
       └───────┬───────┘ └────────┬───────┘ └────────────────────┘
               │                  │
               ▼                  ▼
       Arc<dyn LanguageModelV4>   LanguageModelV4CallOptions
                       │              │
                       └──────┬───────┘
                              ▼
                 model.do_generate(call_options)   ← Layer 3 (upstream)
```

### 3.1 Model Roles And Subagent Precedence

The canonical closed set is `coco_types::ModelRole`, documented in
`crate-coco-types.md`: `Main`, `Fast`, `Plan`, `Explore`, `Review`,
`Subagent`, `Memory`, and `HookAgent`. There is no `Compact` role in the
current enum; compaction uses the compact service/config path and fallback
rules rather than `ModelRole::Compact`.

`ModelRole::Subagent` is the default LLM role for generic/custom subagent
execution. It does not replace the narrower built-in subagent roles. Spawn-time
selection follows this order:

1. explicit model override: AgentTool request `model` > agent definition
   `model`;
2. explicit role override: `AgentDefinition.model_role`;
3. built-in subagent mapping: `Explore` -> `Explore`, `Plan` -> `Plan`,
   `Verification` -> `Review`;
4. generic built-ins (`GeneralPurpose`, `StatusLine`, `CocoGuide`), custom
   agents, and missing type -> `Subagent`.

The concrete model override and semantic model role are independent. A subagent
can pin a concrete model while still carrying a role for fallback, recovery
policy, and telemetry.

## 4. Configuration File Shape

Three fixed-name sibling files under `~/.coco/`:

| File | Owns | Mutability | Layered with `.claude/` overlay? |
|---|---|---|---|
| `settings.json` | Role bindings, features, hooks, permissions, user preferences. Optional partial overrides for provider entries. | per-user, per-project (`.claude/settings.{json,local.json}`) | ✅ 6-source `SettingsWithSource` |
| `providers.json` | Provider catalog — `api`, `base_url`, `env_key`, `wire_api`, `client_options`, plus the per-(provider, model) entries (`api_model_name`, `tool_overrides`, per-entry sampling overrides) | per-machine; team / org-shareable | ❌ single-source |
| `models.json` | ModelInfo catalog — provider-agnostic metadata (context_window, capabilities, thinking levels, default tool_overrides, base instructions, **extra_body**) | per-machine; community-shareable | ❌ single-source |

Both `providers.json` and `models.json` are **optional**. When absent, only the compiled-in builtin registries are used. When present they layer between builtin and any per-user `settings.json` overrides.

**Design intent.** Switching between configurations (work / personal / dev / prod) only edits `settings.json`. The shared `providers.json` and `models.json` carry stable infrastructure that is the same across configurations.

### 4.1 Minimal example

```jsonc
// ~/.coco/settings.json
{
  "models": {
    "main": "anthropic/claude-sonnet-4-6",
    "fast": "anthropic/claude-haiku-4-5"
  }
}
```

The builtin `providers` registry covers Anthropic / OpenAI / Gemini / Volcengine / Z.AI defaults; the builtin `models` registry covers well-known models. `ANTHROPIC_API_KEY` env var is read by the builtin `anthropic` provider.

### 4.2 Shared catalogs (recommended team / org pattern)

```jsonc
// ~/.coco/providers.json — shared provider catalog
{
  "anthropic-corp": {
    "api": "anthropic",
    "base_url": "https://corp-proxy.example.com/anthropic",
    "env_key": "CORP_ANTHROPIC_KEY",
    "client_options": { "headers": { "X-Corp-Tenant": "engineering" } },
    "models": {
      "claude-sonnet-4-6": {},
      "claude-opus-4-7":   { "max_output_tokens": 64000 }
    }
  },
  "azure-openai-east": {
    "api": "openai",                 // ← Responses API path; see §6.2
    "base_url": "https://my-azure.openai.azure.com/openai/deployments/gpt-5",
    "env_key": "AZURE_OPENAI_KEY",
    "client_options": {
      "headers": { "api-version": "2024-12-01-preview" }
    },
    "models": {
      "gpt-5": { "api_model_name": "gpt-5" }
    }
  },
  "internal-router": {
    "api": "openai_compat",
    "base_url": "https://internal/v1",
    "env_key": "INTERNAL_KEY",
    "models": {
      "internal/coder-v3": { "api_model_name": "ep-internal-v3-prod" }
    }
  }
}
```

```jsonc
// ~/.coco/models.json — provider-agnostic ModelInfo catalog
{
  "claude-opus-4-7": {
    "context_window": 200000,
    "max_output_tokens": 64000,
    "capabilities": ["tool_calling", "vision", "extended_thinking", "fast_mode"],
    "supported_thinking_levels": [
      { "effort": "low" }, { "effort": "medium" },
      { "effort": "high",  "options": { "interleaved": true } },
      { "effort": "xhigh", "budget_tokens": 128000, "options": { "interleaved": true } }
    ],
    "default_thinking_level": "medium",
    "extra_body": {
      "cacheControl": { "type": "ephemeral" }    // ← Layer 1 sees flat keys; Layer 2 wraps
    }
  },
  "gpt-5": {
    "context_window": 272000,
    "max_output_tokens": 16384,
    "apply_patch_tool_type": "shell",
    "tool_overrides": { "extra": ["apply_patch"], "excluded": ["edit"] },
    "extra_body": {
      // Casing rule: extra_body keys are the camelCase form expected by the
      // provider's typed parser (Layer 3 reads `#[serde(rename_all = "camelCase")]`
      // on AnthropicProviderOptions / OpenAIResponsesProviderOptions / etc.).
      // snake_case keys are NOT silently re-cased — they fall through to
      // leftover-merge as raw wire-body fields and may be ignored by the API.
      "store": false,
      "reasoningSummary": "auto"
    }
  },
  "internal/coder-v3": {
    "context_window": 128000,
    "max_output_tokens": 8192,
    "capabilities": ["tool_calling", "vision"],
    "apply_patch_tool_type": "shell",
    "base_instructions_file": "prompts/coder-v3.md"
  }
}
```

```jsonc
// ~/.coco/settings.json — switches configurations
{
  "models": {
    "main":    "anthropic-corp/claude-opus-4-7",
    "fast":    "anthropic-corp/claude-sonnet-4-6",
    "explore": "internal-router/internal/coder-v3"
  },
  "features": { "web_search": true }
}
```

Switching `gpt-5` from `openai-direct` to `azure-openai-east` is **a one-line role binding change**. `models.json` is untouched. Layer 2 wraps `extra_body` under whichever provider name the role resolves to.

### 4.3 Per-user overlay into the catalog

Settings.json can override individual fields on any provider entry without forking the catalog:

```jsonc
// ~/.coco/settings.json (excerpt)
{
  "providers": {
    "openai": {
      "client_options": { "organization_id": "org-myown" }
    }
  }
}
```

Settings.json values override `providers.json` values key-by-key. Unset fields in the overlay leave catalog values intact. See §5.1 for the merge semantics — overlays use `PartialProviderConfig` (every field `Option<_>`) so the overlay never silently coerces a non-set field to a default.

### 4.4 Anti-pattern: writing `provider_options` directly in user config

Earlier drafts allowed users to write the vercel-ai namespace verbatim:

```jsonc
// ❌ DON'T DO THIS in models.json or settings.json
{
  "models": {
    "gpt-5": {
      "provider_options": {
        "openai": { "store": false }    // namespace key leaks into Layer 1
      }
    }
  }
}
```

This couples the model entry to a specific provider instance. If the user reroutes `gpt-5` from `openai` to `azure-east`, the `"openai"` key becomes silently inert — no error, just no effect. The flat `extra_body` form has no such failure mode.

## 5. Resolution Pipeline

```
settings.json + env + CLI    ~/.coco/providers.json   ~/.coco/models.json
       │                          │                        │
       ▼ load_settings()          ▼ deserialize as         ▼ deserialize as
SettingsWithSource          HashMap<String,           HashMap<String,
(6 sources)               PartialProviderConfig>     PartialModelInfo>
       │                          │                        │
       └──────────────────────────┴────────┬───────────────┘
                                           ▼ build_runtime_config()
RuntimeConfig {
   providers:      ← builtin ⊕ providers.json ⊕ settings.providers (3-layer merge)
   model_roles:    ← settings.models.{main,fast,…} → ModelSpec[]
   features:       ← apply_map(settings.features)
   model_registry: ← three-layer ModelInfo merge per (provider, model_id)
   tool_overrides: ← from main role's ResolvedModel, Arc-wrapped
}
```

### 5.1 Provider resolution — three layers, Partial overlays

🎯 **Target.** Current code uses non-`Option` `ProviderConfig.api` and unconditional overwrite (see `provider/mod.rs:62`). The fix splits the on-disk overlay shape from the resolved shape, and elevates four Rust invariants into the type system: (a) identity is the map key, never duplicated; (b) API keys cannot leak via `Debug`; (c) `client_options` is typed, not a loose map; (d) on-disk maps preserve diff order.

```rust
// Wire format — every field optional so omission means "inherit".
// `BTreeMap` (NOT `HashMap`) so serialised output, snapshots, and review diffs are stable.
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialProviderConfig {
    // NOTE: identity is the map key in the parent `BTreeMap<String, PartialProviderConfig>`.
    // There is intentionally no `name` field here — see §5.1.1 (Identity invariant).
    pub api:            Option<ProviderApi>,
    pub env_key:        Option<String>,
    pub api_key:        Option<RedactedSecret>,    // redacted Debug; see §5.1.2
    pub base_url:       Option<String>,
    pub default_model:  Option<String>,
    pub timeout_secs:   Option<i64>,
    pub streaming:      Option<bool>,
    pub wire_api:       Option<WireApi>,
    pub client_options: Option<PartialProviderClientOptions>, // typed; see §5.1.3
    pub models:         Option<BTreeMap<String, PartialProviderModelOverride>>,
}

// Custom Debug — never prints api_key.
impl fmt::Debug for PartialProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PartialProviderConfig")
            .field("api", &self.api)
            .field("env_key", &self.env_key)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            // ... remaining fields ...
            .finish()
    }
}

// Resolved form — required fields concrete; only genuinely-optional fields stay Option.
#[derive(Clone)]   // NOTE: no `Debug` derive — see custom impl below.
pub struct ProviderConfig {
    pub name:           String,                       // = parent map key (§5.1.1)
    pub api:            ProviderApi,
    pub env_key:        String,
    pub api_key:        Option<RedactedSecret>,       // redacted Debug
    pub base_url:       String,
    pub default_model:  Option<String>,
    pub timeout_secs:   i64,
    pub streaming:      bool,
    pub wire_api:       WireApi,
    pub client_options: ProviderClientOptions,        // typed
    pub models:         BTreeMap<String, ProviderModelOverride>,
}

impl fmt::Debug for ProviderConfig { /* same redaction as above */ }

fn resolve_providers(
    settings:     &Settings,
    file_catalog: &BTreeMap<String, PartialProviderConfig>,  // ~/.coco/providers.json
) -> Result<BTreeMap<String, ProviderConfig>, ConfigError> {
    let mut result = builtin_providers();                    // L0: ProviderConfig (resolved)
    apply_partial_layer(&mut result, file_catalog)?;         // L1
    apply_partial_layer(&mut result, &settings.providers)?;  // L2
    Ok(result)
}

fn apply_partial_layer(
    base:    &mut BTreeMap<String, ProviderConfig>,
    overlay: &BTreeMap<String, PartialProviderConfig>,
) -> Result<(), ConfigError> {
    for (name, partial) in overlay {
        match base.entry(name.clone()) {
            Entry::Occupied(mut e) => e.get_mut().merge_partial(partial),
            Entry::Vacant(e) => {
                let resolved = ProviderConfig::from_partial(name, partial)?;
                e.insert(resolved);
            }
        }
    }
    Ok(())
}
```

Two distinct merge paths:

| Case | Behavior |
|---|---|
| Overlay matches an existing entry (builtin or earlier-layer) | `merge_partial`: each `Some` field wins; each `None` field keeps the base value. **`api` is never silently coerced.** |
| Overlay declares a new provider | `from_partial(map_key, partial) -> Result<ProviderConfig, ConfigError>`: required fields (`api`, `env_key`, `base_url`) must be `Some(_)` or returns `ConfigError::IncompleteProviderEntry { name }`. **`name` is set from `map_key`, never from the overlay** (§5.1.1). |

This eliminates the api-coercion failure mode — a partial overlay (`{"openai": { "client_options": { … } } }`) cannot accidentally overwrite `api` to `ProviderApi::Anthropic` via serde default.

#### 5.1.1 Identity invariant — map key is the only source

`PartialProviderConfig` deliberately has **no `name` field**. The provider instance identifier is the parent `BTreeMap` key, full stop. `from_partial(map_key, partial)` writes `resolved.name = map_key.to_string()` and `merge_partial` never touches `.name`. Result:

```rust
// Compile-time guarantee: `name` cannot diverge from map key.
//   - User cannot write `{"openai-direct": {"name": "azure-east", ...}}`
//     because `serde(deny_unknown_fields)` rejects the `name` key at parse time.
//   - In-memory `cfg.name` is set in exactly one place (`from_partial`).
//   - No release-vs-debug skew: the `debug_assert_eq!` from the prior draft
//     is gone because the divergence it guarded against is now unrepresentable.
```

`ProviderConfig.name` is the **single source for the vercel-ai `provider_options` outer namespace key** (§7.2). The map key, the `.name` field, and the runtime `model.provider()` string agree by construction.

#### 5.1.2 `RedactedSecret` — type-level Debug guard

```rust
/// Secret string that never round-trips through `Debug` / `Display` / `format!`.
/// Use `.expose()` at the single call-site that builds the auth header.
#[derive(Clone, Deserialize, Serialize)]
#[serde(transparent)]
pub struct RedactedSecret(String);

impl RedactedSecret {
    pub fn expose(&self) -> &str { &self.0 }
}

impl fmt::Debug for RedactedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedactedSecret(<redacted>)")
    }
}

impl fmt::Display for RedactedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}
```

Why a newtype, not just a string redaction in `ProviderConfig::Debug`:

- **Defence-in-depth.** `tracing::error!("{cfg:?}")` is only one of many leak surfaces. snafu cause chains, `.expect("config: {cfg:?}")`, `serde_json::to_string(&cfg).unwrap()` (we don't `Serialize` the struct directly, but `RedactedSecret` `Serialize` writes the raw string — only the auth-header writer ever serialises a single field with `expose()`), test assertion failures with `assert_eq!` formatters — all of those go through `Debug`/`Display`/`expose`.
- **Single audit point.** `grep -r expose\(\) coco-rs/` enumerates every place a secret leaves the type. Today that should be 1-2 sites in `model_factory.rs`.
- **Decouples from `secret-redact`.** `coco-utils/secret-redact` is a string-pattern post-processor at log sinks; it cannot catch a secret embedded in a non-string assertion message or a panic backtrace.

#### 5.1.3 `ProviderClientOptions` — typed, not a loose map

```rust
/// Wire format — `BTreeMap`-ordered, every field `Option`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "snake_case")]
pub struct PartialProviderClientOptions {
    pub headers:                       Option<BTreeMap<String, String>>,
    pub auth_token:                    Option<RedactedSecret>,
    pub organization_id:               Option<String>,
    pub project_id:                    Option<String>,
    pub include_usage:                 Option<bool>,
    pub full_url:                      Option<bool>,
    pub supports_structured_outputs:   Option<bool>,
}

#[derive(Clone, Default)]
pub struct ProviderClientOptions {
    pub headers:                       BTreeMap<String, String>,
    pub auth_token:                    Option<RedactedSecret>,
    pub organization_id:               Option<String>,
    pub project_id:                    Option<String>,
    pub include_usage:                 Option<bool>,         // None = SDK default (false)
    pub full_url:                      bool,                  // default false
    pub supports_structured_outputs:   bool,                  // default false
}
impl fmt::Debug for ProviderClientOptions { /* redacts auth_token */ }
```

Benefits over the prior `HashMap<String, JSONValue>` shape:

- **Source-span errors.** `serde_json` reports the failing field by name and JSON pointer; we no longer surface a generic "unknown key in client_options" deep into request build.
- **`deny_unknown_fields` actually works.** Previously the deny was applied at `PartialProviderConfig` level only; nested options keys silently flowed through.
- **Exhaustive matching downstream.** `model_factory::build_*` matches on concrete fields, so adding a new client option is a compile error in every provider arm rather than a runtime miss.

True provider pass-through (HTTP headers the user wants on every request, gateway-specific knobs not modelled here) goes through `ModelInfo.extra_body` (Layer 1) or, for headers specifically, through `client_options.headers`. There is no need for a generic "anything goes" map.

### 5.2 ModelRegistry build — three-layer merge with `PartialModelInfo`

🎯 **Target.** Current code uses non-`Option` numeric fields with serde defaults (`context_window = 200_000`, `max_output_tokens = 16_384` — see `model/mod.rs:33-36, 89-95`); missing required metadata is silently masked. The fix is the same Partial-vs-resolved split as §5.1, plus newtype-wrapped positive integers so `as u64` casts cannot underflow.

#### 5.2.1 Bounded-positive integer newtypes

```rust
/// Token-count metadata that must be a positive int. Constructed via `try_from(i64)`.
/// Internal repr is u32 because no production model exceeds 4G tokens; we choose u32
/// over i64 deliberately — every i64 arithmetic site below would otherwise need
/// `try_into::<u64>().expect(...)`. With u32, the `.into()` to u64 is infallible.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PositiveTokens(u32);

impl TryFrom<i64> for PositiveTokens {
    type Error = ConfigError;
    fn try_from(v: i64) -> Result<Self, ConfigError> {
        u32::try_from(v)
            .map(Self)
            .map_err(|_| ConfigError::NonPositiveTokens { value: v })
    }
}
impl<'de> Deserialize<'de> for PositiveTokens { /* via i64 + TryFrom */ }
impl From<PositiveTokens> for u64 { fn from(v: PositiveTokens) -> u64 { v.0 as u64 } }
impl From<PositiveTokens> for i64 { fn from(v: PositiveTokens) -> i64 { v.0 as i64 } }

/// Same shape, used for `top_k` / similar small positive ints.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PositiveCount(u32);
impl TryFrom<i64> for PositiveCount { /* same pattern */ }
```

Why a newtype rather than `u32` directly: JSON callers naturally write `200000` (parses as `i64`), so we keep the wire format `i64` and validate at the type boundary. The closed `u32` repr means downstream `u64` arithmetic is `From`-not-`TryFrom`, eliminating the `as u64` footgun across the entire call chain.

#### 5.2.2 Partial / resolved structs

```rust
// Wire format — Option distinguishes "unset" from "explicitly set".
// `BTreeMap` order is preserved in serialised output; tests / snapshots are stable.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialModelInfo {
    // NOTE: model_id is the parent map key (same identity invariant as §5.1.1).
    pub display_name:              Option<String>,
    pub context_window:            Option<PositiveTokens>,
    pub max_output_tokens:         Option<PositiveTokens>,
    pub timeout_secs:              Option<i64>,
    pub capabilities:              Option<Vec<Capability>>,
    pub temperature:               Option<f32>,
    pub top_p:                     Option<f32>,
    pub top_k:                     Option<PositiveCount>,
    pub supported_thinking_levels: Option<Vec<ThinkingLevel>>,
    pub default_thinking_level:    Option<ReasoningEffort>,
    pub auto_compact_pct:          Option<i32>,
    pub apply_patch_tool_type:     Option<ApplyPatchToolType>,
    pub tool_overrides:            Option<ToolOverrides>,
    pub shell_type:                Option<String>,
    pub max_tool_output_chars:     Option<i32>,
    pub base_instructions:         Option<String>,
    pub base_instructions_file:    Option<String>,
    pub extra_body:                Option<BTreeMap<String, JSONValue>>,
}

// Resolved form — context_window / max_output_tokens are required-and-concrete-positive.
// temperature / top_p / top_k stay Option because None == "let the provider default";
// see §7.1 for why this matters at the wire level.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub model_id:                  String,                  // = parent map key
    pub display_name:              Option<String>,
    pub context_window:            PositiveTokens,
    pub max_output_tokens:         PositiveTokens,
    pub timeout_secs:              Option<i64>,
    pub capabilities:              Option<Vec<Capability>>,
    pub temperature:               Option<f32>,
    pub top_p:                     Option<f32>,
    pub top_k:                     Option<PositiveCount>,
    pub supported_thinking_levels: Option<Vec<ThinkingLevel>>,
    pub default_thinking_level:    Option<ReasoningEffort>,
    pub auto_compact_pct:          Option<i32>,
    pub apply_patch_tool_type:     Option<ApplyPatchToolType>,
    pub tool_overrides:            Option<ToolOverrides>,
    pub shell_type:                Option<String>,
    pub max_tool_output_chars:     Option<i32>,
    pub base_instructions:         Option<String>,
    pub base_instructions_file:    Option<String>,
    /// Layer 1 escape hatch. Provider-agnostic flat keys, **camelCase to match
    /// each provider's typed-options struct rename_all attribute** (§4.2).
    /// Layer 2 wraps as `provider_options[<provider_name>]` at call time.
    pub extra_body:                BTreeMap<String, JSONValue>,
}
```

Build pipeline:

```rust
fn build_model_registry(
    providers:    &BTreeMap<String, ProviderConfig>,
    user_catalog: &BTreeMap<String, PartialModelInfo>,   // ~/.coco/models.json
    coco_home:    &Path,
) -> Result<ModelRegistry, ConfigError> {
    let mut resolved = BTreeMap::new();
    for (provider_name, cfg) in providers {
        for (model_id, entry) in &cfg.models {
            // L0: builtin (already resolved ModelInfo)
            let mut acc: PartialModelInfo = builtin_models()
                .get(model_id)
                .map(PartialModelInfo::from_resolved)
                .unwrap_or_default();

            // L1: user catalog ~/.coco/models.json
            if let Some(user_info) = user_catalog.get(model_id) {
                acc.merge_from(user_info);
            }

            // L2: per-(provider, model) entry overrides
            entry.apply_to(&mut acc);

            // Resolve base_instructions_file under coco_home — propagate IO error
            if let Some(file) = acc.base_instructions_file.take() {
                let path = coco_home.join(&file);
                let content = std::fs::read_to_string(&path).map_err(|source| {
                    ConfigError::BaseInstructionsRead { path: path.clone(), source }
                })?;
                acc.base_instructions = Some(content);
            }

            // Validate — `Option::None` is the "not declared" signal, NOT a 0-default sentinel.
            let info = ModelInfo::from_partial(provider_name, model_id, acc)?;
            resolved.insert(
                (provider_name.clone(), model_id.clone()),
                Arc::new(ResolvedModel { info, provider_model: entry.clone() }),
            );
        }
    }
    Ok(ModelRegistry { resolved })
}

impl ModelInfo {
    fn from_partial(
        provider: &str,
        model_id: &str,
        p: PartialModelInfo,
    ) -> Result<Self, ConfigError> {
        Ok(Self {
            model_id:          model_id.to_string(),
            context_window:    p.context_window
                .ok_or_else(|| ConfigError::MissingContextWindow {
                    provider: provider.into(), model: model_id.into(),
                })?,
            max_output_tokens: p.max_output_tokens
                .ok_or_else(|| ConfigError::MissingMaxOutputTokens {
                    provider: provider.into(), model: model_id.into(),
                })?,
            extra_body:        p.extra_body.unwrap_or_default(),
            // ... pass-through Option fields ...
        })
    }
}
```

A model declared inline in `models.json` without `context_window` now fails resolution with a precise error. A negative `context_window` fails at deserialise time with `ConfigError::NonPositiveTokens { value }`. Neither path can reach the request builder with garbage state.

### 5.3 `tool_overrides` plumbing — closes the L1 dormant gap

🎯 **Target.** `runtime.rs:141-156` currently passes `None` as the second arg to `resolve_tool_overrides` (the `ModelInfo` argument), with the comment "*until that plumbing lands*". With `ModelRegistry`, the lookup succeeds and the override flows through:

```rust
fn resolve_main_tool_overrides(
    roles:    &ModelRoles,
    registry: &ModelRegistry,
) -> Arc<ToolOverrides> {
    let Some(spec) = roles.get(ModelRole::Main) else {
        return Arc::new(ToolOverrides::none());
    };
    let info = registry.resolve(&spec.provider, &spec.model_id).map(|r| &r.info);
    Arc::new(crate::tool_overrides::resolve_tool_overrides(&spec.model_id, info))
}
```

The "until that plumbing lands" comment is removed. The dormant `ProviderInfo` / `ProviderModel` types (currently in `provider/mod.rs:80, 116`) are deleted — they are unused outside `lib.rs` re-exports.

## 6. Layer 2: Provider Construction (Lane C)

`app/cli/src/model_factory.rs::build_language_model_from_runtime` is the **single binding point** between `RuntimeConfig` and `vercel-ai-*` crates. 🎯 **Target** — the current `build_language_model_from_spec` (`model_factory.rs:35-70`) errors out for `Volcengine`/`Zai`/`OpenaiCompat`; the rewrite plumbs `RuntimeConfig` so all six `ProviderApi` variants can be served.

Pipeline:

1. Look up `ProviderConfig` and `ResolvedModel` from `RuntimeConfig`.
2. Resolve `api_model_name` (entry override → fallback to `model_id`).
3. Resolve API key (env var via `provider.env_key` → fallback to `provider.api_key`).
4. Build a shared `reqwest::Client` honouring `provider.timeout_secs`.
5. Extract Lane C client-construction keys from `client_options` and pass them to the SDK's `*ProviderSettings`.

### 6.1 client_options keys

Defined in §5.1.3 as the typed `ProviderClientOptions` struct. Each field is opt-in; missing fields mean "SDK default":

| Field | Type | Provider | Effect |
|---|---|---|---|
| `headers` | `BTreeMap<String, String>` | all | Custom HTTP headers (`X-Corp-Tenant`, gateway tracking, …). `BTreeMap` so order is stable across restarts. |
| `auth_token` | `RedactedSecret` | Anthropic | Bearer token; if set, `api_key` is ignored. Redacted Debug. |
| `organization_id` | `String` | OpenAI | Sent as `OpenAI-Organization` |
| `project_id` | `String` | OpenAI | Sent as `OpenAI-Project` |
| `include_usage` | `Option<bool>` | OpenAI-compat | Request `stream_options.include_usage`. `None` matches the SDK's `false` default at `openai_compatible_provider.rs:98`. |
| `full_url` | `bool` | OpenAI-compat | Treat `base_url` as the complete endpoint (skip `/v1/chat/completions` suffix). For Azure-style routing where the path includes deployment + api-version. |
| `supports_structured_outputs` | `bool` | OpenAI-compat | Enables `response_format = json_schema` shaping |

Adding a new option = adding a field. `serde(deny_unknown_fields)` rejects misspellings at parse time with a JSON-pointer error message; `model_factory` arms cannot compile if a new field is added without being threaded through to every provider builder that uses it.

### 6.2 Wire-API selection (Azure / Responses / Chat)

Azure OpenAI needs special care. The vercel-ai `OpenAICompatibleProvider::language_model()` always returns the Chat Completions model (`openai_compatible_provider.rs:158-161`); pointing its `base_url` at a `/responses` endpoint produces 404.

Two viable Azure setups:

**(a) Azure via OpenAI direct (Responses API).** Recommended for newer Azure deployments. Uses `vercel-ai-openai`'s `provider.language_model()` which defaults to Responses.
```jsonc
{
  "azure-openai-east": {
    "api": "openai",                                     // ← NOT openai_compat
    "wire_api": "responses",                             // explicit, default for openai
    "base_url": "https://my-azure.openai.azure.com/openai/deployments/gpt-5",
    "client_options": {
      "headers": { "api-version": "2024-12-01-preview" }
    }
  }
}
```

**(b) Azure via OpenAI-compat (Chat Completions).** For older deployments without Responses support.
```jsonc
{
  "azure-openai-east": {
    "api": "openai_compat",
    "wire_api": "chat",
    "base_url": "https://my-azure.openai.azure.com/openai/deployments/gpt-5/chat/completions?api-version=2024-08-01-preview",
    "client_options": { "full_url": true }
  }
}
```

The mismatch (Responses endpoint + chat-shaped body) is **not configurable** via OpenAI-compat today; if Responses is required, use option (a).

### 6.3 Sketch

```rust
pub fn build_language_model_from_runtime(
    runtime: &RuntimeConfig,
    spec:    &ModelSpec,
) -> Result<Arc<dyn LanguageModelV4>, ConfigError> {
    let provider_cfg = runtime.providers.get(&spec.provider)
        .ok_or_else(|| ConfigError::UnknownProvider { name: spec.provider.clone() })?;
    let resolved = runtime.model_registry.resolve(&spec.provider, &spec.model_id)
        .ok_or_else(|| ConfigError::UnknownModel {
            provider: spec.provider.clone(), model: spec.model_id.clone(),
        })?;
    let api_model = resolved.provider_model.api_model_name
        .as_deref().unwrap_or(&spec.model_id);
    let api_key = provider_cfg.resolve_api_key()?;     // Result; missing key fails here
    let client  = build_http_client(provider_cfg.timeout_secs);

    // No conversion step needed — `provider_cfg.client_options` is already typed (§5.1.3).
    let opts = &provider_cfg.client_options;

    match spec.api {
        ProviderApi::Anthropic   => build_anthropic(provider_cfg, api_model, api_key, client, opts),
        ProviderApi::Openai      => build_openai(provider_cfg, api_model, api_key, client, opts),
        ProviderApi::Gemini      => build_google(provider_cfg, api_model, api_key, client, opts),
        ProviderApi::Volcengine
        | ProviderApi::Zai
        | ProviderApi::OpenaiCompat => build_openai_compat(provider_cfg, api_model, api_key, client, opts),
    }
}
```

The previous draft had a `ClientOptionsView::extract(&loose_map)?` step. With §5.1.3's typed `ProviderClientOptions`, that view is gone — `serde` validation happened at config-load time, not at request time. Each `build_*` arm matches on `opts.full_url`, `opts.headers`, `opts.organization_id` directly; the compiler enforces exhaustive treatment when fields are added.

**Identity returned by the SDK.** Each provider builder passes `provider_cfg.name.clone()` into `*ProviderSettings.provider_id` (Anthropic/OpenAI: hardcoded "anthropic"/"openai" by the SDK; OpenAI-compat: pass-through; Google: hardcoded). When the model later answers `model.provider()`, the string equals `provider_cfg.name` — closing the namespace round-trip (§7.2).

## 7. Layer 2: Per-Request Building (Lanes A and B)

`services/inference/src/build_call_options.rs::build_call_options` constructs a fresh `LanguageModelV4CallOptions` per turn. This is the boundary where Layer 1's flat `extra_body` is wrapped under `ProviderConfig.name` for Layer 3.

```rust
pub fn build_call_options(
    info:          &ModelInfo,            // already merged through L0/L1/L2
    provider_name: &str,                  // = ProviderConfig.name (single source of truth)
    per_call:      &PerCallOverrides,     // CLI/TUI runtime overrides for this turn
    prompt:        LlmPrompt,
    tools:         Option<Vec<LanguageModelV4Tool>>,
) -> LanguageModelV4CallOptions {
    let mut call = LanguageModelV4CallOptions::new(prompt);
    call.tools = tools;

    // Lane A: typed sampling. None == let provider default.
    // PositiveTokens / PositiveCount → u64 is `From`-infallible (§5.2.1) — no `as u64` casts.
    call.temperature       = per_call.temperature.or(info.temperature);
    call.top_p             = per_call.top_p     .or(info.top_p);
    call.top_k             = per_call.top_k     .or(info.top_k).map(u64::from);
    call.max_output_tokens = per_call.max_output_tokens
                                .or(Some(info.max_output_tokens))
                                .map(u64::from);

    // Lane A2: typed reasoning channel.
    let thinking = per_call.thinking_level.as_ref().or(info.default_thinking());
    if let Some(t) = thinking {
        call.reasoning = Some(t.effort.into());
    }

    // Lane B: shallow-merge extra_body and wrap under provider_name.
    // `BTreeMap` preserves key order so test snapshots are stable.
    let mut extra: BTreeMap<String, JSONValue> = info.extra_body.clone();
    for (k, v) in &per_call.extra_body {
        extra.insert(k.clone(), v.clone());      // per-call wins over model
    }
    if let Some(t) = thinking {
        // thinking_convert produces flat camelCase keys (§7.4) which match the
        // typed-options structs at Layer 3 — same shape as user-supplied extra_body.
        for (k, v) in thinking_convert::to_extra_body(t, provider_name) {
            extra.insert(k, v);
        }
    }
    if !extra.is_empty() {
        let mut po = ProviderOptions::default();
        po.set(provider_name, extra);            // ← namespace wrap happens HERE, only HERE
        call.provider_options = Some(po);
    }

    call
}
```

There is exactly **one** place in the entire Coco codebase that writes a key into `ProviderOptions.0`: this function. Every other place reads `ModelInfo.extra_body`.

### 7.1 Why typed sampling fields keep `Option<T>`

`temperature`, `top_p`, `top_k` stay `Option`. `None` carries semantic meaning at the wire level: every provider's body builder uses `if let Some(v) = call.temperature { body["temperature"] = json!(v); }`. A `None` simply omits the field, letting the provider apply its own default. Forcing concrete defaults would silently override every provider's tuned default, which is the opposite of what users expect.

`context_window` and `max_output_tokens` are different: every model has one (it is metadata, not request-time tuning). They are concrete in resolved `ModelInfo`, and required during `PartialModelInfo → ModelInfo` validation.

### 7.2 Why namespace wrapping uses `ProviderConfig.name`

The vercel-ai contract says: **the language model implementation reads `provider_options[ self.provider() ]`.** `self.provider()` returns the runtime instance identifier, which (for our provider instances) is `ProviderConfig.name`. By using `provider_name = ProviderConfig.name` as the wrap key in Layer 2, we guarantee the namespace round-trips:

```
settings.providers.<KEY>      → ProviderConfig.name = <KEY>
                              → call.provider_options[<KEY>]
                              → model.provider() returns <KEY>
                              → reads its own namespace ✓
```

The legacy "provider-name backfill" concern from earlier reviews becomes unrepresentable: identity is the parent map key, set in `from_partial` exactly once (§5.1.1). User config never types this key.

### 7.3 What Layer 3 does with the extras (uniform leftover-merge convention)

Each `vercel-ai-*` provider extracts `provider_options[ self.provider() ]` and parses it through serde into a typed `*ProviderOptions` struct (`AnthropicProviderOptions`, `OpenAIChatProviderOptions`, `GoogleLanguageModelOptions`). Today the typed parse is **strict** for Anthropic / OpenAI / Google — unknown keys are silently dropped. OpenAI-compat is the exception: `openai_compatible_chat_options.rs:74-82` retains all non-schema keys as a `passthrough` map and merges them into the wire body at `openai_compatible_chat_language_model.rs:194-199`.

🎯 **Target.** Generalise OpenAI-compat's leftover-merge to Anthropic, OpenAI Chat / Responses, and Google. Each provider's `extract_*_options` returns `(TypedOptions, BTreeMap<String, JSONValue>)` (BTreeMap so wire-body field order in tests/insta snapshots is stable); each `get_args` ends with a 5-line shallow-merge:

```rust
// Inserted at the end of get_args, after typed body is built.
if let Some(obj) = body.as_object_mut() {
    for (k, v) in &leftover {
        obj.insert(k.clone(), v.clone());     // leftover overrides typed
    }
}
```

After this generalisation:

| Key in `extra_body` | Outcome |
|---|---|
| Matches a typed-known key (e.g. Anthropic `thinking`, OpenAI `service_tier`, Google `safetySettings`) | serde deserialises, provider-specific structured wire mapping (e.g. `body["thinking"] = json!({"type":"enabled", ...})`) |
| Does not match any typed key | shallow-merged into the wire body root; overrides any earlier typed write at the same key |

This unifies what was previously a Lane A/B isolation inconsistency (broken in OpenAI-compat, intact elsewhere) into a single rule: **`extra_body` keys are uniform across providers; the provider chooses the wire-level shape if it knows the key, otherwise the key lands raw at the top of the wire body**.

Note: the leftover-merge happens **after** typed writes, so a leftover key wins over the typed lane at the wire level. This is intentional — `extra_body` is the user's escape hatch for "I want this exact wire field." The pitfall of putting `temperature` in `extra_body` is now well-defined: it wins, and the user gets the wire-body field they asked for. (Setting both `info.temperature = Some(0.7)` and `info.extra_body["temperature"] = 0.5` is user error; we document but do not police it.)

### 7.4 `thinking_convert::to_extra_body` and the camelCase rule

Typed reasoning collapses into flat `extra_body` keys that Layer 3's typed parser will pick up. **All keys are camelCase**, matching the `#[serde(rename_all = "camelCase")]` attribute on every provider's typed-options struct (`AnthropicProviderOptions`, `OpenAIChatProviderOptions`, `OpenAIResponsesProviderOptions`, `GoogleLanguageModelOptions`):

| Provider | `extra_body` keys produced |
|---|---|
| Anthropic | `{ "thinking": { "type": "enabled", "budgetTokens": <n> } }` |
| OpenAI Responses | `{ "reasoningSummary": "auto" }` and `{ "include": ["reasoning.encrypted_content"] }` |
| Google | `{ "thinkingConfig": { "includeThoughts": true, "thinkingBudget": <n> } }` |
| OpenAI-compat (xAI / DeepSeek) | `{ "reasoningEffort": "high" }` |

`thinking_convert` is provider-aware (it takes `provider_name`), but its output is provider-neutral flat keys — the same shape a user would write directly into `models.json::extra_body`. There is no separate code path for "typed thinking" vs "user extras"; they share the merge.

**The casing rule (one sentence):** `extra_body` keys are the camelCase form expected by the provider's typed-options serde schema. snake_case keys do not get auto-recased — they fall through Layer 3's typed-known check, land in the leftover-merge as raw wire fields, and are likely silently dropped by the upstream API. This rule is uniform across all providers and is enforced in `from_partial` validation only by surface lint (we do not transform; the user gets the wire body they asked for, see §7.3).

## 8. Tool Filter Pipeline (cross-cutting reference)

See `feature-gates-and-tool-filtering.md` for the full 5-layer filter. Multi-provider relevance:

- **Layer 2 (per-model `ToolOverrides`)** is owned by `ModelRegistry`. Two sources merge:
  - `coco_config::tool_overrides::builtin_tool_overrides_for(model_id)` — e.g. `gpt-5*` family → `extra: ["apply_patch"], excluded: ["edit"]`
  - `ResolvedModel.info.tool_overrides` (from `ModelInfo` or per-entry override) — user-side opt-ins
- **`RuntimeConfig.tool_overrides: Arc<ToolOverrides>`** is computed once for the Main role at config-build (§5.3) and threaded through `ToolUseContext.tool_overrides`. Subagent contexts inherit via `Arc::clone` and never widen.

## 9. Builtin Model Registry

`coco_config::builtin::builtin_models() -> &'static BTreeMap<String, ModelInfo>` (lazy `OnceLock`). `BTreeMap` because `tests/snapshots/builtin_models.snap` should not depend on hash randomisation. Initial coverage:

| model_id | Builtin defaults |
|---|---|
| `claude-sonnet-4-6` | context_window: 1_000_000 (with `context-1m` beta), max_output_tokens: 64_000, capabilities: {tool_calling, vision, extended_thinking, fast_mode}, default_thinking_level: medium |
| `claude-opus-4-7` | similar, default_thinking_level: medium |
| `claude-haiku-4-5` | smaller window, no extended thinking |
| `gpt-5` | context_window: 272_000, apply_patch_tool_type: shell, tool_overrides: {extra: apply_patch, excluded: edit}, capabilities: {tool_calling, structured_output, reasoning_summaries} |
| `gpt-5-2` | similar, apply_patch_tool_type: freeform |
| `gemini-2.5-pro` | context_window: 1_000_000, capabilities: {extended_thinking} |

Coverage expands over time; the authoritative list is in `coco_config::builtin::builtin_models`. Models not in builtin (e.g., `deepseek-r1`, custom finetunes) must declare `context_window` and `max_output_tokens` somewhere in the merge chain.

## 10. Why `providers.json` and `models.json` as sibling files

An earlier draft put both catalogs inline in `settings.json`. Promoting them to siblings:

1. **Separation of concerns** — three files, three questions:
   - `settings.json` — "what does this user want?" (role bindings, features, hooks, preferences)
   - `providers.json` — "how do we reach providers?" (endpoints, env keys, served models)
   - `models.json` — "what models exist and what are they like?" (context_window, capabilities, thinking levels, extra_body)
2. **Switching configurations is just editing settings.json.** Work / personal / dev / prod toggle by changing role bindings; shared infrastructure files stay untouched.
3. **Shareability.** Team admins ship a corporate `providers.json` (proxy URLs, env keys); community curators publish `models.json`. Users drop them into `~/.coco/` without copy-paste-merging into personal preferences.
4. **Distinct lifecycles.** Provider endpoints change on vendor cadence; model metadata changes on vendor cadence; user preferences change on user cadence. Three diff histories.
5. **Clean override layering.** Three well-defined layers per resolution (builtin → catalog file → settings.json). "Where does `claude-opus-4-7.context_window` come from?" is a 3-step lookup in single-source files, not a needle search inside a fat JSON.
6. **Builtin extension without recompilation.** Editing the file overrides built-ins; no PR to the binary required.
7. **Per-user override path is preserved.** `settings.json.providers.<name>` partial overlays still merge per-key onto the catalog file; users tweak personal `organization_id` or `extra_body` without forking the team's catalog.

This is **not** the cocode-rs `*provider.json` / `*model.json` glob pattern. Glob expansion makes "which file owns this key" debugging painful. `~/.coco/providers.json` and `~/.coco/models.json` are **single fixed-name files**, each with one purpose. The 6-source `SettingsWithSource` traceability still applies to `settings.json`; the catalog files are intentionally single-source because they are not per-project preferences.

## 11. Reload / Hot-Reload

🎯 **Target.** `SettingsWatcher` (`coco-config/src/settings/watcher.rs:14`) currently watches only the four settings paths and has a `TODO: Wire to utils/file-watch` placeholder. The target wires two additional files (`~/.coco/providers.json`, `~/.coco/models.json`) into the same debounced detector and rebuilds `RuntimeConfig` on any change.

Mechanism:

1. Re-runs `build_runtime_config()` → fresh `Arc<RuntimeConfig>` snapshot (with new `model_registry` and `tool_overrides`).
2. Publishes via `tokio::sync::watch::Sender<Arc<RuntimeConfig>>`.
3. Subscribers (`QueryEngine`, TUI, …) call `receiver.borrow().clone()` at turn boundaries to obtain the fresh `Arc`. In-flight turns retain the `Arc` they captured at turn start; a mid-turn config change has no torn-read effect — the next turn picks up the new snapshot atomically.

### 11.1 Closing the role-binding ↔ provider-client coherence gap

**Problem.** `tool_overrides` and `ModelRegistry` rebuild eagerly on hot reload, but the `Arc<dyn LanguageModelV4>` cached inside `ApiClient` does not. After a role rebinding (Main switches from `openai-direct/gpt-5` to `azure-east/gpt-5`), the next turn would filter tools using the new (provider, model) pair but still issue HTTP to the old `ApiClient`. Tool-result schema and request URL would diverge.

**Fix — turn-boundary fingerprint check.** At the start of every turn, `QueryEngine` computes:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderClientFingerprint {
    pub provider:               String,         // ProviderConfig.name
    pub api:                    ProviderApi,
    pub api_model_name:         String,         // resolved per-(provider, model)
    pub base_url:               String,
    pub wire_api:               WireApi,
    pub client_options_digest:  [u8; 32],       // blake3 over typed ProviderClientOptions
    pub timeout_secs:           i64,
    pub api_key_origin_digest:  [u8; 32],       // hash of (env_key_value | api_key_redacted_marker)
                                                // — detects rotated keys without storing the secret
}

impl QueryEngine {
    fn ensure_client_for_turn(&mut self, runtime: &RuntimeConfig, role: ModelRole) -> Result<()> {
        let fp = ProviderClientFingerprint::compute(runtime, role)?;
        if self.api_client.fingerprint() != &fp {
            let new_client = ApiClient::build(runtime, role, &fp)?;
            self.api_client = Arc::new(new_client);  // release the old client
        }
        Ok(())
    }
}
```

Properties:

- **Atomic with role-binding read.** Both `tool_overrides` and `api_client` are taken from the same `Arc<RuntimeConfig>` captured at turn start; they cannot diverge.
- **Cheap.** Fingerprint compare is `==` over 8 fields; no rebuild when nothing material changed (the common case during settings.json edits that touch only features).
- **Key rotation detected.** `api_key_origin_digest` is computed at fingerprint time from the current env-var value (or the redacted-secret pointer if `api_key` is set in config). The digest itself is non-reversible; we never store the live key.
- **`extra_body` is not in the fingerprint.** It is a per-call concern (rebuilt every turn in `build_call_options`), so changing it does not invalidate the cached client.

This replaces the prior "tracked in audit-gaps.md" note. The check runs unconditionally at each turn boundary and is O(1) when the fingerprint matches.

## 12. Crate Responsibility

| Concern | Crate | Key items |
|---|---|---|
| Foundation enums | `coco-types` | `ProviderApi`, `WireApi`, `ModelRole`, `ModelSpec`, `Capability`, `ApplyPatchToolType`, `ThinkingLevel`, `ReasoningEffort`, `ToolOverrides`, `Features` |
| Settings + RuntimeConfig | `coco-config` | `Settings`, `PartialProviderConfig`, `ProviderConfig` (incl. `resolve_api_key`), `PartialProviderModelOverride`, `ProviderModelOverride`, `PartialProviderClientOptions`, `ProviderClientOptions`, `PartialModelInfo`, `ModelInfo`, `PositiveTokens`, `PositiveCount`, `RedactedSecret`, `ModelRoles`, `ModelRegistry`, `ResolvedModel`, `RuntimeConfig`, `ConfigError`, `builtin_models`, `tool_overrides::resolve_tool_overrides` |
| Inference wrapper | `coco-inference` | `ApiClient` (with `fingerprint(): &ProviderClientFingerprint`), `RetryConfig`, `build_call_options`, `thinking_convert::to_extra_body`, `PerCallOverrides`, `ProviderClientFingerprint` |
| Provider construction | `app/cli/src/model_factory.rs` | `build_language_model_from_runtime`, `build_anthropic` / `build_openai` / `build_google` / `build_openai_compat`, `build_http_client` |
| Tool filter Layer 2 | `coco-tool-runtime` + `coco-config` | `ToolOverrides` plumbed through `RuntimeConfig.tool_overrides` → `ToolUseContext.tool_overrides` |
| Per-provider SDK (Layer 3) | `vercel-ai-{anthropic,openai,google,openai-compatible}` | `*Provider`, `*ProviderSettings`, `extract_*_options` (typed-known + leftover return signature, §7.3) |

The boundary line between Coco and vercel-ai is exactly:

- **Coco owns**: `ModelInfo.extra_body` (Layer 1), `build_call_options` (Layer 2 entry), `model_factory` (Layer 2 client construction).
- **vercel-ai owns**: `LanguageModelV4CallOptions`, `ProviderOptions`, every provider implementation. We add a small uniform leftover-merge to each provider's `get_args` and **do not** modify call-options or trait shapes.

## 13. Design Decisions

| Decision | Rationale |
|---|---|
| **Three-layer boundary (config / coco-boundary / vercel-ai)** | Vercel AI v4 spec namespaces `ProviderOptions` by provider instance name. Exposing that to user config makes models non-portable across instances and creates a footgun ("`provider_options.openai` is silently inert if you reroute to `azure-east`"). Hiding it inside Layer 2 keeps user config provider-agnostic. |
| **`ModelInfo.extra_body: BTreeMap<String, JSONValue>` (flat, single level, camelCase)** | Layer 1 user surface. One mental model: "extra wire-body keys for this model." Layer 2 wraps it under `ProviderConfig.name` exactly once. `BTreeMap` keeps serialised output deterministic for snapshots and diff review. camelCase is the wire convention because every Layer-3 typed-options struct uses `#[serde(rename_all = "camelCase")]`. |
| **`provider_options` as the namespace mechanism (kept as upstream defines)** | Outer key is `String` because a single `ProviderApi::OpenaiCompat` backs N user-named instances (`xai`, `groq`, `azure-east`, …). Enum cannot represent 1-to-N. This is upstream `@ai-sdk/provider` v4 contract; not negotiable. |
| **Uniform leftover-merge in every Layer 3 provider** | Generalising openai-compat's existing pattern to Anthropic / OpenAI / Google with 5 lines per `get_args` makes `extra_body` semantics provider-uniform. The previous Lane A/B isolation invariant was already broken in openai-compat (`passthrough` map merged after typed body); this design embraces that behaviour rather than fighting it. |
| **`PartialProviderConfig` and `PartialModelInfo` (every-Option overlay; `from_partial → Result`)** | Prevents serde defaults from masking missing required fields (`api`, `context_window`, `max_output_tokens`) in catalog overlays. The wire format and the resolved form are different types — there is no `Option<i64>` on resolved `ModelInfo` for required fields. |
| **Identity = parent map key; no `name` field on `PartialProviderConfig`** | `serde(deny_unknown_fields)` rejects user-written `name` at parse time; `from_partial(map_key, partial)` writes `name = map_key` in exactly one place. Eliminates the release-vs-debug skew that a `debug_assert_eq!` would have introduced. |
| **`RedactedSecret` newtype for all credential fields** | `Debug`/`Display` print `<redacted>`; `.expose()` is the single audit point. Defence-in-depth against panics, snafu cause chains, assertion failures, and any other code path that goes through `Debug` before reaching the log-sink-level `secret-redact` post-processor. A type-level guarantee, not a string-pattern hope. |
| **`ProviderClientOptions` typed (not `HashMap<String, JSONValue>`)** | serde reports field-level errors with JSON pointers; `deny_unknown_fields` actually catches typos; downstream `match` arms are exhaustively checked at compile time. Pure pass-through (rare) goes through `ModelInfo.extra_body`, which is intentionally untyped. |
| **`PositiveTokens` / `PositiveCount` newtypes** | `TryFrom<i64>` validates at deserialise time with `ConfigError::NonPositiveTokens`; downstream `From<PositiveTokens> for u64` is infallible. The arithmetic chain through `build_call_options` no longer contains a single `as u64` cast — negative `top_k` / `max_output_tokens` cannot wrap to `u64::MAX`. |
| **`Option<T>` retained for `temperature`, `top_p`, `top_k` in resolved `ModelInfo`** | `None` carries wire semantics ("let provider default"); typed body builders write the field only on `Some`. Forcing concrete defaults would override provider defaults globally. |
| **`context_window` / `max_output_tokens` are concrete `PositiveTokens`** | Every model has one, and they participate in budget / compact / tool-schema decisions; "unset" at this layer would be a bug. The on-disk overlay (`PartialModelInfo`) keeps them `Option<PositiveTokens>` so omission is detectable. |
| **On-disk maps are `BTreeMap` (not `HashMap`)** | `serde_json::to_string` over `BTreeMap` produces sorted keys; CI diffs, insta snapshots, and review patches are stable. Runtime indices that don't serialise can stay `HashMap` if hot-path lookup matters; nothing in this plan does. |
| **Turn-boundary `ProviderClientFingerprint` rebuild** | At each turn, `QueryEngine` compares the live `ProviderClientFingerprint` against the cached one; mismatch → rebuild `Arc<dyn LanguageModelV4>`. Atomic with `tool_overrides` because both come from the same `Arc<RuntimeConfig>` snapshot. Guarantees that role rebinding via hot-reload cannot leave `tool_overrides` and `api_client` inconsistent. |
| **`~/.coco/providers.json` + `~/.coco/models.json` as sibling files** | Provider catalog and model catalog have different ownership / lifecycle / shareability profile than user preferences. Switching configurations only edits `settings.json`. See §10. |
| **`ProviderModelOverride`** | Per-(provider, model) override layer — overrides any builtin/catalog `ModelInfo` for the (provider, model) pair. "Override" makes the per-key delta semantics explicit; the rejected alternative `ProviderModelEntry` was ambiguous ("entry of what?"). New (provider, model) pairs that lack a builtin/catalog still go through the same struct — an override against an empty base is well-defined. |
| **`extra_body` field name (renamed from `options` and `extras`)** | "Options" was ambiguous against `ProviderConfig.client_options`; "extras" was vague. "extra_body" mirrors the LangGraph / OpenAI Python SDK convention and reads as "additional wire-body fields." |
| **Delete dormant `ProviderInfo` / `ProviderModel`** | Zero callsites outside lib.rs re-exports. Their fields (`timeout_secs`, `streaming`, `wire_api`) merge into `ProviderConfig`. |
| **`ModelRegistry` as `RuntimeConfig` field** | Closes the L1 dormant gap — `runtime.rs::resolve_main_tool_overrides` previously always passed `None`. With registry, lookup is O(1) and `Some(&info)` flows through. |
| **`full_url` semantic preserved** | Required for OpenAI-compat Azure-style routing where `base_url` is a complete endpoint path; without it the SDK appends `/chat/completions` and the request 404s. (For Azure with Responses API, prefer `ProviderApi::Openai` direct — see §6.2.) |
| **Builtin registry compiled-in (not file-loaded)** | Matches cocode-rs `builtin.rs` `OnceLock` pattern; avoids first-run network/filesystem lookup. User overrides via catalogue files flesh out custom finetunes. |
| **No new typed sampling fields on `ModelInfo`** | Keep existing typed (`temperature`, `top_p`, `top_k`, `max_output_tokens`). `frequency_penalty` / `presence_penalty` / `seed` / `stop_sequences` go through `extra_body` — the rare user accepts the wire-shape responsibility. |

## 14. Replaces / removed from prior versions

- **Three-lane separation with "no cross-merge" invariant** — Replaced by the three-layer boundary model. The flat-`extra_body` ⊕ uniform leftover-merge design supersedes Lane A/B isolation. The third-party review demonstrated that Lane A/B isolation was already broken in OpenAI-compat; the new design generalises that behaviour rather than fighting it.
- **Provider-namespaced `extras` in user config** — Removed. Users wrote `extras: HashMap<String, JSONValue>` flat in earlier drafts but the spec implied namespaced semantics; the new `extra_body` makes the flatness explicit and makes namespace wrapping a Layer 2 internal concern.
- **`ModelHub`** — coco-rs has no central model hub; provider clients are built per-`ApiClient` via `model_factory`.
- **`excluded_tools: Vec<String>` on ModelInfo** — superseded by `ToolOverrides { extra, excluded }` in `coco-types`, plumbed through ModelRegistry.
- **5-step `RequestBuilder` pipeline** — replaced by `build_call_options` (Layer 2 per-call) + `build_language_model_from_runtime` (Layer 2 client construction).
- **Beta headers matrix / `provider_supports(provider, cap, model)`** — Anthropic-specific concerns belong in `vercel-ai-anthropic`. Capabilities are per-`ModelInfo.capabilities`; use `info.has_capability(cap)`.
- **`ApplyPatchTool` conditional registration via `model_info.apply_patch_tool_type`** — registration is driven by `ToolOverrides` Layer 2 (`extra: ["apply_patch"]`). The `apply_patch_tool_type` field still exists on `ModelInfo` but drives *request shape* only (`shell` / `freeform` / `function`), not registry membership.

## 15. Test Invariants

Group A is the original cross-layer behaviour. Group B is the Rust-invariant pass — each closes a specific third-party-review claim and must have a dedicated test.

### A. Cross-layer behaviour

| Invariant | What to assert |
|---|---|
| Per-key precedence (providers) | `settings.providers.<name>.<key>` wins over `providers.json.<name>.<key>` wins over builtin, **field-by-field**; an overlay that omits `api` does NOT coerce the resolved `api` to a serde default. |
| Per-key precedence (models) | `provider_cfg.models.<id>.<key>` wins over `models.json.<id>.<key>` wins over builtin |
| `extra_body` shallow-merge order | `info.extra_body` < `per_call.extra_body` (per-call wins; both are flat single-level `BTreeMap`s) |
| Layer 2 wraps under `ProviderConfig.name` exactly once | After `build_call_options`, `call.provider_options.0` has exactly one outer key, and that key equals `provider_cfg.name`. The same `ModelInfo` rendered under two different provider names produces two different namespaces but identical inner key-sets. |
| Namespace round-trip | The runtime `model.provider()` string equals the outer key in `call.provider_options.0`. |
| Layer 3 leftover-merge cross-provider uniform | A flat camelCase key written into `extra_body` that does NOT match any provider's typed schema (e.g. `"myCustomField": "x"`) appears in the wire body for Anthropic, OpenAI Chat, OpenAI Responses, Google, AND OpenAI-compat. (Once §7.3 generalisation lands.) |
| Layer 3 typed-known wins structured shape | A flat key that DOES match a typed schema (`thinking` for Anthropic, `serviceTier` for OpenAI, `safetySettings` for Google) is parsed into the typed struct and produces the structured wire-body shape rather than a raw passthrough. |
| `ModelInfo` is provider-portable | Switching a role from `provider_a/m1` to `provider_b/m1` (same `model_id`, different provider instance) re-uses the same `ModelInfo` (no `models.json` change required); only the namespace key in `call.provider_options.0` differs. |
| Settings-only inline equivalent to split files | A `settings.json` containing the full `providers` block produces the same `RuntimeConfig` as the same data hoisted into sibling `providers.json` + minimal `settings.json`. |
| Missing required field → typed error | `models.json` declaring `gpt-99` without `context_window` returns `ConfigError::MissingContextWindow { provider, model }`, never panics, never silently uses a serde default. |
| New provider in settings.json only | A provider name appearing only in `settings.providers.<name>` requires `api`, `env_key`, `base_url` in the partial; otherwise `ConfigError::IncompleteProviderEntry`. |
| Three-way concurrent reload | Editing all three files inside the same `SettingsWatcher` debounce window produces exactly one `RuntimeConfig` rebuild with all changes applied; in-flight turns continue with the pre-reload `Arc`. |
| `tool_overrides` plumbed | Switching the `Main` role's model causes `RuntimeConfig.tool_overrides` to reflect the new model's `ToolOverrides` (closes the L1 dormant gap from `runtime.rs:141-156`). |
| `full_url` preserved | An `openai_compat` provider with `client_options.full_url = true` produces a `*ProviderSettings` where the SDK skips its default path suffix (Azure-style routing). |
| `include_usage` default | An `openai_compat` provider with `client_options.include_usage` unset produces `OpenAICompatibleProviderSettings.include_usage = None`, which the provider treats as `false` (matches `openai_compatible_provider.rs:98`). Setting `true` or `false` explicitly round-trips. |

### B. Rust invariants (each ↔ a third-party-review claim)

| Invariant | What to assert | Closes claim |
|---|---|---|
| `RedactedSecret::Debug` never leaks | `assert_eq!(format!("{secret:?}"), "RedactedSecret(<redacted>)")`; `format!("{cfg:?}")` over a `ProviderConfig` containing an `api_key` MUST NOT contain the raw key bytes (assert via `!.contains(raw_key)`). | #1 |
| Identity = map key (no split-brain) | `serde_json::from_str::<PartialProviderConfig>(r#"{"name": "x", "api": "openai"}"#)` returns `Err(...)` with `unknown field "name"`. After resolution, `runtime.providers.iter().all(|(k, v)| k == &v.name)` holds in **release builds** (not just debug). | #2 |
| `extra_body` casing — typed parser sees the key | An `extra_body` `{"reasoningSummary": "auto"}` deserialises into `OpenAIResponsesProviderOptions { reasoning_summary: Some("auto"), .. }`. An `extra_body` `{"reasoning_summary": "auto"}` deserialises into `reasoning_summary: None` AND surfaces in the wire body's `reasoning_summary` field via leftover-merge (documenting the failure mode). | #3 |
| `PositiveTokens` / `PositiveCount` reject ≤0 | `serde_json::from_str::<PartialModelInfo>(r#"{"context_window": -1}"#)` returns `Err(ConfigError::NonPositiveTokens { value: -1 })`. `From<PositiveTokens> for u64` is infallible by type (compile-checked). `grep "as u64" coco-rs/services/inference/src/build_call_options.rs` returns zero matches. | #4 |
| `ProviderClientOptions` is typed | `serde_json::from_str::<PartialProviderClientOptions>(r#"{"orgization_id": "o"}"#)` (typo) returns `Err(...)` with field name `orgization_id` and a JSON pointer. Adding a new field without threading it through every `model_factory::build_*` arm fails to compile. | #5 |
| Hot-reload provider-client coherence | After hot-reloading `providers.json` to point `Main` at a different provider instance, the next turn's `api_client.fingerprint() != old_fingerprint` and a fresh `Arc<dyn LanguageModelV4>` is built. `tool_overrides` and `api_client` are taken from the same `Arc<RuntimeConfig>` (assert `Arc::ptr_eq` between the two read sites). | #6 |
| On-disk `BTreeMap` produces stable serialisation | Round-trip `providers.json` 100 times through `from_str` + `to_string_pretty`; assert byte-identical output every time. Same for `models.json`. | #7 |

## 16. Implementation Status Snapshot

This table reflects the difference between the current code and the target design. Track migration progress here.

| Item | Status | Location | Notes |
|---|---|---|---|
| `ProviderConfig` non-Option `api` | ✅ Implemented (with bug) | `common/config/src/provider/mod.rs:16` | Bug per third-party review #3; replace with `PartialProviderConfig` |
| `ProviderConfig` derives `Debug` over raw `api_key: Option<String>` | ✅ Implemented (with bug) | `common/config/src/provider/mod.rs:12,16` | Leak risk; introduce `RedactedSecret` newtype + custom Debug (§5.1.2) |
| `ProviderConfig.client_options: HashMap<String, JSONValue>` | 🎯 Target | not present | Convert to typed `ProviderClientOptions` (§5.1.3) |
| `ModelInfo.options: Option<HashMap<...>>` | ✅ Implemented | `common/config/src/model/mod.rs:86` | Rename to `extra_body: BTreeMap<String, JSONValue>` (default empty) |
| `ModelInfo` numeric defaults via serde | ✅ Implemented (with bug) | `common/config/src/model/mod.rs:33-36, 89-95` | Bug per third-party review #2; split via `PartialModelInfo` + `PositiveTokens` |
| `as u64` casts in inference client | ✅ Implemented (with bug) | `services/inference/src/client.rs:134, 191` | Replace with `From<PositiveTokens>` (§5.2.1, §7) |
| Identity field `name` on partial overlay | 🎯 Target (delete) | not present | Identity is the parent map key; reject any user-written `name` via `deny_unknown_fields` |
| `BTreeMap` for on-disk catalog maps | 🎯 Target | currently `HashMap` patterns elsewhere | Convert `providers.json` / `models.json` deserialisation to `BTreeMap` |
| `RedactedSecret` newtype | 🎯 Target | not present | New type in `coco-config`; replaces all `api_key` / `auth_token` strings |
| `PositiveTokens` / `PositiveCount` newtypes | 🎯 Target | not present | New types in `coco-config`; replace bare `i64` for token counts |
| `ProviderClientFingerprint` | 🎯 Target | not present | New type in `coco-inference`; turn-boundary coherence check (§11.1) |
| `ModelRegistry`, `ResolvedModel` | 🎯 Target | not present | New type, defined in `crate-coco-config.md` |
| `runtime.rs::resolve_main_tool_overrides` plumbed | 🎯 Target | `common/config/src/runtime.rs:141-156` passes `None` | Wire through `ModelRegistry` |
| `build_language_model_from_runtime` (handles Volcengine / Zai / OpenaiCompat) | 🎯 Target | `app/cli/src/model_factory.rs:35-70` errors out | Rewrite with `RuntimeConfig` argument |
| `build_call_options` | 🎯 Target | `services/inference/src/client.rs:132` inlines | Extract to `services/inference/src/build_call_options.rs` |
| Layer 3 uniform leftover-merge | 🎯 Target (partial) | OpenAI-compat: ✅ `openai_compatible_chat_language_model.rs:194-199`. Anthropic / OpenAI direct / Google: ❌ | Add 5-line shallow-merge to four `get_args` |
| `SettingsWatcher` watches catalog files | 🎯 Target | `common/config/src/settings/watcher.rs:14` watches 4 settings paths only | Extend watch list to add `providers.json` / `models.json` + actually wire `utils/file-watch` (currently TODO) |
| Hot-reload via `tokio::sync::watch::Sender<Arc<RuntimeConfig>>` | 🎯 Target | not present | Build during `RuntimeConfigBuilder` integration |
| Turn-boundary fingerprint rebuild of `ApiClient` | 🎯 Target | not present | New code path in `QueryEngine`; closes claim #6 |
| Dormant `ProviderInfo` / `ProviderModel` deletion | 🎯 Target | `common/config/src/provider/mod.rs:80, 116` | Delete; merge fields into `ProviderConfig` |
| Azure example (correct wire API) | ✅ Doc-only | this doc §6.2 | The previous draft pointed `openai_compat` at a `/responses` endpoint; corrected here |
| `extra_body` casing example | ✅ Doc-only | this doc §4.2 | Was `reasoning_summary` (snake_case, silently dropped); now `reasoningSummary` with rule note |
| Anthropic prompt-cache + beta policy | ✅ Implemented (Round 7, schema migrated R7-10) | `vercel-ai-anthropic` (`cache_policy`, `cache_placement`, `beta_resolver`, `beta_capabilities`, `provider_options`); `services/inference::cache_convert` pass-through | Full design in `prompt-cache-design.md` (with R7-10 follow-up in `audit-gaps.md`). Adapter owns policy; inference layer is opaque pass-through. `ProviderClientFingerprint.runtime_state_digest` invalidates cached client on settings reload of `account` / `prompt_cache.allowlist` / per-provider `provider_options` map (formerly `anthropic_knobs.*`, now per-instance under `ProviderConfig.provider_options`). |
