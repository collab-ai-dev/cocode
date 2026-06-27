# coco-llm-types

Pure DTO re-export crate for `vercel-ai-provider`. The DTO seam of the
dual-seam architecture (the other being `services/inference` for the
runtime seam).

## Scope

**Re-exports only.** No logic, no client construction, no auth, no
retry, no prompt-cache, no thinking conversion. Anything that runs
during a request belongs in `services/inference`.

A type belongs here iff a Message-family DTO field or domain code names
its shape. Variant-internal sub-types (`ReasoningFilePart`,
`SharedV4FileData`-only-by-vercel-ai-internal) that are only reached
via `match` patterns are not re-exported until a caller actually names
them.

## What's exposed

| Group | Names |
|-------|-------|
| Message envelope | `LlmMessage` (= `LanguageModelV4Message`), `LlmPrompt` |
| Role-keyed content envelopes | `AssistantContentPart`, `ToolContentPart`, `UserContentPart` |
| Content parts | `DataContent`, `FilePart`, `ReasoningPart`, `SharedV4FileData`, `TextPart`, `ToolCallPart`, `ToolResultContent`, `ToolResultContentPart`, `ToolResultPart` |
| Provider-extension bags | `ProviderMetadata`, `ProviderOptions` |
| Result-side DTOs | `FinishReason`, `ReasoningLevel`, `ResponseFormat`, `ResponseMetadata`, `StopReason` (= `UnifiedFinishReason`), `Usage` |

`StopReason` is the workspace-canonical name (transcript JSON field is
`stop_reason`); `UnifiedFinishReason` lives only inside
`vercel-ai-provider`.

## Architecture

```
vercel-ai-provider (3rd-party SDK)
   ├── common/llm-types          ← DTO seam (this crate)
   │     re-exports message + content + finish/usage/metadata
   │     consumers: coco-types (Message family), coco-messages,
   │                services/inference (DTOs for inputs / results),
   │                services/compact, app/query, app/cli, app/tui,
   │                tests/harness, tests/live
   │
   └── services/inference        ← runtime/client seam
         owns: LanguageModelV4 trait, CallOptions, GenerateResult,
               StreamResult, Provider trait, model runtime, retry, auth,
               prompt-cache detection, thinking-level conversion
         consumers: app/query, app/cli, model_factory
```

## SDK upgrade story

v4 → v5 edits **two files**:
1. `common/llm-types/src/lib.rs` — point aliases at v5 names
2. `services/inference/src/lib.rs` — point runtime aliases at v5 names

Everything else stays unchanged.

## Why two crates, not one

Conflating DTOs and runtime would force one of:
- Schema-only consumers (like `coco-messages` ops, transcript persistence,
  `coco-compact`) to compile the client+retry+auth machinery.
- Runtime callers to import a giant "everything" crate.

Two narrow seams keeps `coco-messages` ops at ~50 LOC of vercel-ai dep
weight and stays decoupled from `services/inference` runtime changes.

## Seam enforcement

`scripts/check-vercel-ai-seam.sh` allows direct `vercel-ai*` Cargo deps
only in:
- `common/llm-types/Cargo.toml`
- `services/inference/Cargo.toml`
- Workspace root + `vercel-ai/*` crates themselves

Any other Cargo.toml declaring a `vercel-ai*` dep fails the gate.

## Naming policy

- DTO command authority lives here. If a vercel-ai type is renamed for
  coco-rs (`LanguageModelV4Message as LlmMessage`,
  `UnifiedFinishReason as StopReason`), the rename happens here once;
  no other crate may re-rename. inference does NOT re-export DTOs
  under different names.
- Types whose vercel-ai name carries a version digit are renamed to
  strip the digit (`LlmMessage`, `LlmPrompt`, `StopReason`). Types
  without a version digit (`TextPart`, `FilePart`, `Usage`,
  `FinishReason`) pass through unchanged.
