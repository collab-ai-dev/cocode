use super::*;

#[test]
fn deserialize_image_response() {
    let json = r#"{
        "data": [
            {
                "b64_json": "aGVsbG8=",
                "revised_prompt": "A beautiful sunset"
            }
        ],
        "created": 1700000000
    }"#;
    let resp: OpenAICompatibleImageResponse =
        serde_json::from_str(json).expect("should deserialize");
    assert_eq!(resp.data.len(), 1);
    assert_eq!(resp.data[0].b64_json.as_deref(), Some("aGVsbG8="));
}
