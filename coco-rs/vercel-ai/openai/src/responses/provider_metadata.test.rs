use super::*;

#[test]
fn builds_empty_metadata() {
    assert!(build_responses_provider_metadata(None, None).is_none());
}

#[test]
fn builds_with_response_id() {
    let meta = build_responses_provider_metadata(Some("resp_123"), None).expect("should be Some");
    assert_eq!(meta.0["openai"]["responseId"], "resp_123");
}

#[test]
fn builds_with_both() {
    let meta =
        build_responses_provider_metadata(Some("resp_123"), Some("flex")).expect("should be Some");
    assert_eq!(meta.0["openai"]["responseId"], "resp_123");
    assert_eq!(meta.0["openai"]["serviceTier"], "flex");
}

#[test]
fn reasoning_metadata_round_trips() {
    // None blob → no metadata (plain reasoning stays metadata-free).
    assert!(build_reasoning_provider_metadata(None).is_none());
    assert!(build_reasoning_provider_metadata(Some(&serde_json::Value::Null)).is_none());

    // Some blob → builder/reader symmetry under the shared key.
    let blob = serde_json::Value::String("ENC_BLOB".into());
    let meta = build_reasoning_provider_metadata(Some(&blob)).expect("should be Some");
    assert_eq!(
        meta.0["openai"][REASONING_ENCRYPTED_CONTENT_KEY],
        "ENC_BLOB"
    );
    assert_eq!(reasoning_encrypted_content(&meta), Some(&blob));
}

#[test]
fn compaction_metadata_round_trips_through_one_struct() {
    // Writer and reader both go through `ResponsesCompactionProviderMetadata`,
    // so the camelCase wire keys can't drift between capture and sendback.
    let meta = build_compaction_provider_metadata("itm_1", Some("ENC"));
    assert_eq!(meta.0["openai"]["type"], "compaction");
    assert_eq!(meta.0["openai"]["itemId"], "itm_1");
    assert_eq!(meta.0["openai"]["encryptedContent"], "ENC");

    let read = read_compaction_provider_metadata(&meta).expect("decodes");
    assert_eq!(read.meta_type, "compaction");
    assert_eq!(read.item_id, "itm_1");
    assert_eq!(read.encrypted_content.as_deref(), Some("ENC"));
}

#[test]
fn compaction_reader_defaults_type_and_tolerates_missing_blob() {
    // A blob-less compaction item omits `encryptedContent`; the reader still
    // decodes and defaults `type` to "compaction".
    let meta = build_compaction_provider_metadata("itm_2", None);
    assert!(meta.0["openai"].get("encryptedContent").is_none());
    let read = read_compaction_provider_metadata(&meta).expect("decodes");
    assert_eq!(read.meta_type, "compaction");
    assert_eq!(read.encrypted_content, None);
}

#[test]
fn raw_reasoning_segment_id_appends_suffix() {
    assert_eq!(raw_reasoning_segment_id("rs_1"), "rs_1::content");
    assert!(raw_reasoning_segment_id("rs_1").ends_with(RAW_REASONING_ID_SUFFIX));
}
