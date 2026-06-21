//! Registry module for provider management.
//!
//! This module provides a registry for managing multiple providers and
//! creating custom providers with specific model configurations.

mod custom_provider;
mod no_such_provider_error;
mod provider_registry;

pub use custom_provider::CustomProviderOptions;
pub use custom_provider::custom_provider;
pub use no_such_provider_error::NoSuchProviderError;
pub use provider_registry::ProviderRegistry;
pub use provider_registry::ProviderRegistryOptions;
pub use provider_registry::create_provider_registry;
