# coco-model-card

Vendor-defined model **facts**: pricing, knowledge cutoff, vendor context
window, display name, family, aliases (deps: serde/serde_json only).
Ownership split: this crate holds facts whose update cadence is
independent of user config; `coco-config::ModelInfo` holds the resolved
*operational* config (the context_window/max_output_tokens the user runs
with, sampling, thinking, tool overrides). `vendor_context_window` here
is the vendor max — capacity decisions use `ModelInfo`, never the card.

## Invariant 1 — exact-id lookup, ambiguity rejected

Lookup is index-based over normalized keys (`lookup_key_tiers`): exact
normalized (+ provider-qualified) → providerless → date-stripped. Within
a tier, more than one hit returns `LookupResult::Ambiguous` — **never a
guess, never substring matching**. Convenience fns (`lookup`, `pricing`)
collapse Ambiguous/NotFound to `None`. New spellings are handled by the
resolver (normalization, Anthropic name-order aliases), not by loosening
lookup. `:free` OpenRouter variants get no slug/HF aliases, so they never
answer for the paid canonical model.

*Deliberate exception:* `bytes_per_token_for_model` substring-matches
Claude-family keywords — a UX heuristic for the `/skills` `~N tok` column
only (exact-id sets rot every model generation); real token accounting
uses the live tokenizer in `services/inference`. Don't "fix" it to
exact-id, and don't cite it as precedent for fact lookup.

## Invariant 2 — OpenRouter is the single pricing source

The catalog is generated from OpenRouter `/api/v1/models`
(`data/openrouter-models.json` bundled via `include_str!`; must parse or
first use panics). Pricing is taken **verbatim** — no vendor-side
override layer, so only install snapshots you'd accept for cost
reporting. `install_openrouter_snapshot` atomically swaps the whole
in-memory catalog (`RwLock<Arc<ModelCardCatalog>>`); readers clone the
Arc, so a refresh never tears a lookup. The background refresh lives in
`app/agent-host` (`integrations/model_card_refresh.rs`), gated by
`Feature::DynamicModelCard`. Knowledge cutoffs: the curated table in
`catalog.rs` wins over OpenRouter's sparse/stale field.

## Conventions

`display_model_name` strips the `provider/` prefix but preserves the
configured id spelling — the agent is told the model it actually runs,
not a re-canonicalized slug.
