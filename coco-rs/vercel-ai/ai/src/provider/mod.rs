//! Provider module.
//!
//! This module provides the global default provider pattern.

mod global_provider;

pub use global_provider::clear_default_provider;
pub use global_provider::get_default_provider;
pub use global_provider::has_default_provider;
pub use global_provider::set_default_provider;
