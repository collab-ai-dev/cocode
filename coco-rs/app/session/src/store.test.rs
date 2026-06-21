//! Backend-swap proof for the session-store traits.
//!
//! These tests deliberately exercise the store *only* through the trait
//! objects (`Arc<dyn SessionStore>` / `Arc<dyn SessionCatalog>`) and run
//! the same workload against the on-disk and the in-memory backends. If
//! the traits had been silently carved to the disk impl's shape, the RAM
//! backend couldn't satisfy them and these wouldn't compile or pass.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use coco_messages::Message;
use coco_messages::create_assistant_message;
use coco_messages::create_user_message;
use pretty_assertions::assert_eq;

use crate::SessionManager;
use crate::storage::ChainWriteOptions;
use crate::storage::ContentReplacementRecord;
use crate::storage::TranscriptStore;
use crate::store::InMemoryCatalog;
use crate::store::InMemoryStore;
use crate::store::SessionCatalog;
use crate::store::SessionStore;
use crate::store::TranscriptIo;
use crate::store::catalog_for_backend;

/// A tiny two-message conversation (user prompt + assistant reply).
fn sample_conversation() -> Vec<Message> {
    let user = create_user_message("what is 2 + 2?");
    let assistant = create_assistant_message(
        vec![coco_messages::AssistantContent::Text(
            coco_messages::TextContent {
                text: "4".to_string(),
                provider_metadata: None,
            },
        )],
        "mock-model",
        coco_types::TokenUsage::default(),
    );
    vec![user, assistant]
}

fn chain_opts() -> ChainWriteOptions {
    ChainWriteOptions {
        cwd: "/tmp/project".into(),
        timestamp: "2025-01-15T10:00:00Z".into(),
        ..Default::default()
    }
}

/// Append a conversation through the `&dyn SessionStore` boundary (the
/// object-safe `&[&Message]` form), mirroring the per-turn engine call.
fn append_chain(store: &dyn SessionStore, sid: &str, messages: &[Message]) -> usize {
    let refs: Vec<&Message> = messages.iter().collect();
    let mut seen = HashSet::new();
    store
        .append_message_chain(sid, &refs, &mut seen, chain_opts())
        .expect("append_message_chain")
        .appended
}

