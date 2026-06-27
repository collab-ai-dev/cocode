//! Test-only helpers exposed across the integration-test seam.
//!
//! Integration tests in `tests/` only see the crate's public API, so
//! crate-internal pipeline pieces (the queue → history drain, the
//! per-item attachment conversion) need a thin `pub` re-export to be
//! reachable for end-to-end coverage. Production code never goes
//! through this module — direct callers route through `engine` /
//! `engine_finalize_turn` which still use the `pub(crate)` paths.
//!
//! Keep this module minimal: it's an integration-test seam, not a
//! second public API.

pub use crate::helpers::drain_command_queue_into_history as drain_into_history;
pub use crate::helpers::queued_command_to_attachment;

use std::sync::Arc;

use coco_inference::LanguageModel;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::PrebuiltLanguageModelSlot;
use coco_inference::RetryConfig;
use coco_types::ModelRole;

pub fn model_runtime_registry(model: Arc<dyn LanguageModel>) -> Arc<ModelRuntimeRegistry> {
    Arc::new(ModelRuntimeRegistry::from_prebuilt_language_model(
        ModelRole::Main,
        PrebuiltLanguageModelSlot::new(model, RetryConfig::default()),
    ))
}

pub fn model_runtime_registry_with_fallback(
    primary: Arc<dyn LanguageModel>,
    fallback: Arc<dyn LanguageModel>,
) -> Arc<ModelRuntimeRegistry> {
    Arc::new(ModelRuntimeRegistry::from_prebuilt_language_models(
        ModelRole::Main,
        PrebuiltLanguageModelSlot::new(primary, RetryConfig::default()),
        vec![PrebuiltLanguageModelSlot::new(
            fallback,
            RetryConfig::default(),
        )],
    ))
}
