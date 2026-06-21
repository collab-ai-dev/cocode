use super::*;
use crate::shared::FileRawData;

#[test]
fn test_file_new() {
    let file = LanguageModelV4File::new("image/png", "base64data");
    assert_eq!(file.media_type, "image/png");
    assert!(matches!(
        file.data,
        LanguageModelV4FileData::Data {
            data: FileRawData::Base64(_)
        }
    ));
    assert!(file.provider_metadata.is_none());
}

#[test]
fn test_file_from_bytes() {
    let file = LanguageModelV4File::from_bytes("image/png", vec![1, 2, 3, 4]);
    assert_eq!(file.media_type, "image/png");
    assert!(matches!(
        file.data,
        LanguageModelV4FileData::Data {
            data: FileRawData::Bytes(_)
        }
    ));
}

#[test]
fn test_file_serialization() {
    let file = LanguageModelV4File::new("application/pdf", "cGRmZGF0YQ==");
    let json = serde_json::to_string(&file).unwrap();
    assert!(json.contains(r#""mediaType":"application/pdf"#));
    assert!(json.contains(r#""data":"cGRmZGF0YQ=="#));
}

#[test]
fn test_file_data_base64() {
    let data = LanguageModelV4FileData::base64("test");
    match data {
        LanguageModelV4FileData::Data {
            data: FileRawData::Base64(s),
        } => {
            assert_eq!(s, "test");
        }
        _ => panic!("Expected Data/Base64 variant"),
    }
}

#[test]
fn test_file_data_bytes() {
    let data = LanguageModelV4FileData::bytes(vec![1, 2, 3]);
    match data {
        LanguageModelV4FileData::Data {
            data: FileRawData::Bytes(b),
        } => {
            assert_eq!(b, vec![1, 2, 3]);
        }
        _ => panic!("Expected Data/Bytes variant"),
    }
}
