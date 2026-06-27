use super::*;
use crate::telemetry::TelemetrySettings;

#[test]
fn test_embed_options() {
    let options = EmbedOptions::new("text-embedding-3-small", "Hello").with_dimensions(512);

    assert!(options.model.is_string());
    assert_eq!(options.dimensions, Some(512));
}

#[test]
fn test_embed_options_with_telemetry() {
    let telemetry = TelemetrySettings {
        function_id: Some("embed-fn".to_string()),
        ..Default::default()
    };
    let options = EmbedOptions::new("text-embedding-3-small", "Hello").with_telemetry(telemetry);

    assert!(options.telemetry.is_some());
    assert_eq!(
        options.telemetry.as_ref().unwrap().function_id,
        Some("embed-fn".to_string())
    );
}

#[test]
fn test_embed_many_options() {
    let options = EmbedManyOptions::new(
        "text-embedding-3-small",
        vec!["Hello".to_string(), "World".to_string()],
    );

    assert_eq!(options.values.len(), 2);
}

#[tokio::test]
async fn test_embed_raw_response_propagation() {
    use crate::model::EmbeddingModel;
    use crate::test_utils::MockEmbeddingModel;
    use std::sync::Arc;
    use vercel_ai_provider::EmbeddingModelV4EmbedResult;
    use vercel_ai_provider::EmbeddingUsage;
    use vercel_ai_provider::EmbeddingValue;

    let model = Arc::new(
        MockEmbeddingModel::builder()
            .with_embed_handler(|options| {
                let count = options.values.len();
                let embeddings = (0..count)
                    .map(|_| EmbeddingValue::Dense {
                        vector: vec![1.0, 2.0, 3.0],
                    })
                    .collect();
                Ok(EmbeddingModelV4EmbedResult {
                    embeddings,
                    usage: EmbeddingUsage::new(count as u64),
                    warnings: Vec::new(),
                    provider_metadata: None,
                    raw_response: Some(serde_json::json!({"model": "test"})),
                })
            })
            .build(),
    );

    let options = EmbedOptions {
        model: EmbeddingModel::from_v4(model),
        value: "Hello".to_string(),
        ..Default::default()
    };

    let result = embed(options).await.unwrap();
    assert!(result.raw_response.is_some());
    assert_eq!(result.raw_response.unwrap()["model"], "test");
}
