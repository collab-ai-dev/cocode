use super::*;

/// Helper to create a test chunk with default metadata.
fn make_test_chunk(id: &str, filepath: &str, content: &str) -> CodeChunk {
    CodeChunk {
        id: id.to_string(),
        source_id: "test".to_string(),
        filepath: filepath.to_string(),
        language: "rust".to_string(),
        content: content.to_string(),
        start_line: 1,
        end_line: 3,
        embedding: None,
        modified_time: None,
        workspace: "test".to_string(),
        content_hash: String::new(),
        indexed_at: 0,
        parent_symbol: None,
        is_overview: false,
    }
}

/// Helper to create a test chunk with parent symbol context.
fn make_test_chunk_with_parent(id: &str, filepath: &str, content: &str, parent: &str) -> CodeChunk {
    CodeChunk {
        id: id.to_string(),
        source_id: "test".to_string(),
        filepath: filepath.to_string(),
        language: "rust".to_string(),
        content: content.to_string(),
        start_line: 1,
        end_line: 3,
        embedding: None,
        modified_time: None,
        workspace: "test".to_string(),
        content_hash: String::new(),
        indexed_at: 0,
        parent_symbol: Some(parent.to_string()),
        is_overview: false,
    }
}

#[test]
fn test_code_chunk_embedding_content() {
    let chunk = make_test_chunk(
        "test:src/main.rs:0",
        "src/main.rs",
        "fn main() {\n    println!(\"Hello\");\n}",
    );

    let embedding_content = chunk.embedding_content();
    assert!(embedding_content.starts_with("```src/main.rs\n"));
    assert!(embedding_content.ends_with("\n```"));
    assert!(embedding_content.contains("fn main()"));
}

#[test]
fn test_code_chunk_embedding_content_test_file() {
    // Test that test files are properly wrapped
    let chunk = make_test_chunk(
        "test:tests/integration.rs:0",
        "tests/integration.rs",
        "#[test]\nfn test_something() {}",
    );

    let embedding_content = chunk.embedding_content();
    assert!(embedding_content.starts_with("```tests/integration.rs\n"));
    // The embedding model can now understand this is test code
}

#[test]
fn test_wrap_content_for_embedding() {
    let content = wrap_content_for_embedding("src/lib.rs", "pub fn foo() {}");
    assert_eq!(content, "```src/lib.rs\npub fn foo() {}\n```");
}

#[test]
fn test_wrap_content_preserves_multiline() {
    let content =
        wrap_content_for_embedding("src/utils.rs", "fn helper() {\n    // do something\n}");
    assert_eq!(
        content,
        "```src/utils.rs\nfn helper() {\n    // do something\n}\n```"
    );
}

#[test]
fn test_code_chunk_with_metadata() {
    let chunk = CodeChunk {
        id: "ws:file.rs:0".to_string(),
        source_id: "ws".to_string(),
        filepath: "file.rs".to_string(),
        language: "rust".to_string(),
        content: "fn test() {}".to_string(),
        start_line: 1,
        end_line: 1,
        embedding: None,
        modified_time: Some(1700000000),
        workspace: "ws".to_string(),
        content_hash: "abc123".to_string(),
        indexed_at: 1700000100,
        parent_symbol: None,
        is_overview: false,
    };

    assert_eq!(chunk.workspace, "ws");
    assert_eq!(chunk.content_hash, "abc123");
    assert_eq!(chunk.indexed_at, 1700000100);
}

#[test]
fn test_embedding_content_with_parent_symbol() {
    // Test method inside a class/impl
    let chunk = make_test_chunk_with_parent(
        "test:src/user_service.rs:0",
        "src/user_service.rs",
        "fn get_user(&self, id: i64) -> User {\n    self.repo.find(id)\n}",
        "impl UserService",
    );

    let embedding_content = chunk.embedding_content();
    assert!(embedding_content.starts_with("```src/user_service.rs\nimpl UserService ..."));
    assert!(embedding_content.contains("fn get_user(&self"));
    assert!(embedding_content.ends_with("\n```"));
}

#[test]
fn test_embedding_content_without_parent_symbol() {
    // Test top-level function
    let chunk = make_test_chunk(
        "test:src/main.rs:0",
        "src/main.rs",
        "fn main() {\n    println!(\"Hello\");\n}",
    );

    let embedding_content = chunk.embedding_content();
    // Should not have the "..." parent marker
    assert!(!embedding_content.contains("..."));
    assert!(embedding_content.starts_with("```src/main.rs\nfn main()"));
}

