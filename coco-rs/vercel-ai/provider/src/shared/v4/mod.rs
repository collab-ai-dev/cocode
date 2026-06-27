//! Shared types module (V4).
//!
//! Types that are shared across different model types.

mod provider_metadata;
mod provider_options;
mod shared_v4_file_data;
mod warning;

pub use provider_metadata::ProviderMetadata;
pub use provider_options::ProviderOptions;
pub use shared_v4_file_data::FileRawData;
pub use shared_v4_file_data::SharedV4FileData;
pub use warning::Warning;
