use super::*;
use tempfile::TempDir;

async fn setup() -> (TempDir, RepoMapCache) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(SqliteStore::open(&db_path).unwrap());
    let cache = RepoMapCache::new(store);
    (dir, cache)
}

fn make_tag(name: &str, line: i32, is_def: bool) -> CodeTag {
    CodeTag {
        name: name.to_string(),
        kind: TagKind::Function,
        start_line: line,
        end_line: line + 10,
        start_byte: line * 100,
        end_byte: (line + 10) * 100,
        signature: Some(format!("fn {}()", name)),
        docs: None,
        is_definition: is_def,
    }
}

#[tokio::test]
async fn test_tag_cache() {
    let (_dir, cache) = setup().await;

    // Initially empty
    let result = cache.get_tags("test.rs").await.unwrap();
    assert!(result.is_none());

    // Store tags (None = no optimistic lock check)
    let tags = vec![make_tag("foo", 10, true), make_tag("bar", 20, false)];
    let written = cache.put_tags("test.rs", &tags, None).await.unwrap();
    assert!(written);

    // Retrieve tags
    let result = cache.get_tags("test.rs").await.unwrap();
    assert!(result.is_some());
    let cached_tags = result.unwrap();
    assert_eq!(cached_tags.len(), 2);

    // Verify all fields round-trip correctly for first tag
    assert_eq!(cached_tags[0].name, "foo");
    assert!(cached_tags[0].is_definition);
    assert_eq!(cached_tags[0].kind, TagKind::Function);
    assert_eq!(cached_tags[0].start_line, 10);
    assert_eq!(cached_tags[0].end_line, 20);
    assert_eq!(cached_tags[0].start_byte, 1000);
    assert_eq!(cached_tags[0].end_byte, 2000);
    assert_eq!(cached_tags[0].signature, Some("fn foo()".to_string()));

    // Verify second tag
    assert_eq!(cached_tags[1].name, "bar");
    assert!(!cached_tags[1].is_definition);
    assert_eq!(cached_tags[1].signature, Some("fn bar()".to_string()));

    // Invalidate
    cache.invalidate_tags("test.rs").await.unwrap();
    let result = cache.get_tags("test.rs").await.unwrap();
    assert!(result.is_none());
}
