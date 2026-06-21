use super::*;

#[test]
fn test_invalid_role_error() {
    let error = MessageConversionError::invalid_role("unknown");
    assert!(error.is_invalid_role());
    assert!(error.to_string().contains("unknown"));
}

#[test]
fn test_missing_field_error() {
    let error = MessageConversionError::missing_field("content");
    assert!(error.is_missing_field());
    assert!(error.to_string().contains("content"));
}

#[test]
fn test_tool_call_id_mismatch_error() {
    let error = MessageConversionError::tool_call_id_mismatch("expected_id", "actual_id");
    match error {
        MessageConversionError::ToolCallIdMismatch { expected, actual } => {
            assert_eq!(expected, "expected_id");
            assert_eq!(actual, "actual_id");
        }
        _ => panic!("Expected ToolCallIdMismatch"),
    }
}

#[test]
fn test_unsupported_type_error() {
    let error = MessageConversionError::unsupported_type("custom_type");
    assert!(error.is_unsupported_type());
}

#[test]
fn test_base64_error() {
    let error = MessageConversionError::base64_error("invalid encoding");
    match error {
        MessageConversionError::Base64Error(msg) => {
            assert_eq!(msg, "invalid encoding");
        }
        _ => panic!("Expected Base64Error"),
    }
}

#[test]
fn test_invalid_image_data_error() {
    let error = MessageConversionError::invalid_image_data("not an image");
    assert!(error.to_string().contains("not an image"));
}