#[test]
fn test_calculate_n_final_none() {
    // No context length -> use default
    assert_eq!(calculate_n_final(None), DEFAULT_N_FINAL);
}

#[test]
fn test_calculate_n_final_large_context() {
    // 128k tokens = 64k for retrieval / 512 per chunk = 125 chunks
    // But capped at DEFAULT_N_FINAL (20)
    assert_eq!(calculate_n_final(Some(128_000)), DEFAULT_N_FINAL);
}

#[test]
fn test_calculate_n_final_small_context() {
    // 4k tokens = 2k for retrieval / 512 per chunk = 3 chunks
    assert_eq!(calculate_n_final(Some(4_000)), 3);
}

#[test]
fn test_calculate_n_final_very_small_context() {
    // 512 tokens = 256 for retrieval / 512 per chunk = 0, but min is 1
    assert_eq!(calculate_n_final(Some(512)), 1);
}

#[test]
fn test_calculate_n_final_zero_context() {
    // Zero context -> use default
    assert_eq!(calculate_n_final(Some(0)), DEFAULT_N_FINAL);
}

#[test]
fn test_calculate_n_final_negative_context() {
    // Negative context -> use default
    assert_eq!(calculate_n_final(Some(-1000)), DEFAULT_N_FINAL);
}

#[test]
fn test_search_query_with_context_length() {
    let query = SearchQuery {
        text: "test query".to_string(),
        limit: calculate_n_final(Some(8000)),
        context_length: Some(8000),
        ..Default::default()
    };
    // 8000 tokens = 4000 for retrieval / 512 = 7 chunks
    assert_eq!(query.limit, 7);
    assert_eq!(query.context_length, Some(8000));
}

#[test]
fn test_compute_chunk_hash() {
    let hash1 = compute_chunk_hash("fn main() {}");
    let hash2 = compute_chunk_hash("fn main() {}");
    let hash3 = compute_chunk_hash("fn main() { }"); // different content

    // Same content = same hash
    assert_eq!(hash1, hash2);
    // Different content = different hash
    assert_ne!(hash1, hash3);
    // Full 64-char SHA256 hex
    assert_eq!(hash1.len(), 64);
}

#[test]
fn test_hashes_match() {
    let full_hash = compute_chunk_hash("fn main() {}");
    let short_hash = &full_hash[..16]; // 16-char truncated hash

    // Same length hashes
    assert!(hashes_match(&full_hash, &full_hash));
    assert!(hashes_match(short_hash, short_hash));

    // Different length hashes (16 vs 64 chars)
    assert!(hashes_match(&full_hash, short_hash));
    assert!(hashes_match(short_hash, &full_hash));

    // Non-matching hashes
    let other_hash = compute_chunk_hash("fn bar() {}");
    assert!(!hashes_match(&full_hash, &other_hash));
    assert!(!hashes_match(&full_hash[..16], &other_hash[..16]));
}

#[test]
fn test_chunk_ref_staleness_with_short_hash() {
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.rs");
    let mut file = std::fs::File::create(&file_path).unwrap();
    writeln!(file, "fn foo() {{}}").unwrap();

    // Use 16-char hash (SourceFileId format)
    let expected_content = "fn foo() {}";
    let full_hash = compute_chunk_hash(expected_content);
    let short_hash = full_hash[..16].to_string();

    let chunk_ref = ChunkRef {
        id: "test:test.rs:0".to_string(),
        source_id: "test".to_string(),
        filepath: "test.rs".to_string(),
        language: "rust".to_string(),
        start_line: 1,
        end_line: 1,
        embedding: None,
        workspace: "test".to_string(),
        content_hash: short_hash, // 16-char hash
        indexed_at: 0,
        parent_symbol: None,
        is_overview: false,
    };

    let hydrated = chunk_ref.read_content(dir.path()).unwrap();
    assert!(
        hydrated.is_fresh,
        "16-char hash should match 64-char computed hash"
    );

    // Modify file
    let mut file = std::fs::File::create(&file_path).unwrap();
    writeln!(file, "fn bar() {{}}").unwrap();

    let hydrated = chunk_ref.read_content(dir.path()).unwrap();
    assert!(
        !hydrated.is_fresh,
        "Should detect stale content with 16-char hash"
    );
}

