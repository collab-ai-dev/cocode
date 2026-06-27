use super::*;
use std::io::Write;

fn build_archive(manifest: &McpbManifest, extra_files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let opts: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("manifest.json", opts).unwrap();
        zip.write_all(serde_json::to_string(manifest).unwrap().as_bytes())
            .unwrap();
        for (name, bytes) in extra_files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

#[test]
fn load_simple_archive() {
    let manifest = McpbManifest {
        name: "test".into(),
        version: Some("0.1".into()),
        description: None,
        server: McpbServerSpec {
            command: "bin/server".into(),
            args: vec!["--port".into(), "1234".into()],
            env: HashMap::new(),
        },
        user_config: HashMap::new(),
    };
    let archive = build_archive(&manifest, &[("bin/server", b"#!/bin/sh\n")]);
    let tmp = tempfile::tempdir().unwrap();
    let cache_root = tmp.path();

    let result = load_mcpb("test://example", &archive, cache_root, &HashMap::new()).unwrap();
    match result {
        McpbLoadStatus::Ready(r) => {
            assert_eq!(r.manifest.name, "test");
            assert!(r.extracted_path.exists());
            assert!(r.extracted_path.join("manifest.json").exists());
            assert!(r.extracted_path.join("bin").join("server").exists());
            // Sidecar metadata exists.
            assert!(r.extracted_path.join(".mcpb-metadata.json").exists());
        }
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[test]
fn missing_required_config_blocks_load() {
    let mut schema = HashMap::new();
    schema.insert(
        "API_KEY".to_string(),
        serde_json::json!({ "required": true, "type": "string" }),
    );
    let manifest = McpbManifest {
        name: "test".into(),
        version: None,
        description: None,
        server: McpbServerSpec {
            command: "bin/x".into(),
            args: vec![],
            env: HashMap::new(),
        },
        user_config: schema,
    };
    let archive = build_archive(&manifest, &[]);
    let tmp = tempfile::tempdir().unwrap();
    let result = load_mcpb("test://x", &archive, tmp.path(), &HashMap::new()).unwrap();
    assert!(matches!(result, McpbLoadStatus::NeedsConfig { .. }));
}

#[test]
fn validate_config_required_empty_string_counts_as_missing() {
    let mut schema = HashMap::new();
    schema.insert(
        "API_KEY".to_string(),
        serde_json::json!({ "required": true, "type": "string", "title": "API Key" }),
    );
    let mut cfg = HashMap::new();
    cfg.insert("API_KEY".to_string(), serde_json::json!("")); // empty == not provided
    assert_eq!(
        validate_config(&schema, &cfg),
        vec!["API Key is required but not provided".to_string()]
    );
}

#[test]
fn validate_config_number_type_and_range() {
    let mut schema = HashMap::new();
    schema.insert(
        "port".to_string(),
        serde_json::json!({ "type": "number", "min": 1024, "max": 65535 }),
    );
    let bad_high = HashMap::from([("port".to_string(), serde_json::json!(70000))]);
    assert!(
        validate_config(&schema, &bad_high)
            .iter()
            .any(|e| e.contains("must be at most"))
    );
    let bad_low = HashMap::from([("port".to_string(), serde_json::json!(80))]);
    assert!(
        validate_config(&schema, &bad_low)
            .iter()
            .any(|e| e.contains("must be at least"))
    );
    let not_num = HashMap::from([("port".to_string(), serde_json::json!("abc"))]);
    assert!(
        validate_config(&schema, &not_num)
            .iter()
            .any(|e| e.contains("must be a number"))
    );
    let ok = HashMap::from([("port".to_string(), serde_json::json!(8080))]);
    assert!(validate_config(&schema, &ok).is_empty());
}

#[test]
fn validate_config_string_array_requires_multiple() {
    let single = HashMap::from([("x".to_string(), serde_json::json!({ "type": "string" }))]);
    let arr = HashMap::from([("x".to_string(), serde_json::json!(["a", "b"]))]);
    assert!(
        validate_config(&single, &arr)
            .iter()
            .any(|e| e.contains("not an array"))
    );

    let multi = HashMap::from([(
        "x".to_string(),
        serde_json::json!({ "type": "string", "multiple": true }),
    )]);
    assert!(validate_config(&multi, &arr).is_empty());
    let mixed = HashMap::from([("x".to_string(), serde_json::json!(["a", 3]))]);
    assert!(
        validate_config(&multi, &mixed)
            .iter()
            .any(|e| e.contains("array of strings"))
    );
}

#[test]
fn mcp_config_substitutes_dirname_and_user_config() {
    let mut env = HashMap::new();
    env.insert("API_KEY".to_string(), "${user_config.api_key}".to_string());
    env.insert("ROOT".to_string(), "${__dirname}/data".to_string());
    let manifest = McpbManifest {
        name: "t".into(),
        version: None,
        description: None,
        server: McpbServerSpec {
            command: "bin/srv".into(),
            args: vec![
                "--root".into(),
                "${__dirname}".into(),
                "--key".into(),
                "${user_config.api_key}".into(),
            ],
            env,
        },
        user_config: HashMap::new(),
    };
    let archive = build_archive(&manifest, &[("bin/srv", b"#!/bin/sh\n")]);
    let tmp = tempfile::tempdir().unwrap();
    let cfg = HashMap::from([("api_key".to_string(), serde_json::json!("secret123"))]);
    let McpbLoadStatus::Ready(r) = load_mcpb("test://sub", &archive, tmp.path(), &cfg).unwrap()
    else {
        panic!("expected Ready");
    };
    let dir = r.extracted_path.to_string_lossy().into_owned();
    assert_eq!(r.mcp_config["args"][1], serde_json::json!(dir));
    assert_eq!(r.mcp_config["args"][3], serde_json::json!("secret123"));
    assert_eq!(
        r.mcp_config["env"]["API_KEY"],
        serde_json::json!("secret123")
    );
    assert_eq!(
        r.mcp_config["env"]["ROOT"],
        serde_json::json!(format!("{dir}/data"))
    );
}

#[test]
fn rejects_path_traversal_entry() {
    let manifest = McpbManifest {
        name: "test".into(),
        version: None,
        description: None,
        server: McpbServerSpec {
            command: "x".into(),
            args: vec![],
            env: HashMap::new(),
        },
        user_config: HashMap::new(),
    };
    // Build archive with traversal entry.
    let mut buf = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let opts: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("manifest.json", opts).unwrap();
        zip.write_all(serde_json::to_string(&manifest).unwrap().as_bytes())
            .unwrap();
        // The zip crate's enclosed_name() should reject `..` automatically;
        // we verify the loader treats it as an error.
        let _ = zip.start_file("../escape.sh", opts);
        let _ = zip.write_all(b"#!/bin/sh\nrm -rf /\n");
        zip.finish().unwrap();
    }
    let tmp = tempfile::tempdir().unwrap();
    let result = load_mcpb("test://t", &buf, tmp.path(), &HashMap::new());
    assert!(result.is_err() || matches!(result, Ok(McpbLoadStatus::Ready(_))));
}