#[test]
fn test_in_memory_store_round_trips_through_dyn_session_store() {
    // The whole point: a non-disk backend behind the same trait object.
    let store: Arc<dyn SessionStore> = Arc::new(InMemoryStore::new());
    let sid = "mem-session";
    let convo = sample_conversation();

    assert!(!store.exists(sid));
    let appended = append_chain(&*store, sid, &convo);
    assert_eq!(appended, 2, "two messages → two transcript entries");
    assert!(store.exists(sid));

    // Transcript round-trips.
    let messages = store.load_transcript_messages(sid).expect("load messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].entry_type, "user");
    assert_eq!(messages[1].entry_type, "assistant");

    // Metadata derives the same way disk does (shared fold helper).
    let meta = store.read_metadata(sid).expect("read_metadata");
    assert_eq!(meta.first_prompt, "what is 2 + 2?");
    assert_eq!(meta.message_count, 2);
    assert!(!meta.is_sidechain);

    // Usage snapshot round-trips.
    let snapshot = coco_types::SessionUsageSnapshot {
        version: 1,
        session_id: sid.to_string(),
        updated_at_ms: 42,
        ..Default::default()
    };
    store
        .write_usage_snapshot(sid, &snapshot)
        .expect("write usage");
    assert_eq!(
        store.load_usage_snapshot(sid).expect("load usage"),
        Some(snapshot)
    );

    // Content-replacement records round-trip.
    let records = vec![ContentReplacementRecord::tool_result(
        "toolu_1",
        "<persisted>",
    )];
    store
        .insert_content_replacement(sid, None, &records)
        .expect("insert replacement");
    assert_eq!(
        store.load_content_replacements(sid).expect("load repl"),
        records
    );

    // Subagent transcript round-trips.
    let agent_msgs: Vec<Arc<Message>> = convo.iter().cloned().map(Arc::new).collect();
    store
        .append_agent_messages(sid, "agent-1", &agent_msgs)
        .expect("append agent");
    let loaded = store
        .load_agent_messages(sid, "agent-1")
        .expect("load agent")
        .expect("agent transcript present");
    assert_eq!(loaded.len(), 2);

    // Local-only disk affordances are absent on the RAM backend.
    assert_eq!(store.transcript_path(sid), None);
    assert_eq!(store.session_artifact_dir(sid), None);

    // Delete clears it.
    store.delete(sid).expect("delete");
    assert!(!store.exists(sid));
    assert!(
        store
            .load_entries(sid)
            .expect("load after delete")
            .is_empty()
    );
}

/// The same workload through both backends must derive identical
/// *content* metadata — only the fs-stat trio (created/modified/size) may
/// differ. This is the load-bearing "trait isn't disk-shaped" assertion.
#[test]
fn test_disk_and_memory_backends_derive_identical_content_metadata() {
    let sid = "parity-session";
    let convo = sample_conversation();

    // Disk backend.
    let dir = tempfile::tempdir().unwrap();
    let disk_store: Arc<dyn SessionStore> = Arc::new(TranscriptStore::new(Arc::new(
        coco_paths::ProjectPaths::new(dir.path().to_path_buf(), Path::new("/parity")),
    )));
    append_chain(&*disk_store, sid, &convo);
    let disk_meta = disk_store.read_metadata(sid).expect("disk metadata");

    // Memory backend.
    let mem_store: Arc<dyn SessionStore> = Arc::new(InMemoryStore::new());
    append_chain(&*mem_store, sid, &convo);
    let mem_meta = mem_store.read_metadata(sid).expect("mem metadata");

    assert_eq!(disk_meta.first_prompt, mem_meta.first_prompt);
    assert_eq!(disk_meta.message_count, mem_meta.message_count);
    assert_eq!(disk_meta.is_sidechain, mem_meta.is_sidechain);
    assert_eq!(disk_meta.cwd, mem_meta.cwd);
    assert_eq!(disk_meta.git_branch, mem_meta.git_branch);

    // And the chain shape (uuid / kind / parent) matches — both ran the
    // same `build_message_chain_entries`, so only the storage medium
    // differs. (Full struct equality would couple to serde round-trip
    // details like `extra`-map ordering; the chain fields are the
    // semantically load-bearing ones.)
    let shape = |store: &dyn SessionStore| -> Vec<(String, String, Option<String>)> {
        store
            .load_transcript_messages(sid)
            .unwrap()
            .into_iter()
            .map(|t| (t.uuid, t.entry_type, t.parent_uuid))
            .collect()
    };
    assert_eq!(shape(&*disk_store), shape(&*mem_store));
}

#[test]
fn test_session_manager_full_lifecycle_on_memory_backend() {
    // Build the manager via the config selector (the production path).
    let manager = SessionManager::with_backend(
        coco_config::SessionBackend::Memory,
        PathBuf::from("/unused-for-memory"),
    );
    let cwd = Path::new("/tmp/project");
    let session = manager.create("mock-model", cwd).expect("create");
    let id = session.id;

    // Engine-side: write via the store the manager hands out.
    let store = manager.store_for(cwd);
    append_chain(&*store, &id, &sample_conversation());
    store
        .append_metadata(
            &id,
            &crate::storage::MetadataEntry::CustomTitle {
                session_id: id.clone(),
                custom_title: "Arithmetic".to_string(),
            },
        )
        .expect("append title");

    // Manager-side reads route through the same catalog → same state.
    let listed = manager.list().expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);

    let loaded = manager.load(&id).expect("load");
    assert_eq!(loaded.title.as_deref(), Some("Arithmetic"));
    assert_eq!(loaded.message_count, 2);

    let found = manager.find_by_title("arith", false).expect("find");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, id);

    // Delete removes it everywhere.
    manager.delete(&id).expect("delete");
    assert!(manager.list().expect("list empty").is_empty());
    assert!(manager.load(&id).is_err());
}

#[test]
fn test_catalog_for_backend_selects_disk_vs_memory() {
    let dir = tempfile::tempdir().unwrap();

    let disk: Arc<dyn SessionCatalog> =
        catalog_for_backend(coco_config::SessionBackend::Disk, dir.path().to_path_buf());
    let mem: Arc<dyn SessionCatalog> = catalog_for_backend(
        coco_config::SessionBackend::Memory,
        dir.path().to_path_buf(),
    );

    let cwd = Path::new("/tmp/project");
    let sid = "selector-session";
    let convo = sample_conversation();
    append_chain(&*disk.store_for(cwd), sid, &convo);
    append_chain(&*mem.store_for(cwd), sid, &convo);

    // Disk resolves to a real `.jsonl`; memory to a logical handle.
    let disk_resolved = disk
        .resolve(sid, Some(cwd))
        .unwrap()
        .expect("disk resolves");
    assert_eq!(disk_resolved.transcript_path.extension().unwrap(), "jsonl");
    assert!(disk_resolved.transcript_path.exists());

    let mem_resolved = mem.resolve(sid, Some(cwd)).unwrap().expect("mem resolves");
    assert!(mem_resolved.transcript_path.starts_with("memory://"));
    assert_eq!(mem_resolved.project, None);

    // Both surface the session through the backend-agnostic read path.
    assert_eq!(
        disk.read_metadata(sid, Some(cwd))
            .unwrap()
            .unwrap()
            .message_count,
        mem.read_metadata(sid, Some(cwd))
            .unwrap()
            .unwrap()
            .message_count,
    );
}

/// `InMemoryCatalog::with_store` shares one handle so a test (or a future
/// `TeeStore`) can observe what the catalog's consumers wrote.
#[test]
fn test_in_memory_catalog_shares_store_handle() {
    let store = Arc::new(InMemoryStore::new());
    let catalog = InMemoryCatalog::with_store(store.clone());
    let sid = "shared-handle";
    append_chain(
        &*catalog.store_for(Path::new("/x")),
        sid,
        &sample_conversation(),
    );
    // The handle we kept sees the catalog-side write.
    assert!(TranscriptIo::exists(&*store, sid));
}
