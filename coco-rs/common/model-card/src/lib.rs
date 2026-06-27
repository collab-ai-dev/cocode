//! Vendor-defined model facts (context window, pricing, knowledge cutoff).
//!
//! This crate owns model facts whose update cadence is independent of
//! user-editable `coco-config::ModelInfo`: knowledge cutoffs, pricing, and
//! context limits.
//!
//! The bundled catalog is generated from OpenRouter's `/api/v1/models`
//! response and can be atomically refreshed in memory at runtime. OpenRouter
//! is the single source of truth for pricing. Lookup is intentionally
//! index-based: normalize model IDs into exact lookup keys, reject ambiguous
//! matches, and never infer facts via substring matching.
//!
//! Heuristics (e.g. [`bytes_per_token_for_model`]) live in dedicated
//! submodules and are NOT considered "facts" — they are coarse
//! estimators used only for UX-side approximations (the `/skills`
//! token column). They are explicitly allowed to substring-match
//! because the alternative is an exact-id Set that has to be edited
//! every time Anthropic ships a new generation.

#![forbid(unsafe_code)]

mod bytes_per_token;
mod catalog;
mod resolver;
mod schema;

pub use bytes_per_token::bytes_per_token_for_model;
pub use catalog::LookupResult;
pub use catalog::ModelCardCatalog;
pub use catalog::bundled_catalog;
pub use catalog::display_model_name;
pub use catalog::install_openrouter_snapshot;
pub use catalog::knowledge_cutoff;
pub use catalog::lookup;
pub use catalog::lookup_with_provider;
pub use catalog::pricing;
pub use schema::KnowledgeCutoff;
pub use schema::ModelCard;
pub use schema::ModelCardError;
pub use schema::ModelFamily;
pub use schema::Pricing;

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
