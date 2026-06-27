# Tool Input Schema — Source-of-Truth Refactor (v4.2, final)

> **Follow-up (post-v4.3, superseding doc claims below):** a later refactor
> **deleted the output-schema path entirely** — `Tool::output_schema` /
> `derive_output_schema` are gone and the `Self::Output: JsonSchema` bound is
> dropped (kept `Serialize + Deserialize`; output → JSON is `serde_json::to_value`,
> never `JsonSchema`). So the "Output path untouched" non-goal and the
> "`Self::Output: JsonSchema` — Kept" decision recorded below are **historical**.
> Same follow-up: `SchemaIssue` now reports all unexpected keys + enumerates
> multi-type mismatches; a strict-free `to_openai_compatible_schema`
> (`oneOf→anyOf` + safe `allOf`-flatten) was added to the OpenAI-family wire
> path; and the model-facing tools array is now name-sorted for prompt-cache
> stability.

> **Supersedes**:
> - [`tool-schema-validated-newtype-plan.md`](tool-schema-validated-newtype-plan.md) (v1, deprecated) — Input↔Schema type binding plus reject-style strict subset; refuted by the measured tool distribution.
> - [`tool-schema-source-plan.md`](tool-schema-source-plan.md) (v2, deprecated) — three-source-kind abstraction over-engineered.
> - v3 / v3.1–v3.5 (this document, earlier revisions) — the **separate `ToolSchemaValidator` cache** family. v4 collapses it; see [Revision Log](#revision-log).
>
> v4 is grounded in three Explore-agent surveys + line-by-line reading of the live source. v4.1 integrates a **sixth, adversarial review round** (three independent reviewers) that corrected two factual premises and surfaced real migration-scope, hook-interaction, and MCP-namespacing hazards. v4.2 integrates a **seventh round** that verified the `additionalProperties:false` closure is multi-provider wire-safe (Gemini strips it; OpenAI/compat run non-strict) and fixed two P1s the round-6 fixes themselves introduced (lazy-`OnceLock` panic timing; `tool_count=0` for report-less servers).
>
> **Single PR + ~7 commits, one breaking reshape**, aligned with the project rule "no backward-compat shims".

---

## The v4 thesis: the validator belongs *inside* the schema

Every prior revision (v1–v3.5) kept the compiled JSON-Schema validator in a
**separate `ToolSchemaValidator` cache** keyed by `ToolId`, decoupled from the
schema it validates. That single decision is the source of nearly all the
plan's complexity:

- async validation behind a `tokio::sync::RwLock` ([`schema.rs:84-87`](../../coco-rs/core/tool-runtime/src/schema.rs)),
- a content-addressed `CachedValidator { schema_bytes, validator }` to detect MCP-reconnect schema churn,
- bounded-growth management of that cache,
- a `SchemaCompileFailed` validate-site branch (compile happens lazily, on the *first* validate call — [`schema.rs:136,192`](../../coco-rs/core/tool-runtime/src/schema.rs)),
- and a proposed `InternalSchemaError` invalid-reason variant for the fail-closed path.

The deeper smell: v3.5's `from_value`/`from_input_type` **compile the validator
at construction (for meta-validation) and then throw it away**, leaving the
cache to recompile it later. **The schema is compiled twice.**

**v4 keeps the construction-time validator** by storing `Arc<jsonschema::Validator>`
inside `ToolInputSchema`. Validation becomes a synchronous, lock-free method on
the schema. The cache, its async surface, its content-addressing, its growth
management, the fail-closed branch, and the `InternalSchemaError` variant **all
disappear**. MCP-reconnect staleness becomes *structurally impossible*: a
reconnect builds a new tool → new schema → new validator; the registry overwrite
drops the old one. There is nothing to invalidate.

### Headline changes vs. v3.5

| Area | v3.5 (separate cache) | v4.1 (validator-in-schema) | Driver |
|---|---|---|---|
| Validator location | `ToolSchemaValidator` cache keyed by `ToolId`; compiled lazily on first validate; threaded through `QueryEngine`/`ToolContextFactory`/`ToolUseContext` | `ToolInputSchema` **owns** `Arc<jsonschema::Validator>`, compiled once at construction | v4 thesis |
| Validation call | `async`, `tokio::RwLock` read/write per call | **sync, lock-free** `schema.validate(input)` | v4 thesis |
| MCP-reconnect staleness | `CachedValidator { schema_bytes }` + content-addressed replace-on-change + bounded-growth tests | **structurally impossible** (new tool ⇒ new validator; registry overwrite drops old) | v4 thesis |
| `SchemaCompileFailed` at validate site | fail-closed via new `InternalSchemaError` variant | **does not exist** — a tool is only registered if its schema compiled | v4 thesis |
| Schema compiled | **twice** (construction meta-validate → discarded + lazy cache compile) | **once** (construction, kept) | v4 thesis |
| Bucket-A migration | add a `schema` field + `new()` to each tool | **unit structs preserved**; `runtime_validation_schema` returns a per-tool `OnceLock` static — no field, no `new()`, no call-site churn | round-6 finding 1 |
| Remote `$ref` handling | "`validator_for` panics" → `reject_remote_refs` guard | **false premise**: pinned `default-features = false` ⇒ remote `$ref` returns `Err`, no panic/fetch. Guard dropped; `from_value`'s `map_err` already covers it; add a build-invariant test | round-6 finding 2 |
| Root-type strictness | strict reject of typeless/array root | **fold-in `type:"object"`** when absent (preserves McpTool behavior); reject only *explicit* non-object roots | round-6 finding 3 |
| Field-honesty closure | `additionalProperties:false` + `#[serde(deny_unknown_fields)]` | `additionalProperties:false` only; **`deny_unknown_fields` dropped** as redundant with always-run closed-schema validation (finding 4 = Option A) | round-6 finding 4 |
| `tool_count` source | `ToolRegistry::count_by_server` (separate registry read → TOCTOU) | `report.registered.len()` — single report read; **`count_by_server` deleted** | round-6 finding 7 |

---

## Context

### Problems being solved

1. **Dynamic-schema garbage reaching the wire** ([`traits.rs:480-491`](../../coco-rs/core/tool-runtime/src/traits.rs)). Tools whose `type Input = Value` fall through to a `derive_input_schema_value::<Value>()` that produces `{type:"null"}`/`anyOf` garbage; strict OpenAI-compatible providers respond 400. Firefought twice in prod (McpTool X3 fix `0303dc3ef2`; StructuredOutput P0). The `debug_assert!` only trips in dev.
2. **Two schema representations** today: `input_schema() -> ToolInputSchema{properties,required}` *and* `input_json_schema() -> Option<Value>`, bridged by `effective_tool_schema` ([`schema.rs:65-77`](../../coco-rs/core/tool-runtime/src/schema.rs)). Drift-prone (AgentTool's two disagree — see finding 10).
3. **Validator cache key has no schema content** ([`schema.rs`](../../coco-rs/core/tool-runtime/src/schema.rs)). After an MCP reconnect the stale validator stays live; `clear()` must be called by hand.
4. **"Must override for `Value` tools" is a cultural convention.** The next Plugin/Custom/HTTP/SDK dynamic-schema tool steps on the same trap. The type system can enforce it.
5. **Late, silent validation failure.** Compile happens on the *first* validate call; on failure [`tool_input_validate.rs:94-104`](../../coco-rs/app/query/src/tool_input_validate.rs) **logs and skips schema validation, then executes**. The model appears able to call the tool but its input is never validated.
6. **MCP reconnect is not transactional.** `register_mcp_tools` ([`core/tools/src/lib.rs:129-148`](../../coco-rs/core/tools/src/lib.rs)) calls `deregister_by_server` then per-tool `register` — separate write locks ([`registry.rs:103,258`](../../coco-rs/core/tool-runtime/src/registry.rs)); readers between iterations see a partial tool set.

### Non-goals

- Do **not** bind Input ↔ Schema at the type level (≥10 tools decouple them).
- Do **not** introduce a "schema source kinds" trait/newtype abstraction layer.
- Do **not** sanitize / lower / coerce external or user schemas ([codex-rs `sanitize_json_schema`](../../codex-rs/tools/src/json_schema.rs) is a no-op against schemars 1.2 output or actively wrong against external contracts).
- Do **not** touch the Output schema path (`Tool::output_schema` + `derive_output_schema`) — no production consumer; legal Output shapes (string/array/tagged union) don't match the tool-input "root object" invariant.
- Do **not** narrow the **runtime validation schema** below what hooks/permission rewrites may inject (AgentTool `mcp_servers`, Bash `_simulatedSedEdit` must stay accepted).
- Do **not** widen the **model-facing schema** above what `Self::Input` deserializes.
- Do **not** apply `additionalProperties:false` to **external** schemas (`McpTool` wire, `StructuredOutput` user input) — closing those would silently reject valid third-party payloads.
- Do **not** ship the TUI display of MCP `skipped_tools`/`tombstoned_tools` this PR (independent `McpStartupStatusParams` path — separate scope).

---

## Phase 1 findings (verified tool distribution)

Source of truth: [`core/tools/src/lib.rs:22-92`](../../coco-rs/core/tools/src/lib.rs) — **42 static `registry.register(...)`** calls in `register_all_tools`. Two more (McpTool, StructuredOutputTool) register dynamically → **44** total production surface.

**Census verified by grep** (`impl Tool for` + `fn input_schema`/`fn input_json_schema` in `core/tools/src`, excluding tests) — this corrects BOTH the v3.5 estimate (26/15) and the round-7 reviewer's estimate (≈19/8). Exactly **8** tools override a schema method: `AgentTool` (E); `McpTool` + `StructuredOutputTool` (D); `AskUserQuestion` / `Bash` / `TodoWrite` / `WebFetch` / `WebSearch` (B/C). Everything else is plain derive-only — including the Task* tools, `SkillTool`, and the MCP-resource tools (which prior surveys wrongly placed in B/C).

| Bucket | Count | Current shape |
|---|---|---|
| **A: derive-only** | **36** | `type Input = TypedStruct`; no schema override. Unit structs (`pub struct ReadTool;`). |
| **B/C: override-input_schema** | **5** | AskUserQuestion · Bash · TodoWrite · WebFetch · WebSearch — typed Input; `input_schema()` overridden (hand-written Value or derive+mutate) |
| **E: session-aware** | **1** | AgentTool — overrides `input_schema` + `*_for_session` |
| Static subtotal | **42** | |
| **D: dynamic-wire** | **2** | McpTool/StructuredOutputTool — `type Input = Value`; override `input_json_schema` |
| Full surface | **44** | |

Key verified facts the templates depend on:
- `ToolInputSchema { properties: HashMap, required: Vec }` lives in [`common/types/src/tool.rs:371-376`](../../coco-rs/common/types/src/tool.rs).
- `McpToolSchema` fields are `server_name` / `tool_name` / `description` / `input_schema` / `annotations` ([`mcp_handle.rs:104-112`](../../coco-rs/core/tool-runtime/src/mcp_handle.rs)).
- `ToolId` has **no `as_str()`** — wire form is `Display`/`to_string()` ([`tool.rs:261-300`](../../coco-rs/common/types/src/tool.rs)).
- `ToolInputInvalidReason` lives in [`vercel-ai/provider/src/content.rs:207-247`](../../coco-rs/vercel-ai/provider/src/content.rs) (variants `JsonParseFailed`/`SchemaViolation`/`NoSuchTool`) — **not** in `app/query`.
- `SchemaContext { background_tasks_disabled, fork_mode_active, features }` ([`traits.rs:23-37`](../../coco-rs/core/tool-runtime/src/traits.rs)).
- `coco-mcp-types` has its **own** `ToolInputSchema` wire DTO and **no `coco-types` dep** — deleting `coco_types::ToolInputSchema` does not touch it.
- `jsonschema = "0.46.5"`, `default-features = false` ([`Cargo.toml:246`](../../coco-rs/Cargo.toml)) — `resolve-http` OFF; `Validator` is `Clone+Debug+Send+Sync`; `iter_errors` is the all-errors API.

---

## Design

### Core type — self-validating newtype (owner = `coco-tool-runtime`)

`coco_types::ToolInputSchema` is **deleted outright**; the new owner is `coco-tool-runtime` (L3, which already depends on `schemars`+`jsonschema`+`coco-error`). `coco-types` (L1) must not reverse-depend on schemars.

```rust
// core/tool-runtime/src/schema.rs
#[derive(Clone)]
pub struct ToolInputSchema {
    value: Value,                            // full JSON Schema; root type:"object"
    validator: Arc<jsonschema::Validator>,   // compiled ONCE at construction
}

// Manual Debug — the compiled tree is huge/noisy (mirrors structured_output.rs:62-69).
impl std::fmt::Debug for ToolInputSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolInputSchema")
            .field("value", &self.value)
            .field("validator", &"<compiled>")
            .finish()
    }
}

impl ToolInputSchema {
    /// Bucket A — schemars-derived; close (additionalProperties:false), then compile.
    /// A failure is a tool-author bug ⇒ panic w/ type name (caught by the registry
    /// test the moment the tool is constructed; cannot ship).
    pub fn from_input_type<T: JsonSchema>() -> Self {
        let mut raw = crate::derive::derive_input_schema_value::<T>();
        if let Some(o) = raw.as_object_mut() {
            o.insert("additionalProperties".into(), Value::Bool(false));
        }
        Self::from_value(raw).unwrap_or_else(|e| panic!(
            "schemars-derived schema for {} failed: {e}", std::any::type_name::<T>(),
        ))
    }

    /// Bucket B/C/D/E — programmer Value, derive+mutate, MCP wire schema, or user
    /// `--json-schema`. Normalizes the root, then compiles (= meta-validation) and
    /// KEEPS the validator. No sanitize/lower; external schemas pass verbatim
    /// (modulo the type fold-in below).
    pub fn from_value(mut raw: Value) -> Result<Self, SchemaError> {
        normalize_root_object(&mut raw)?;
        let validator = jsonschema::validator_for(&raw)
            .map_err(|e| SchemaError::InvalidSchema { message: e.to_string() })?;  // remote $ref → Err here
        Ok(Self { value: raw, validator: Arc::new(validator) })
    }

    pub fn as_value(&self) -> &Value { &self.value }

    /// SYNC, lock-free. Returns the same `SchemaIssue` classification today's
    /// `validate_collect` produces (consumed by `format_schema_error`).
    pub fn validate(&self, input: &Value) -> Result<(), Vec<SchemaIssue>> {
        let issues: Vec<SchemaIssue> = self.validator
            .iter_errors(input).map(SchemaIssue::from_jsonschema).collect();
        if issues.is_empty() { Ok(()) } else { Err(issues) }
    }
}

/// Fold-in `type:"object"` ONLY when the root carries neither `type` nor a
/// composition keyword (`$ref`/`allOf`/`anyOf`/`oneOf`/`not`) — folding it onto a
/// composition root would corrupt the contract (round-7 finding 2a; the current
/// McpTool fold-in at mcp_tools.rs:390-392 is unconditional and slightly wrong).
/// Reject only EXPLICIT non-object roots: `type:"array"`, `type:"null"`, and array
/// form `["object","null"]` (which would let `null` inputs reach `execute(Value::Null)`).
/// This is the ONLY mutation `from_value` makes to an external schema — documented
/// at the `from_value` doc so the "verbatim external schema" promise stays honest.
pub(crate) fn normalize_root_object(value: &mut Value) -> Result<(), SchemaError>;

/// ONE clone of `schema` with every `field` removed from `properties` and
/// `required`; drops `required` if it empties. Plural (round-7 finding 5) so
/// AgentTool's two conditional omits cost a single clone, not two chained ones.
/// Used by `model_schema` overrides — the model view is never validated, so no
/// validator recompile. Mirrors agent_tool.rs:378-385.
pub fn schema_omit_properties(schema: &Value, fields: &[&str]) -> Value {
    let mut out = schema.clone();
    if let Some(obj) = out.as_object_mut() {
        if let Some(props) = obj.get_mut("properties").and_then(Value::as_object_mut) {
            for f in fields { props.remove(*f); }
        }
        if let Some(req) = obj.get_mut("required").and_then(Value::as_array_mut) {
            req.retain(|v| v.as_str().is_none_or(|s| !fields.contains(&s)));
            if req.is_empty() { obj.remove("required"); }
        }
    }
    out
}
```

**Kept verbatim** from today's `schema.rs`: `SchemaIssue` enum + `from_jsonschema`
+ `format_type_kind` + `json_type_name` ([`schema.rs:221-299`](../../coco-rs/core/tool-runtime/src/schema.rs)). `app/query::format_schema_error` is untouched.

```rust
// SchemaError — tier 3 via thiserror + manual StackError + ErrorExt, mirroring
// coco_context::ContextError (the crate uses thiserror, NOT snafu — no new dep).
// ErrorExt requires `status_code` + `as_any`; the rest default.
#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("schema root must declare type:\"object\" as a single string \
             (composition/array forms like [\"object\",\"null\"] are rejected)")]
    RootTypeNotObject,
    #[error("schema root is the singleton null type")]
    RootTypeNull,
    #[error("schema failed JSON Schema meta-validation: {message}")]
    InvalidSchema { message: String },
}
impl StackError for SchemaError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) { buf.push(format!("{layer}: {self}")); }
    fn next(&self) -> Option<&dyn StackError> { None }
}
impl ErrorExt for SchemaError {
    fn status_code(&self) -> StatusCode { StatusCode::InvalidArguments }   // non-retryable
    fn as_any(&self) -> &(dyn std::any::Any + 'static) { self }
}
```

**Build invariant (test):** assert `jsonschema` stays `default-features = false`
(resolve-http OFF) so a schema with a remote `$ref` is rejected as `Err` from
`validator_for` — never fetched (SSRF-safe for untrusted MCP schemas) or panicked.

**Deleted (no longer needed):** `ToolSchemaValidator`, its `cache`, `CachedValidator`,
`canonical_bytes`, content-addressed keys, bounded-growth handling, `clear()`,
`SchemaValidationError` (+`SchemaCompileFailed`/`Rejected`), `effective_tool_schema`,
the async `validate*` methods, any `reject_remote_refs`/`RemoteRefUnsupported`, and the
once-proposed `InternalSchemaError`.

### Tool trait reshape

```rust
pub trait Tool: Send + Sync + 'static {
    type Input: for<'de> Deserialize<'de> + Send + Sync + 'static;            // DROP JsonSchema bound
    type Output: Serialize + for<'de> Deserialize<'de> + JsonSchema + Send + Sync + 'static; // keep

    /// Runtime validation schema — static, owns the validator. Validated on EVERY
    /// call incl. hook-rewritten input (tool_call_preparer.rs:743). No default ⇒ E0046.
    /// MUST be a superset of every model_schema(ctx) view; internal schemas carry
    /// additionalProperties:false; external (Bucket D) pass through verbatim.
    fn runtime_validation_schema(&self) -> &ToolInputSchema;

    /// Model-facing schema — a plain Value (never validated, only serialized into
    /// the prompt). Default borrows the runtime value. Tools with runtime-only
    /// hook-injected fields (AgentTool mcp_servers, Bash _simulatedSedEdit) override.
    fn model_schema(&self, _ctx: &SchemaContext) -> Cow<'_, Value> {
        Cow::Borrowed(self.runtime_validation_schema().as_value())
    }

    fn output_schema(&self) -> Option<Value> { Some(derive_output_schema::<Self::Output>()) } // unchanged
    // strict() / execute() / render_for_model() / everything else unchanged.
}
```

The model view is `Value` (only serialized) — not a validated newtype — so
`schema_omit_properties` is a cheap clone+remove with no throwaway validator
recompile, and the model/runtime concerns are cleanly separated.

**DynTool sync:** add `runtime_validation_schema` + `model_schema`; delete the old
four (`input_schema` / `input_schema_for_session` / `input_json_schema` /
`input_json_schema_for_session`) from the trait, `DynTool`, and the blanket impl.
`output_schema` / `strict` kept. Both new methods forward 1:1; object-safe
(today's `input_json_schema_for_session(&self, &SchemaContext) -> Option<Value>` already is).

**Invariants (integration-tested, commit 7):**
1. *Subset*: `model_schema(ctx).properties ⊆ runtime_validation_schema().properties` for every tool × every SchemaContext.
2. *Field honesty*: every runtime property maps to a `Self::Input` deserialize field (AgentTool `mcp_servers` + Bash `_simulatedSedEdit` are the only hook-injected exceptions).
3. *Closure*: internal runtime schemas carry `additionalProperties:false`; AgentTool's model schema has no `mcp_servers`.

### Validation path (sync)

```rust
// app/query/src/tool_input_validate.rs::validate_tool_call (drops the validator param)
match tool.runtime_validation_schema().validate(&tc.input) {
    Ok(())     => {}
    Err(issues) => {
        tc.invalid = true;
        tc.invalid_reason = Some(ToolInputInvalidReason::SchemaViolation {
            message: format_schema_error(&tc.tool_name, &issues),
        });
    }
}
```

No `SchemaCompileFailed`/`Rejected` arms, no `.await`, no `Option` gate. Compile
failure is impossible at the validate site (a tool is only registered if its
schema compiled) — it surfaces at the *boundary*: built-in bug → startup panic
(force-init test); MCP → skipped + reported; `--json-schema` → `StructuredOutputTool::new`
`Result`.

**There are THREE validate sites, not two** (round-7 finding 4) — all must flip to sync:
1. pre-hook model input — [`tool_runner.rs:90-95`](../../coco-rs/app/query/src/tool_runner.rs) (drop the `Option` gate);
2. post-hook re-validation — [`tool_call_preparer.rs:743-758`](../../coco-rs/app/query/src/tool_call_preparer.rs);
3. **permission `updated_input` re-validation** — `tool_call_preparer.rs:660` (`validate_effective_input_or_complete_error`), the path SDK/TUI approval + the production `CanUseTool` fork rewrite funnel through.

All three use the **same closed runtime schema** (finding 4 = Option A — a rewrite injecting an *undeclared* field errors). Dropping `deny_unknown_fields` is safe **only because** every value reaching `Tool::execute` passes one of these three; the alternate `execution::execute_tool_call` step-3.5 rewrite is **dead in production** (zero non-test callers) — commit 6 must add a regression test asserting the closed schema rejects an undeclared field via the production preparer path, or delete that dead entry, so a future revival can't re-open the v3.5 hole.

---

## Tool migration by bucket

### Bucket A — 36 derive-only tools (unit structs preserved, via a macro)

The **36** derive-only tools (verified census) each need the identical lazy-static
body, so a **declarative macro** (round-7 findings A + 1) replaces the copy-paste —
the unit struct + all ~120 `Arc::new(GrepTool)` call sites (11 files) stay untouched:

```rust
// core/tool-runtime/src/schema.rs
macro_rules! impl_runtime_schema {
    ($tool:ty, $input:ty) => {
        fn runtime_validation_schema(&self) -> &ToolInputSchema {
            static S: std::sync::OnceLock<ToolInputSchema> = std::sync::OnceLock::new();
            S.get_or_init(ToolInputSchema::from_input_type::<$input>)
        }
    };
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadInput { /* ... */ }            // keep JsonSchema; NO deny_unknown_fields

pub struct ReadTool;                          // stays a UNIT STRUCT
impl Tool for ReadTool {
    type Input = ReadInput; type Output = ReadOutput;
    impl_runtime_schema!(ReadTool, ReadInput);
}
```

A marker-trait blanket impl is **infeasible** (E0119 vs hand-written impls; no
stable specialization); a proc-macro is over-abstraction (no tool proc-macro
infra exists). The 8 tools that returned `ToolInputSchema::default()` (empty)
become `from_value(json!({"type":"object","additionalProperties":false,"properties":{}}))`.

> **Round-7 finding 1c (P1):** `from_input_type` now runs **lazily on first
> `runtime_validation_schema()` call**, not at construction — so a malformed
> built-in schema would panic *in production on first use*, not in CI. The
> registry-count test does **not** call the schema method. **Required gate**
> (commit 3): a test that force-initializes every tool's schema —
> `for t in reg.all() { let _ = t.runtime_validation_schema(); }` — turning a
> bad schema into a CI panic. Without it, "cannot ship" is unsubstantiated.

`deny_unknown_fields` is intentionally **not** added: the closed runtime schema is
validated at both validate sites on every call (it is not a bypassable optional
cache like v3.5), so it is the single enforcement point; adding the serde attribute
is redundant, costs 26 edits, and risks `#[serde(flatten)]` incompatibility.

### Bucket B/C — 5 hand-built / derive+mutate (AskUserQuestion · Bash · TodoWrite · WebFetch · WebSearch)

Build once via `from_value(json!({ "type":"object", "additionalProperties":false,
"properties":{...}, "required":[...] }))`. `from_value` folds in `type:"object"`
if a hand-body omits it. Bodies preserved verbatim (no schemars swap this PR).
TodoWrite: `derive_input_schema_value::<TodoWriteInput>()` (still `pub`) → inject
the status `enum` at `/properties/todos/items/properties/status` → set
`additionalProperties:false` → `from_value`.

### Bucket D — 2 dynamic tools (now fallible)

```rust
impl McpTool {
    pub fn new(wire: Value, /* ... */) -> Result<Self, SchemaError> {
        Ok(Self { schema: ToolInputSchema::from_value(wire)?, /* ... */ })  // fold-in keeps type-omitted MCP schemas
    }
}
impl StructuredOutputTool {
    pub fn new(user_schema: Value) -> Result<Self, SchemaError> {           // already returned Result
        Ok(Self { schema: ToolInputSchema::from_value(user_schema)? })      // drop the separate `validator` field
    }
    // execute() validates via self.schema.validate(&input) — error shape now Vec<SchemaIssue>.
}
```

External schemas keep their author's `additionalProperties` (never force-closed);
StructuredOutput's model view ≡ user schema (modulo the `type` fold-in).

### Bucket E — AgentTool (dual-track; fixes the current `mcp_servers` leak)

Runtime schema = the **exact 10 `AgentInput` fields** ([`agent_tool.rs:28-76`](../../coco-rs/core/tools/src/tools/agent/agent_tool.rs)) incl. `mcp_servers` (hook-injected) + `run_in_background` (runtime-accepted), `additionalProperties:false`. No fictional `effort`/`model`.

```rust
fn runtime_validation_schema(&self) -> &ToolInputSchema { &self.schema }
fn model_schema(&self, ctx: &SchemaContext) -> Cow<'_, Value> {
    let mut drop = vec!["mcp_servers"];                                       // ALWAYS hidden
    if ctx.background_tasks_disabled || ctx.fork_mode_active { drop.push("run_in_background"); }
    Cow::Owned(schema_omit_properties(self.schema.as_value(), &drop))         // ONE clone
}
```

> **This fixes a live bug.** Today AgentTool overrides `input_schema()` (omits `mcp_servers`) but **not** `input_json_schema()`, so the model-facing seam (`input_json_schema_for_session`, [`agent_tool.rs:375-388`](../../coco-rs/core/tools/src/tools/agent/agent_tool.rs)) blanket-derives from `AgentInput` (which **includes** `mcp_servers` at line 75) and strips only `run_in_background` — the model currently **sees `mcp_servers`**. Reconcile the stale comments at `agent_tool.rs:31-36,228-242`. `run_in_background` omission is safe (never in `required`).

### Bash dual-track

Runtime schema declares `_simulatedSedEdit` (the TUI sed-edit dialog injects it
before re-validation at [`tool_call_preparer.rs:743`](../../coco-rs/app/query/src/tool_call_preparer.rs)); `model_schema` omits it via `schema_omit_properties(self.schema.as_value(), &["_simulatedSedEdit"])`. `BashInput` keeps no `deny_unknown_fields` (consistent with the Bucket-A decision) and continues to deserialize `_simulatedSedEdit` via its `#[serde(rename)]`.

---

## MCP: atomic swap + fallible construction + reporting

### Registry ([`core/tool-runtime/src/registry.rs`](../../coco-rs/core/tool-runtime/src/registry.rs))

Extract private `remove_tool_by_id` + `register_with_aliases` — the latter
**replicates the MCP-namespace promotion at `registry.rs:113-127` verbatim**
(`McpTool::name()` returns the *bare* `tool_name`; a naive insert would let a
hostile MCP `Read` shadow the built-in — round-6 finding 5).

```rust
/// Atomically replace all tools for one MCP server under a SINGLE write lock.
/// Returns the tombstoned ToolIds (present last batch, absent now).
pub fn replace_server_tools(&self, server: &str, new_tools: Vec<Arc<dyn DynTool>>) -> Vec<ToolId> {
    let mut inner = self.inner.write().unwrap_or_else(PoisonError::into_inner);
    // 1. Server-owned canonical names + ToolIds from inner.tools (the alias map has NO server tag).
    let owned: HashSet<String> = inner.tools.iter()
        .filter(|(_, t)| t.mcp_info().is_some_and(|i| i.server_name == server))
        .map(|(name, _)| name.clone()).collect();
    let old_ids: HashSet<ToolId> = /* ids of owned tools */;
    let new_ids: HashSet<ToolId> = new_tools.iter().map(|t| t.id()).collect();
    let tombstones: Vec<ToolId> = old_ids.difference(&new_ids).cloned().collect();
    // 2. Wipe ALL server-owned aliases by full membership (NOT the tombstone diff — finding 6).
    inner.aliases.retain(|_, canonical| !owned.contains(canonical));
    // 3. Drop tombstoned tools. 4. Re-register the new batch (namespacing-replicating helper).
    for id in &tombstones { inner.remove_tool_by_id(id); }
    for t in new_tools { inner.register_with_aliases(t); }
    tombstones
}
```

`mcp_info()`/`id()` clone from `self.info` and never re-lock, so the guard spans
the whole swap (no re-entrancy on the non-reentrant `std::sync::RwLock`).
**Delete** `ToolRegistry::definitions` (returns the deleted `coco_types::ToolInputSchema`; no production caller — the 5 grep hits are the unrelated `vercel_ai_provider_utils::ToolRegistry`). No `count_by_server` is added (finding 7).

### `register_mcp_tools` → report ([`core/tools/src/lib.rs`](../../coco-rs/core/tools/src/lib.rs))

```rust
pub fn register_mcp_tools(reg: &ToolRegistry, server: &str, mcp_tools: Vec<McpToolSchema>) -> RegisterMcpToolsReport {
    let mut valid = Vec::new(); let mut skipped = Vec::new(); let mut seen = HashSet::new();
    for ts in mcp_tools {                                  // fields: ts.input_schema, ts.tool_name (verified)
        if !seen.insert(ts.tool_name.clone()) {            // round-7 finding 3b: dedup intra-batch
            skipped.push(SkippedTool { name: ts.tool_name, error: /* DuplicateToolName */ });
            continue;                                      // else replace_server_tools silently last-wins
        }
        match McpTool::new(ts.input_schema.clone(), &ts.tool_name, /* ... */) {
            Ok(t)  => valid.push(Arc::new(t) as Arc<dyn DynTool>),
            Err(e) => skipped.push(SkippedTool { name: ts.tool_name, error: e }),
        }
    }
    let registered: Vec<ToolId> = valid.iter().map(|t| t.id()).collect();   // stored in the report (finding 7)
    let tombstones = reg.replace_server_tools(server, valid);
    RegisterMcpToolsReport { registered, skipped, tombstones }
}
pub struct RegisterMcpToolsReport { pub registered: Vec<ToolId>, pub skipped: Vec<SkippedTool>, pub tombstones: Vec<ToolId> }
pub struct SkippedTool { pub name: String, pub error: SchemaError }
```

### Full SDK `mcp/status` surfacing

- [`common/types/src/server_request.rs`](../../coco-rs/common/types/src/server_request.rs) `McpServerStatus` += `skipped_tools: Vec<McpSkippedToolStatus>` + `tombstoned_tools: Vec<String>` (both `#[serde(default, skip_serializing_if)]` — old clients unaffected); new `McpSkippedToolStatus { tool_name, error, status_code }`.
- [`app/cli/src/sdk_server/handlers/mod.rs`](../../coco-rs/app/cli/src/sdk_server/handlers/mod.rs) `SdkServerState` += `mcp_registration_reports: tokio::sync::RwLock<HashMap<String, RegisterMcpToolsReport>>` + `record_*`/`clear_*`. **No `.await` is held under the registry's std write-lock** at any call site (`lib.rs:129` non-async, `handlers/mcp.rs:119`, `sdk_mcp.rs:74`) — verified; helpers take `&mut RegistryInner`.
- Both call sites persist the report; **clear it on every disconnect path** (deregister, failed reconnect [`mcp.rs:384`](../../coco-rs/app/cli/src/sdk_server/handlers/mcp.rs), toggle-off, session reset — round-6 finding 8).
- `handle_mcp_status` ([`mcp.rs:24-71`](../../coco-rs/app/cli/src/sdk_server/handlers/mcp.rs)): `tool_count` via a **typed `SdkServerState::registered_tool_count(server) -> Option<i32>`** accessor (don't destructure the report in the handler — round-7 finding 8b) that returns `report.registered.len()` (registered, not advertised; single read also yields skipped/tombstoned — kills the round-6 TOCTOU). **Fall back to advertised `server.tools.len()` when no report exists** (round-7 finding 4a — agent-spawn inline MCP servers connect via `mcp_handle_adapter.rs add_dynamic_server` *without* `register_mcp_tools`, so `report.registered.len()` would wrongly show 0). Iterate manager-known names so a stale report never displays; `ToolId::to_string()` for wire.
- TUI display (`McpStartupStatusParams`, [`tui/src/state/session.rs:748`](../../coco-rs/app/tui/src/state/session.rs)) is **out of scope** (follow-up); skips also `tracing::warn`-logged.

---

## Dead-code cleanup & consumer changes

| Target | Action |
|---|---|
| `services/inference/src/tool_schemas.rs` (+ `.test.rs`, `mod`, 7 `pub use`) | Delete (dead — only test callers) |
| `common/types/src/tool.rs::ToolInputSchema` (+ re-export) | Delete |
| `derive.rs::derive_input_schema` (struct variant) + `schema_value_to_tool_input_schema` | Delete. **Keep `derive_input_schema_value` (pub)** + `derive_output_schema` |
| `traits.rs` old 4 schema methods (Tool + DynTool + blanket) | Delete |
| `schema.rs` `ToolSchemaValidator` / `effective_tool_schema` / `SchemaValidationError` / async `validate*` | Delete |
| 4 validator holder fields: `QueryEngine` (`engine.rs:239`, built `engine_builder.rs:92`), `ToolContextFactory` (`tool_context.rs:125`, clone `:484`), `ToolUseContext` (`context.rs:407`, clone `:671`, None `:888`), `engine_prompt.rs:609` set | Delete + their construction/clone lines |
| `engine_prompt.rs:492-499` | → `tool.model_schema(&schema_ctx).into_owned()` (model schema always present) |
| `use coco_types::ToolInputSchema` | → `coco_tool_runtime::ToolInputSchema` |
| `ToolRegistry::definitions` (`registry.rs:244-249`) | Delete (no production caller) |

Final grep gate: `ToolInputSchema|input_schema|input_json_schema|ToolSchemaValidator`
resolves only to the new `coco_tool_runtime::ToolInputSchema` + the two new methods.

---

## Implementation sequence (single PR, ~7 commits)

End state has **no default** for `runtime_validation_schema` (E0046 forces every
tool). A temporary scaffolding default may keep commits 2–5 green and is removed
in commit 6.

| # | Contents | Est |
|---|---|---|
| 1 | `ToolInputSchema` newtype (`from_input_type`/`from_value`/`as_value`/`validate` — all in `schema.rs` beside the `pub(crate)` `SchemaIssue::from_jsonschema`; manual Debug); move `SchemaIssue`+helpers; `SchemaError`; `normalize_root_object` (composition-aware fold-in + reject explicit non-object); `schema_omit_properties(&Value, &[&str])` (plural — one clone). Build-invariant test via **`cargo metadata`** asserting `jsonschema`'s activated features exclude `resolve-http`/`resolve-file` (a behavioral `http://`-ref test would hang/panic if the feature flips — round-7 finding 5); plus an unknown-scheme `Err` smoke test. Unit tests. | 4h |
| 2 | Trait/DynTool reshape + blanket forward; `replace_server_tools` (namespacing-replicating `register_with_aliases`, full-membership alias-wipe) + `remove_tool_by_id`; delete `definitions`. | 5h |
| 3 | Migrate 36 Bucket A via the `impl_runtime_schema!` macro (unit structs preserved; the few empty-input tools → empty closed object) + **force-init test** (`for t in reg.all() { t.runtime_validation_schema(); }`) — the gate that turns a bad schema into a CI panic. | 4h |
| 4 | Migrate 5 B/C (AskUserQuestion/Bash/TodoWrite/WebFetch/WebSearch) + 2 D (fallible ctors) + AgentTool (10 fields; `model_schema` omits `mcp_servers`/`run_in_background`; reconcile comments) + Bash dual-track + TodoWrite. `register_mcp_tools` → report via `replace_server_tools` (intra-batch `tool_name` dedup). | 5h |
| 5 | Flip **all three** validate sites to sync against the closed runtime schema (`tool_runner.rs:90`, post-hook `tool_call_preparer.rs:743`, permission-rewrite `tool_call_preparer.rs:660` — round-7 finding 4); update ~12 test doubles' `runtime_validation_schema`. | 4h |
| 6 | Delete `ToolSchemaValidator`/`effective_tool_schema`/`SchemaValidationError` + 4 holder fields; `engine_prompt` → `model_schema`; fix the stale `Tool::Input` doc comment (no longer "derived from JsonSchema"); remove scaffolding default; delete `coco_types::ToolInputSchema` + dead `tool_schemas.rs`. Add the closed-schema-rejects-undeclared-field regression test via the production preparer path (or delete the dead `execution::execute_tool_call` step-3.5 rewrite) — round-7 finding 4. | 4h |
| 7 | SDK surfacing: `McpServerStatus` fields + `McpSkippedToolStatus`; `SdkServerState` report store + record/clear on all disconnect paths; `handle_mcp_status` (`tool_count = report.registered.len()`); update `handlers/tests.rs:3721`; integration tests (subset/field-honesty/closure/alias-hygiene/namespacing-no-shadow/remote-$ref-is-Err). | 5h |
| **Total** | | **~4 days** |

---

## Verification

```bash
cd coco-rs
just quick-check
just test-crate coco-tool-runtime   # newtype + atomic swap + alias hygiene + namespacing-no-shadow + remote-$ref-is-Err
just test-crate coco-tools          # force-init every tool's schema (panics on a bad one); bucket counts; field-honesty (AgentTool no mcp_servers)
just test-crate coco-query          # engine_prompt + sync validate; post-hook re-validation
just pre-commit                     # final gate, once
```

Manual / targeted:
- `--json-schema '{"type":"array"}'` and `'{"type":["object","null"]}'` fail cleanly at startup; `'{"additionalProperties":true,"type":"object",…}'` passes intact; **typeless `{"properties":{…}}` passes** (root fold-in).
- MCP: type-omitted wire schema **registers** (fold-in); invalid schema → `skipped_tools`; removed tool → `tombstoned_tools`; reconnect with a changed schema validates the new shape (no stale validator); a re-advertised `Read` does **not** shadow built-in Read; a retained tool whose aliases changed `["r"]→["repo"]` leaves no stale `r`; `mcp/status.tool_count == report.registered.len()`; disconnect clears the report.
- AgentTool: model schema has **no `mcp_servers`** and no `run_in_background` when background-disabled/fork; runtime validator accepts hook-injected `mcp_servers`.
- Bash: model omits `_simulatedSedEdit`; runtime accepts the TUI-injected payload.
- Post-hook: a hook injecting an **undeclared** field → hard `InputValidationError` (finding 4 = A); declared runtime-only fields pass.
- `SchemaError` classifies as `StatusCode::InvalidArguments` and is non-retryable (`ErrorExt`).

---

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Touching ~44 `impl Tool` sites (+ ~12 test doubles) | 36 Bucket A via the `impl_runtime_schema!` macro; unit structs + ~120 call sites untouched; the **force-init test** (not the count test — finding 1c) turns a bad/missed schema into a CI panic |
| Eager per-tool compile at startup | ~42 flat inlined schemas, <1ms total, once at bootstrap; MCP compile dwarfed by the connection handshake |
| `from_input_type::<T>()` panic opaque | Message carries the failing input type name + meta-validation error |
| Non-object `--json-schema` now fails at startup | Intended (the original P0 was the runtime 422); release-note item |
| `additionalProperties:false` rejects a hook-injected undeclared field (finding 4 = A) | Documented behavior change; escape hatch = declare the field in `Self::Input` + the runtime schema (as `mcp_servers`/`_simulatedSedEdit`); the field was a silent no-op before, so this surfaces latent hooks |
| MCP author hits a `SchemaError` where openai-compat used to silently wrap | `RegisterMcpToolsReport.skipped` surfaces through SDK `mcp/status`; warn log carries the same info |
| AgentTool field-honesty regression | Integration test asserts the two property sets + `additionalProperties:false` + no `mcp_servers` in the model view |
| `replace_server_tools` helpers diverge from `register` | Helpers replicate `registry.rs:113-127` namespacing verbatim; alias-hygiene + namespacing-no-shadow regression tests |
| `tool_count` advertised→registered | Release-note semantics change; the new value is what the model can call; update `handlers/tests.rs:3721` + note TUI `picker.rs:544` |
| `mcp_registration_reports` map grows | Bounded by distinct server names; cleared on every disconnect path; `handle_mcp_status` only reads manager-known names |
| Future code flips `jsonschema` to `resolve-http` | Build-invariant test fails; documents the SSRF/blocking-fetch hazard for untrusted MCP schemas |

---

## Decision Log

| Decision | Choice | Rationale |
|---|---|---|
| Validator location | **Inside `ToolInputSchema`** (`Arc<Validator>`, compiled at construction) | v4 thesis — deletes the cache + its async/staleness/growth/fail-closed machinery; the schema was already compiled at construction and discarded |
| Validation call | **Sync, lock-free** | No shared cache ⇒ no lock ⇒ no `.await` on the hot path |
| MCP-reconnect staleness | **Structurally impossible** | New tool ⇒ new schema ⇒ new validator; registry overwrite drops the old |
| `SchemaCompileFailed` at validate site | **Does not exist** | A tool is only registered if its schema compiled; failures surface at the boundary (MCP skip+report / StructuredOutput Result / built-in startup panic) |
| `InternalSchemaError` variant | **Not added** | Unreachable under construction-time compile |
| Schema vs Input type binding | **Decoupled** | ≥10 counter-example tools; v1 death |
| Schema source abstraction layer | **None** (one newtype + two entries) | Bucket distribution shows a trait + multi-newtype is over-engineering |
| Constructor entries | **Two** (`from_input_type<T>` panicking, `from_value` Result) | TodoWrite needs `from_value` for derive-then-mutate; `from_input_type` is the convenience wrapper |
| Root-type policy | **Fold-in `type:object` if absent; reject explicit non-object** | Preserves McpTool's current fold-in (else type-omitted MCP schemas drop); array `["object","null"]` still rejected (null inputs for Value tools) |
| Field-honesty closure | **`additionalProperties:false` on internal schemas; `deny_unknown_fields` NOT added** (finding 4 = A) | The always-run closed schema is the single enforcement point; the serde attr is redundant + 26 edits + flatten risk |
| Post-hook re-validation | **Same closed runtime schema** (finding 4 = A) | Simplest; a rewrite injecting an undeclared field errors (escape: declare it) |
| Model-facing schema type | **`Cow<'_, Value>`** (not a validated newtype) | Never validated, only serialized; `schema_omit_properties` is a cheap clone with no recompile |
| Schema ownership | **`coco-tool-runtime`** (delete `coco_types::ToolInputSchema`) | The type calls schemars+jsonschema; L1 `coco-types` must not reverse-depend on L3 |
| `Self::Input: JsonSchema` bound | **Removed** | `Value` no longer "passes"; Bucket A derives concretely via `from_input_type::<ConcreteInput>()` |
| `Self::Output: JsonSchema` bound | **Kept** | Output path untouched |
| Bucket A storage | **Unit structs + `OnceLock` static** (finding 1) | Avoids ~120 `Arc::new` call-site rewrites + per-tool `new()` |
| Remote `$ref` | **`from_value` returns `Err` via `validator_for` map_err; build-invariant test keeps resolve-http off** (finding 2) | The "panic" premise was false for `default-features = false`; SSRF-safe |
| MCP registration | **Partition + atomic `replace_server_tools` (single write lock)** | Non-atomic deregister/register leaked partial state |
| `register_with_aliases` | **Replicates `registry.rs:113-127` namespacing verbatim** (finding 5) | Else a hostile MCP tool shadows a built-in |
| Alias-wipe | **By full server membership from `inner.tools`, before drop** (finding 6) | The alias map has no server tag; tombstone-diff wipe leaks changed aliases |
| `tool_count` source | **`report.registered.len()`; `count_by_server` deleted** (finding 7) | Single read kills the TOCTOU; no other consumer |
| MCP report storage | **`SdkServerState::mcp_registration_reports`**, cleared on all disconnect paths | `register_mcp_tools` had no storage seam; `mcp/status` polls later |
| `McpServerStatus` extension | **Add `skipped_tools` + `tombstoned_tools`, serde-default optional** | Forward-compatible with old SDK clients |
| MCP report surfacing | **SDK path this PR; TUI follow-up** | TUI uses an independent `McpStartupStatusParams` path |
| Output schema path | **Untouched** | No production consumer |
| Phase count | **One PR + ~7 commits** | "No backward-compat"; finding 1 widened the scope from ~6 |

---

## Revision Log

### v4.3 — implemented + verified (single PR landed)

The v4.2 design is **implemented and green**: the self-validating
`ToolInputSchema` newtype owns its `Arc<jsonschema::Validator>` in
`core/tool-runtime/src/schema.rs`; the `ToolSchemaValidator` cache,
`effective_tool_schema`, `SchemaValidationError`, async validation, and the
four model-facing `*_for_session` trait methods are deleted; the 36 Bucket-A
tools use `impl_runtime_schema!`; MCP registration is atomic via
`replace_server_tools` + `RegisterMcpToolsReport`; `mcp/status` surfaces
`skipped_tools` + `tombstoned_tools`. Full-workspace `cargo check --tests` and
the refactor-crate nextest (1618 tests) pass.

**One real finding-4 instance surfaced in test, fixed exactly as designed.**
`ExitPlanMode` injects two internal fields via the query-layer normalizer
(`tool_input_normalizer.rs`): `plan` (already declared) **and `planFilePath`
(was NOT declared)**. Under the new closed (`additionalProperties:false`)
runtime schema, the post-normalization re-validation rejected the undeclared
`planFilePath`, so the tool never completed
(`exit_plan_mode_observable_input_includes_disk_plan`). Fix = the documented
escape hatch: declare `plan_file_path` (`#[serde(rename = "planFilePath")]`)
on `ExitPlanModeInput` beside the existing internal `plan`/`user_choice`. This
both validates the finding-4 analysis and confirms the escape hatch works. The
other two injection sites were already safe: Bash declares `_simulatedSedEdit`
(and `model_schema` omits it — the dual-track) and TaskOutput's normalizer
`.remove()`s the legacy aliases as it maps them onto canonical declared keys.

**Tier-2 completed (naming-collision dedup).** `coco_types::ToolInputSchema`
(the old `{properties, required}` shape) and the `Tool::input_schema()` bridge
are **deleted**. The 8 hand-built / dynamic tools (WebFetch, WebSearch, Bash,
TodoWrite, AskUserQuestion, AgentTool, McpTool, StructuredOutput) now build
their closed `coco_tool_runtime::ToolInputSchema` directly: the 6 hand-built
ones inline `from_value(json!({ …, "additionalProperties": false, … }))` in
`runtime_validation_schema` (Bash folds `_simulatedSedEdit`, AgentTool folds
`mcp_servers`, TodoWrite derives-and-mutates via `derive_input_schema_value`);
the 2 dynamic ones (McpTool / StructuredOutput) already stored the validated
newtype from the wire / `--json-schema`, so only their vestigial reverse-derive
`input_schema()` was removed. `closed_schema`, `derive_input_schema`, and
`schema_value_to_tool_input_schema` are gone; `derive_input_schema_value` +
`derive_output_schema` survive. **There is now exactly ONE `ToolInputSchema` in
the workspace** — the self-validating L3 newtype; the L1 ↔ L3 name collision is
resolved. ~25 files (8 tools + trait/DynTool/blanket + helpers + the L1 type +
~15 test doubles/assertions, incl. a rewritten `derive.test.rs`); **zero
behavior change** (model-facing schemas + validation byte-identical), full
workspace `cargo check --tests` clean with no warnings, affected-crate nextest
1618/1618 green. The grep gate confirms no live references to any deleted
machinery (`ToolSchemaValidator`, `effective_tool_schema`, `closed_schema`,
`derive_input_schema`, `*_for_session`, `coco_types::ToolInputSchema`,
`services/inference::tool_schemas`).

### v4.2 — seventh (adversarial) review, attacking the round-6 fixes + provider compat

**Verified SAFE (the biggest open risk):** `additionalProperties:false` on internal
schemas is **wire-safe across all five providers** — Gemini's OpenAPI converter
strips `additionalProperties` (`convert_json_schema_to_openapi_schema.rs:17,276-299`);
OpenAI + openai-compatible run **non-strict** (`engine_prompt.rs:522` hardcodes
`strict: None`, `Tool::strict()` is never read on the wire path, no `with_strict`
call exists); Anthropic passes it through; ByteDance has no tool path. **No
per-provider strip step is needed.** External (Bucket D) schemas hit the same
converter, so open-vs-closed is consistent.

| # | Finding | Severity | Resolution |
|---|---|---|---|
| 1c | Lazy `OnceLock` moves the panic to first *use*; the registry-count test never calls the schema method, so a bad built-in schema passes CI and panics in production | **P1** | Mandatory force-init gate (commit 3): `for t in reg.all() { let _ = t.runtime_validation_schema(); }` |
| 4a | `tool_count = report.registered.len()` shows **0** for connected-without-report servers (agent-spawn inline MCP connects without `register_mcp_tools`) | **P1** | Fall back to advertised `server.tools.len()` when no report; read via a typed `registered_tool_count` accessor |
| 4 (compat) | Plan undercounts validate sites ("both" → **three**: `tool_runner.rs:90`, `tool_call_preparer.rs:743` + `:660`); dropping `deny_unknown_fields` leans on the dead `execution::execute_tool_call` staying test-only | P2 | Commit 5 flips all three; commit 6 adds a closed-schema-rejects-undeclared regression test (or deletes the dead entry) |
| 2a | `from_value` fold-in of `type:"object"` is unguarded against composition roots (`$ref`/`allOf`/`oneOf` with no `type`) | P2 | Fold-in only when neither `type` nor a composition keyword is present |
| 2b | Fold-in silently narrows a typeless user `--json-schema` (byte-identity broken) | P2 | Documented as the single intended mutation at the `from_value` doc |
| 3b | Intra-batch duplicate MCP `tool_name` → silent last-wins in `replace_server_tools` | P2 | Dedup by `tool_name` in `register_mcp_tools`, skip + report the loser |
| 3c | Promised alias-hygiene test is a phantom — real McpTools have no user aliases (`aliases()` default `&[]`); shared global bare-alias namespace lets a server swap revoke another server's bare alias | P2 | Document bare-alias-is-global last-writer-wins; correct the test example |
| 8b | `tool_count` couples the SDK handler to the `RegisterMcpToolsReport` shape | P2 | Hide behind a typed `SdkServerState::registered_tool_count(server)` accessor |
| 6 (arch) | `SchemaIssue::from_jsonschema` is `pub(crate)`; `validate()` must live in `schema.rs` beside it | P2 | State the co-location in commit 1 |
| A / 1 | Bucket census wrong in every prior version; the repetition is the *dominant* case | P1→idiom | **Re-surveyed by grep: A=36 / B/C=5 / D=2 / E=1** (only 8 tools override a schema method). Adopt `macro_rules! impl_runtime_schema!` for the 36 (marker-trait blanket is E0119-infeasible; proc-macro is over-abstraction) |
| 5 | A behavioral remote-`$ref` build-invariant test hangs (sync) or panics (async) if `resolve-http` flips | P2 | Use a `cargo metadata` feature assertion + an unknown-scheme `Err` smoke test |
| 5 (arch) | AgentTool `model_schema` does two chained `schema_omit_property` clones per turn | P3 | `schema_omit_properties(&Value, &[&str])` — one clone |
| 1b | A future flip of wire `strict` to forward `tool.strict()` would arm OpenAI strict-mode against not-all-required closed schemas | P3 | Assert `strict: None` on the built tool definitions in the integration test |

Confirmed-safe by round 7 (with proof): `OnceLock` per-method-static soundness + `'static→'a` return coercion; atomic-swap retained-tool overwrite (single HashMap key, name+id move together); the panic-vs-`Result` ctor split (composes with `get_or_init`); `{value, validator}` storing both (the compiled `Validator` can't round-trip to source); `Cow<'_, Value>` as the model-view shape (correct perf; a `ModelSchema` newtype would be ceremony); the trait surface (no-default `runtime_validation_schema` is the right enforcement); and `model_schema → wire` envelope re-keying per provider.

### v4.1 — sixth (adversarial) review, 11 findings

| # | Finding | Severity | Revision |
|---|---|---|---|
| 1 | ~120 `Arc::new(UnitTool)` sites + ~71 `impl Tool` sites; adding a `schema` field breaks all of them | **P1** | Bucket A stays unit structs; `runtime_validation_schema` via `OnceLock` static — no field/`new()`/call-site churn |
| 2 | "`validator_for` panics on remote `$ref`" is FALSE for `default-features = false` (returns `Err`) | **P1** | Drop `reject_remote_refs`; `from_value` map_err covers it; build-invariant test keeps resolve-http off |
| 3 | Strict root check would drop type-omitted MCP schemas (McpTool folds in `type:object` today) | **P1** | `from_value` folds in `type:object`; reject only explicit non-object roots |
| 4 | `additionalProperties:false` + `deny_unknown_fields` rejects trusted-hook-injected fields | **P1** | Option A: keep closure, drop `deny_unknown_fields`; document the behavior change + escape hatch |
| 5 | `register_with_aliases` would let a hostile MCP `Read` shadow built-in Read | **P1** | Helper replicates `registry.rs:113-127` namespacing verbatim |
| 6 | Alias-wipe by tombstone-diff leaks aliases of retained tools whose alias set changed | **P1** | Wipe by full server membership computed from `inner.tools` before drop |
| 7 | `handle_mcp_status` 3-read TOCTOU; `count_by_server` redundant | P2→simplify | `tool_count = report.registered.len()`; delete `count_by_server` |
| 8 | Report not cleared on failed-reconnect/crash/session-reset | P2 | Clear on every disconnect path; status reads only manager-known names |
| 9 | `tool_count` advertised→registered is an observable wire change (test + TUI) | P2 | Documented; update `handlers/tests.rs:3721` |
| 10 | AgentTool currently SHOWS `mcp_servers` to the model (derive path) | P2 | v4 `model_schema` omits it; reconcile stale comments; field-honesty test |
| 11 | StructuredOutput `execute()` error shape changes; non-object `--json-schema` now a startup error | P2 | Intended (original P0 was the runtime 422); update tests |

### v3.x — first through fifth reviews (separate-cache family, superseded by v4)

The v3.1–v3.5 findings (root-type semantics, MCP atomicity, dead-code extent,
AgentTool `mcp_servers`/`run_in_background`, `derive_output_schema` name,
content-addressed cache key, fail-closed `SchemaCompileFailed`, `RegisterMcpToolsReport`
storage, `replace_server_tools` alias hygiene, `CachedValidator` bounded growth,
`McpToolSchema` field names) all targeted the **separate-cache** architecture. v4
makes the cache itself obsolete, so the cache-specific findings (content-addressed
key, `CachedValidator`, bounded growth, fail-closed branch, `InternalSchemaError`)
are resolved by deletion rather than refinement. The still-relevant ones (atomic
swap, alias hygiene, AgentTool field set, MCP report storage, correct field names)
carry forward and are re-grounded in v4.1 above.

---

## References

- History: [`tool-schema-validated-newtype-plan.md`](tool-schema-validated-newtype-plan.md) (v1) / [`tool-schema-source-plan.md`](tool-schema-source-plan.md) (v2)
- codex-rs reference (not ported): `codex-rs/tools/src/json_schema.rs`
- Adjacent crate docs: [`crate-coco-tool.md`](crate-coco-tool.md) / [`crate-coco-tools.md`](crate-coco-tools.md)
- Project rules: [`coco-rs/CLAUDE.md`](../../coco-rs/CLAUDE.md) — "Code Hygiene" + "Error Handling"
- User memory: `project_coco_rs_mcp_tool_input_json_schema.md`
