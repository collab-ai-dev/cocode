use super::*;
use crate::schedule_store::ScheduleStore;

fn store_in(dir: &std::path::Path) -> DiskBackedScheduleStore {
    DiskBackedScheduleStore::new(dir.join(".coco").join("scheduled_tasks.json"))
}

#[tokio::test]
async fn durable_task_persists_to_disk_without_runtime_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_in(tmp.path());

    let task = store
        .add_cron_task("0 9 * * *", "standup", true, /*durable*/ true, None)
        .await
        .unwrap();

    let path = tmp.path().join(".coco").join("scheduled_tasks.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"prompt\": \"standup\""), "got: {raw}");
    assert!(raw.contains("\"createdAt\""), "camelCase on disk: {raw}");
    // Runtime-only fields are stripped on write.
    assert!(!raw.contains("durable"), "durable must not hit disk: {raw}");
    assert!(!raw.contains("agentId"), "agentId must not hit disk: {raw}");

    // Reloads from disk.
    let listed = store.list_all_cron_tasks().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, task.id);
    assert_eq!(listed[0].durable, None); // file tasks read back as durable (None)
}

#[tokio::test]
async fn session_task_is_memory_only() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_in(tmp.path());

    store
        .add_cron_task("0 9 * * *", "ping", false, /*durable*/ false, None)
        .await
        .unwrap();

    // Not written to disk.
    assert!(
        !tmp.path()
            .join(".coco")
            .join("scheduled_tasks.json")
            .exists()
    );
    // But visible in the merged list, marked durable=Some(false).
    let listed = store.list_all_cron_tasks().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].durable, Some(false));
}

#[tokio::test]
async fn mark_fired_and_remove_round_trip_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_in(tmp.path());
    let task = store
        .add_cron_task("0 * * * *", "hourly", true, true, None)
        .await
        .unwrap();

    store.mark_cron_tasks_fired(&[&task.id], 42).await.unwrap();
    let listed = store.list_all_cron_tasks().await.unwrap();
    assert_eq!(listed[0].last_fired_at, Some(42));

    store.remove_cron_tasks(&[&task.id]).await.unwrap();
    assert!(store.list_all_cron_tasks().await.unwrap().is_empty());
}

#[tokio::test]
async fn missing_and_corrupt_files_degrade_to_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_in(tmp.path());
    // Missing file.
    assert!(store.list_all_cron_tasks().await.unwrap().is_empty());

    // Corrupt JSON.
    let path = tmp.path().join(".coco").join("scheduled_tasks.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ not json").unwrap();
    assert!(store.list_all_cron_tasks().await.unwrap().is_empty());
}

#[tokio::test]
async fn invalid_cron_rows_are_dropped_on_read() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".coco").join("scheduled_tasks.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"tasks":[
            {"id":"good","cron":"0 9 * * *","prompt":"ok","createdAt":1},
            {"id":"bad","cron":"99 99 * * *","prompt":"nope","createdAt":2}
        ]}"#,
    )
    .unwrap();
    let store = store_in(tmp.path());
    let listed = store.list_all_cron_tasks().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "good");
}
