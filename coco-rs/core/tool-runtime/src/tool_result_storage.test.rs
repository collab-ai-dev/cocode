use super::*;
use pretty_assertions::assert_eq;

#[test]
fn declared_bounds_are_authoritative() {
    // No hidden clamp: a tool that declares above the default keeps its
    // declared threshold (WebFetch relies on this for preapproved verbatim).
    assert_eq!(ResultSizeBound::Bytes(102_000).as_bytes(), Some(102_000));
    assert_eq!(
        DEFAULT_TOOL_MAX_RESULT_SIZE_BOUND,
        ResultSizeBound::Bytes(DEFAULT_MAX_RESULT_SIZE_BYTES)
    );
    assert!(ResultSizeBound::Unbounded.as_bytes().is_none());
    assert!(ResultSizeBound::try_bytes(0).is_none());
    assert!(ResultSizeBound::try_bytes(-1).is_none());
}

#[tokio::test]
async fn write_artifact_tooluse_create_new_keeps_first() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = ToolOutputStore::new(tmp.path());
    let key = ArtifactKey::ToolUse {
        id: "abc".into(),
        is_json: false,
    };
    let p1 = store.write_artifact(&key, "first").await.unwrap();
    let p2 = store.write_artifact(&key, "second").await.unwrap();
    assert_eq!(p1, p2);
    assert_eq!(p1, tool_results_dir(tmp.path()).join("abc.txt"));
    // create_new: the globally-unique id ⟹ same bytes, so the first write wins.
    assert_eq!(std::fs::read_to_string(&p1).unwrap(), "first");
}

#[tokio::test]
async fn write_artifact_named_atomic_last_writer_wins_and_leaves_no_tmp() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = ToolOutputStore::new(tmp.path());
    let key = ArtifactKey::Named {
        file_name: "url-x-abc0123456-def45678.md".into(),
    };
    store.write_artifact(&key, "one").await.unwrap();
    let p = store.write_artifact(&key, "two").await.unwrap();
    // Content-addressed names dedup, so last-writer-wins is safe.
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
    let tmp_files: Vec<_> = std::fs::read_dir(tool_results_dir(tmp.path()))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(tmp_files.is_empty());
}

#[tokio::test]
async fn write_artifact_rejects_invalid_named_key() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = ToolOutputStore::new(tmp.path());
    let key = ArtifactKey::Named {
        file_name: "../escape".into(),
    };
    assert!(store.write_artifact(&key, "x").await.is_err());
}

#[test]
fn artifact_key_validation() {
    let ok = |name: &str| ArtifactKey::Named {
        file_name: name.into(),
    };
    assert!(
        ArtifactKey::ToolUse {
            id: "anything/weird".into(),
            is_json: false
        }
        .validate()
        .is_ok()
    );
    assert!(ok("url-docs.rs-a1b2c3-9f8e.md").validate().is_ok());
    assert!(ok(".hidden").validate().is_err());
    assert!(ok("has space").validate().is_err());
    assert!(ok("path/slash").validate().is_err());
    assert!(ok("").validate().is_err());
    assert!(ok(&"a".repeat(101)).validate().is_err());
}

#[tokio::test]
async fn persist_binary_uses_mime_extension_and_is_idempotent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = ToolOutputStore::new(tmp.path());
    let first = b"\x89PNG\r\n\x1a\nfirst";
    let second = b"second";
    let result = store
        .persist_binary("mcp-1", first, Some("image/png"))
        .await
        .unwrap();
    assert_eq!(
        result.filepath,
        tool_results_dir(tmp.path()).join("mcp-1.png")
    );
    assert_eq!(result.original_size, first.len() as i64);
    assert_eq!(std::fs::read(&result.filepath).unwrap(), first);

    let second_result = store
        .persist_binary("mcp-1", second, Some("image/png; charset=binary"))
        .await
        .unwrap();
    assert_eq!(second_result.original_size, first.len() as i64);
    assert_eq!(std::fs::read(&result.filepath).unwrap(), first);
}

#[test]
fn extension_for_mime_type_table() {
    assert_eq!(extension_for_mime_type(Some("image/png")), "png");
    assert_eq!(
        extension_for_mime_type(Some("image/jpeg; charset=binary")),
        "jpg"
    );
    assert_eq!(extension_for_mime_type(Some("image/svg+xml")), "svg");
    assert_eq!(extension_for_mime_type(Some("application/zip")), "zip");
    assert_eq!(
        extension_for_mime_type(Some("application/octet-stream")),
        "bin"
    );
    assert_eq!(extension_for_mime_type(None), "bin");
}

#[test]
fn test_render_mcp_binary_reference_includes_filepath_and_mime() {
    let p = PersistedMcpBinaryOutput {
        filepath: PathBuf::from("/sess/tool-results/mcp-1.pdf"),
        original_size: 4_096,
        mime_type: "application/pdf".into(),
    };
    let rendered = render_mcp_binary_reference(&p);
    assert!(rendered.starts_with(PERSISTED_OUTPUT_TAG));
    assert!(rendered.ends_with(PERSISTED_OUTPUT_CLOSING_TAG));
    assert!(rendered.contains("MCP output is binary"));
    assert!(rendered.contains("4KB"));
    assert!(rendered.contains("application/pdf"));
    assert!(rendered.contains("/sess/tool-results/mcp-1.pdf"));
}
