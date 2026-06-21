//! Google Maps tool.

use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Provider tool ID for Google Maps.
pub const GOOGLE_MAPS_TOOL_ID: &str = "google.google_maps";

/// Create a Google Maps provider tool.
pub fn google_maps() -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool::from_id(GOOGLE_MAPS_TOOL_ID, "google_maps")
}
