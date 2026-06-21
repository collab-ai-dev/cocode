use std::collections::HashMap;

use serde_json::json;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Computer use tool (2024-10-22) — Basic actions.
pub fn computer_20241022(
    display_width_px: u32,
    display_height_px: u32,
    display_number: u32,
) -> LanguageModelV4ProviderTool {
    let mut args = HashMap::new();
    args.insert("displayWidthPx".into(), json!(display_width_px));
    args.insert("displayHeightPx".into(), json!(display_height_px));
    args.insert("displayNumber".into(), json!(display_number));
    LanguageModelV4ProviderTool {
        id: "anthropic.computer_20241022".into(),
        name: "computer_20241022".into(),
        args,
    }
}

/// Computer use tool (2025-01-24) — Enhanced with hold_key, triple_click, scroll, wait.
pub fn computer_20250124(
    display_width_px: u32,
    display_height_px: u32,
    display_number: u32,
) -> LanguageModelV4ProviderTool {
    let mut args = HashMap::new();
    args.insert("displayWidthPx".into(), json!(display_width_px));
    args.insert("displayHeightPx".into(), json!(display_height_px));
    args.insert("displayNumber".into(), json!(display_number));
    LanguageModelV4ProviderTool {
        id: "anthropic.computer_20250124".into(),
        name: "computer_20250124".into(),
        args,
    }
}

/// Computer use tool (2025-11-24) — Adds zoom action for detailed inspection.
pub fn computer_20251124(
    display_width_px: u32,
    display_height_px: u32,
    display_number: u32,
    enable_zoom: bool,
) -> LanguageModelV4ProviderTool {
    let mut args = HashMap::new();
    args.insert("displayWidthPx".into(), json!(display_width_px));
    args.insert("displayHeightPx".into(), json!(display_height_px));
    args.insert("displayNumber".into(), json!(display_number));
    args.insert("enableZoom".into(), json!(enable_zoom));
    LanguageModelV4ProviderTool {
        id: "anthropic.computer_20251124".into(),
        name: "computer_20251124".into(),
        args,
    }
}
