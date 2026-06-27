use std::collections::HashMap;

use serde_json::json;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Options for creating an image_generation provider tool.
#[derive(Default)]
pub struct ImageGenerationToolOptions {
    pub background: Option<String>,
    pub input_fidelity: Option<String>,
    pub input_image_mask: Option<serde_json::Value>,
    pub model: Option<String>,
    pub moderation: Option<String>,
    pub output_compression: Option<u32>,
    pub output_format: Option<String>,
    pub partial_images: Option<u32>,
    pub quality: Option<String>,
    pub size: Option<String>,
}

/// Create an image_generation provider tool for the Responses API.
pub fn openai_image_generation_tool(
    options: ImageGenerationToolOptions,
) -> LanguageModelV4ProviderTool {
    let mut args: HashMap<String, serde_json::Value> = HashMap::new();
    if let Some(ref bg) = options.background {
        args.insert("background".into(), json!(bg));
    }
    if let Some(ref fidelity) = options.input_fidelity {
        args.insert("input_fidelity".into(), json!(fidelity));
    }
    if let Some(mask) = options.input_image_mask {
        args.insert("input_image_mask".into(), mask);
    }
    if let Some(ref m) = options.model {
        args.insert("model".into(), json!(m));
    }
    if let Some(ref mod_) = options.moderation {
        args.insert("moderation".into(), json!(mod_));
    }
    if let Some(compression) = options.output_compression {
        args.insert("output_compression".into(), json!(compression));
    }
    if let Some(ref fmt) = options.output_format {
        args.insert("output_format".into(), json!(fmt));
    }
    if let Some(partial) = options.partial_images {
        args.insert("partial_images".into(), json!(partial));
    }
    if let Some(ref q) = options.quality {
        args.insert("quality".into(), json!(q));
    }
    if let Some(ref s) = options.size {
        args.insert("size".into(), json!(s));
    }
    LanguageModelV4ProviderTool {
        id: "openai.image_generation".into(),
        name: "image_generation".into(),
        args,
    }
}
