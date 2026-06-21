//! Utility to resolve model paths for the Google API.

/// Get the full model path for a Google API request.
///
/// If the model ID already contains a slash (e.g., "publishers/google/models/gemini-2.0"),
/// it's returned as-is. Otherwise, "models/" is prepended.
pub fn get_model_path(model_id: &str) -> String {
    if model_id.contains('/') {
        model_id.to_string()
    } else {
        format!("models/{model_id}")
    }
}

#[cfg(test)]
#[path = "get_model_path.test.rs"]
mod tests;
