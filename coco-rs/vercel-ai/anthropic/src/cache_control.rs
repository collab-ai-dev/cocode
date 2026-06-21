use serde_json::Value;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::Warning;

/// Maximum number of cache breakpoints Anthropic allows per request.
const MAX_CACHE_BREAKPOINTS: u32 = 4;

/// Context information for cache control validation.
pub struct CacheContext<'a> {
    /// Description of the context type (e.g., "system message", "thinking block").
    pub type_name: &'a str,
    /// Whether caching is allowed in this context.
    pub can_cache: bool,
}

/// Validates and tracks cache breakpoints across an entire Anthropic request.
///
/// Anthropic allows a maximum of 4 cache breakpoints per request. This validator
/// tracks the count and emits warnings when the limit is exceeded or when
/// cache control is applied to non-cacheable contexts (e.g., thinking blocks).
pub struct CacheControlValidator {
    breakpoint_count: u32,
    warnings: Vec<Warning>,
    /// Resolved `cache_control` value the adapter emits at the built-in/MCP
    /// tool boundary. The engine marks that tool with `cacheBoundary: true`
    /// (it owns the `is_mcp` partition); the TTL is resolved *here* so it
    /// matches the last-user auto-marker exactly. `None` when caching is
    /// inactive — so a no-cache request never gains a lone tool breakpoint.
    tool_boundary_marker: Option<Value>,
}

impl CacheControlValidator {
    pub fn new() -> Self {
        Self {
            breakpoint_count: 0,
            warnings: Vec::new(),
            tool_boundary_marker: None,
        }
    }

    /// Extract and validate cache control from provider metadata.
    ///
    /// Returns `Some(value)` if cache control is valid for this context,
    /// `None` if absent, non-cacheable, or limit exceeded.
    pub fn get_cache_control(
        &mut self,
        provider_metadata: &Option<ProviderMetadata>,
        context: CacheContext<'_>,
    ) -> Option<Value> {
        let cache_control_value = extract_cache_control(provider_metadata)?;
        self.validate_and_track(cache_control_value, context)
    }

    /// Extract and validate cache control from provider options (used for tools).
    ///
    /// `ProviderOptions` has `HashMap<String, HashMap<String, Value>>` structure,
    /// while `ProviderMetadata` has `HashMap<String, Value>`.
    pub fn get_cache_control_from_options(
        &mut self,
        provider_options: &Option<ProviderOptions>,
        context: CacheContext<'_>,
    ) -> Option<Value> {
        let cache_control_value = extract_cache_control_from_options(provider_options);
        let cache_control_value = cache_control_value?;
        self.validate_and_track(cache_control_value, context)
    }

    /// Set the resolved `cache_control` marker emitted at the built-in/MCP
    /// tool boundary (see [`tool_boundary_cache_control`]). Pass the *same*
    /// value used for the last-user auto-marker so the two coco-driven
    /// breakpoints share a TTL; pass `None` to disable boundary caching
    /// (caching off or a non-`Auto` strategy).
    ///
    /// [`tool_boundary_cache_control`]: Self::tool_boundary_cache_control
    pub fn set_tool_boundary_marker(&mut self, marker: Option<Value>) {
        self.tool_boundary_marker = marker;
    }

    /// Emit the boundary `cache_control` for a tool that carries the engine's
    /// `cacheBoundary: true` hint, counted through the shared breakpoint
    /// budget. Returns `None` when the tool is not the boundary, no marker is
    /// configured (caching off), or the 4-breakpoint limit is exhausted.
    pub fn tool_boundary_cache_control(
        &mut self,
        provider_options: &Option<ProviderOptions>,
    ) -> Option<Value> {
        let marker = self.tool_boundary_marker.clone()?;
        let is_boundary = provider_options
            .as_ref()
            .and_then(|po| po.0.get("anthropic"))
            .and_then(|anthropic| anthropic.get("cacheBoundary"))
            .and_then(Value::as_bool)
            == Some(true);
        if !is_boundary {
            return None;
        }
        self.validate_and_track(
            marker,
            CacheContext {
                type_name: "tool cache boundary",
                can_cache: true,
            },
        )
    }

    /// Common validation logic for both metadata and options.
    fn validate_and_track(&mut self, value: Value, context: CacheContext<'_>) -> Option<Value> {
        if !context.can_cache {
            self.warnings.push(Warning::Unsupported {
                feature: "cache_control on non-cacheable context".into(),
                details: Some(format!(
                    "cache_control cannot be set on {}. It will be ignored.",
                    context.type_name
                )),
            });
            return None;
        }

        self.breakpoint_count += 1;
        if self.breakpoint_count > MAX_CACHE_BREAKPOINTS {
            self.warnings.push(Warning::Unsupported {
                feature: "cacheControl breakpoint limit".into(),
                details: Some(format!(
                    "Maximum {MAX_CACHE_BREAKPOINTS} cache breakpoints exceeded (found {}). This breakpoint will be ignored.",
                    self.breakpoint_count
                )),
            });
            return None;
        }

        Some(value)
    }

    /// Consume the validator and return accumulated warnings.
    pub fn into_warnings(self) -> Vec<Warning> {
        self.warnings
    }
}

impl Default for CacheControlValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract cache_control from provider metadata.
/// Allows both `cacheControl` and `cache_control` for flexibility.
fn extract_cache_control(provider_metadata: &Option<ProviderMetadata>) -> Option<Value> {
    let anthropic = provider_metadata
        .as_ref()
        .and_then(|pm| pm.0.get("anthropic"))?;

    // Allow both cacheControl and cache_control
    anthropic
        .get("cacheControl")
        .or_else(|| anthropic.get("cache_control"))
        .cloned()
}

/// Extract cache_control from provider options (tool-level).
/// ProviderOptions has `HashMap<String, HashMap<String, Value>>`.
fn extract_cache_control_from_options(provider_options: &Option<ProviderOptions>) -> Option<Value> {
    let anthropic = provider_options
        .as_ref()
        .and_then(|po| po.0.get("anthropic"))?;

    // Allow both cacheControl and cache_control
    anthropic
        .get("cacheControl")
        .or_else(|| anthropic.get("cache_control"))
        .cloned()
}

#[cfg(test)]
#[path = "cache_control.test.rs"]
mod tests;
