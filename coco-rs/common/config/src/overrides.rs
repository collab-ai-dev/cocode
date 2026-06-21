use std::collections::BTreeMap;

use coco_types::Feature;
use coco_types::PermissionMode;
use coco_types::ProviderModelSelection;

/// Mutable state that changes during a session.
/// NOT persisted to config files. Lost when session ends.
///
/// Thinking-level and fast-mode are **not** here because they have
/// their own state holders — `ThinkingLevel` lives per-query on
/// `QueryParams`, and fast-mode runs off the process-wide state in
/// `coco_config::fast_mode`. This struct is only for layered config
/// resolution (the kind that flows into `RuntimeConfig` at build time).
#[derive(Debug, Clone, Default)]
pub struct RuntimeOverrides {
    /// /model command override — validated `provider/model_id` selection
    /// routed through `RuntimeConfigBuilder::with_overrides`.
    pub model_override: Option<ProviderModelSelection>,
    /// --permission-mode CLI override; wins over `settings.permissions.default_mode`.
    pub permission_mode_override: Option<PermissionMode>,
    /// `--fallback-model` CLI overrides. Each entry is a validated
    /// `ProviderModelSelection`; the list is applied in flag order as Main
    /// role's fallback chain. Non-empty values replace any
    /// `settings.models.main.fallbacks` from JSON (CLI wins over
    /// settings for Main fallback).
    pub fallback_model_overrides: Vec<ProviderModelSelection>,
    /// CLI `--enable <feature>` / `--disable <feature>` overrides.
    /// Applied last in `Features::resolve()`, after settings.json + env
    /// vars. Public field so callers can splat with
    /// `..Default::default()`; mutate by inserting directly.
    pub feature_overrides: BTreeMap<Feature, bool>,
}
