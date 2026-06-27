//! Provider options type.

use std::collections::HashMap;

/// Additional provider-specific options.
///
/// They are passed through to the provider from the AI SDK and enable
/// provider-specific functionality that can be fully encapsulated in the provider.
pub type ProviderOptions = HashMap<String, serde_json::Value>;