#[test]
fn test_chunk_ref_read_content() {
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.rs");
    let mut file = std::fs::File::create(&file_path).unwrap();
    writeln!(file, "line1").unwrap();
    writeln!(file, "line2").unwrap();
    writeln!(file, "line3").unwrap();
    writeln!(file, "line4").unwrap();

    let chunk_ref = ChunkRef {
        id: "test:test.rs:0".to_string(),
        source_id: "test".to_string(),
        filepath: "test.rs".to_string(),
        language: "rust".to_string(),
        start_line: 2,
        end_line: 3,
        embedding: None,
        workspace: "test".to_string(),
        content_hash: String::new(),
        indexed_at: 0,
        parent_symbol: None,
        is_overview: false,
    };

    let hydrated = chunk_ref.read_content(dir.path()).unwrap();
    assert_eq!(hydrated.content, "line2\nline3");
    assert!(hydrated.is_fresh); // Empty hash = always fresh
}

#[test]
fn test_chunk_ref_staleness_detection() {
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.rs");
    let mut file = std::fs::File::create(&file_path).unwrap();
    writeln!(file, "fn foo() {{}}").unwrap();

    // Compute hash of expected content
    let expected_content = "fn foo() {}";
    let expected_hash = compute_chunk_hash(expected_content);

    let chunk_ref = ChunkRef {
        id: "test:test.rs:0".to_string(),
        source_id: "test".to_string(),
        filepath: "test.rs".to_string(),
        language: "rust".to_string(),
        start_line: 1,
        end_line: 1,
        embedding: None,
        workspace: "test".to_string(),
        content_hash: expected_hash,
        indexed_at: 0,
        parent_symbol: None,
        is_overview: false,
    };

    let hydrated = chunk_ref.read_content(dir.path()).unwrap();
    assert!(hydrated.is_fresh);

    // Now modify the file
    let mut file = std::fs::File::create(&file_path).unwrap();
    writeln!(file, "fn bar() {{}}").unwrap();

    let hydrated = chunk_ref.read_content(dir.path()).unwrap();
    assert!(!hydrated.is_fresh); // Content changed, hash doesn't match
}

#[test]
fn test_chunk_ref_to_code_chunk() {
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("main.rs");
    let mut file = std::fs::File::create(&file_path).unwrap();
    writeln!(file, "fn main() {{").unwrap();
    writeln!(file, "    println!(\"hello\");").unwrap();
    writeln!(file, "}}").unwrap();

    let chunk_ref = ChunkRef {
        id: "test:main.rs:0".to_string(),
        source_id: "test".to_string(),
        filepath: "main.rs".to_string(),
        language: "rust".to_string(),
        start_line: 1,
        end_line: 3,
        embedding: None,
        workspace: "test".to_string(),
        content_hash: String::new(),
        indexed_at: 12345,
        parent_symbol: Some("mod main".to_string()),
        is_overview: false,
    };

    let code_chunk = chunk_ref.to_code_chunk(dir.path()).unwrap();
    assert_eq!(code_chunk.id, "test:main.rs:0");
    assert_eq!(code_chunk.filepath, "main.rs");
    assert!(code_chunk.content.contains("fn main()"));
    assert_eq!(code_chunk.parent_symbol, Some("mod main".to_string()));
}

#[test]
fn test_code_chunk_to_chunk_ref() {
    let chunk = CodeChunk {
        id: "ws:file.rs:0".to_string(),
        source_id: "ws".to_string(),
        filepath: "file.rs".to_string(),
        language: "rust".to_string(),
        content: "fn test() {}".to_string(),
        start_line: 1,
        end_line: 1,
        embedding: Some(vec![0.1, 0.2]),
        modified_time: Some(1700000000),
        workspace: "ws".to_string(),
        content_hash: "abc123".to_string(),
        indexed_at: 1700000100,
        parent_symbol: Some("impl Foo".to_string()),
        is_overview: false,
    };

    let chunk_ref: ChunkRef = chunk.into();
    assert_eq!(chunk_ref.id, "ws:file.rs:0");
    assert_eq!(chunk_ref.filepath, "file.rs");
    assert_eq!(chunk_ref.content_hash, "abc123");
    assert_eq!(chunk_ref.embedding, Some(vec![0.1, 0.2]));
    assert_eq!(chunk_ref.parent_symbol, Some("impl Foo".to_string()));
    // Note: content is NOT in ChunkRef
}
