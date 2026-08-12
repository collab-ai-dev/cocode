use super::*;
use crate::schedule_store::ScheduleStore;

fn store_in(dir: &std::path::Path) -> DiskBackedScheduleStore {
    DiskBackedScheduleStore::new(
        dir.join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("scheduled_tasks.json"),
    )
}

#[tokio::test]
async fn durable_task_persists_to_disk_without_runtime_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_in(tmp.path());

    let task = store
        .add_cron_task(
            "0 9 * * *",
            CronPayload::prompt("standup"),
            true,
            /*durable*/ true,
            None,
        )
        .await
        .unwrap();

    let path = tmp
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("scheduled_tasks.json");
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
        .add_cron_task(
            "0 9 * * *",
            CronPayload::prompt("ping"),
            false,
            /*durable*/ false,
            None,
        )
        .await
        .unwrap();

    // Not written to disk.
    assert!(
        !tmp.path()
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
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
        .add_cron_task("0 * * * *", CronPayload::prompt("hourly"), true, true, None)
        .await
        .unwrap();

    store.mark_cron_tasks_fired(&[&task.id], 42).await.unwrap();
    let listed = store.list_all_cron_tasks().await.unwrap();
    assert_eq!(listed[0].last_fired_at, Some(42));

    store.remove_cron_tasks(&[&task.id]).await.unwrap();
    assert!(store.list_all_cron_tasks().await.unwrap().is_empty());
}

#[tokio::test]
async fn missing_file_is_empty_but_corrupt_file_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_in(tmp.path());
    // Missing file.
    assert!(store.list_all_cron_tasks().await.unwrap().is_empty());

    // Corrupt JSON.
    let path = tmp
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("scheduled_tasks.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ not json").unwrap();
    let error = store
        .list_all_cron_tasks()
        .await
        .expect_err("corrupt durable schedules must not be treated as empty");
    assert!(error.to_string().contains("refusing to overwrite"));
}

#[tokio::test]
async fn concurrent_store_instances_do_not_lose_durable_adds() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("scheduled_tasks.json");
    let left = DiskBackedScheduleStore::new(path.clone());
    let right = DiskBackedScheduleStore::new(path);

    let (a, b) = tokio::join!(
        left.add_cron_task("0 8 * * *", CronPayload::prompt("a"), true, true, None),
        right.add_cron_task("0 9 * * *", CronPayload::prompt("b"), true, true, None),
    );
    a.unwrap();
    b.unwrap();

    let tasks = left.list_all_cron_tasks().await.unwrap();
    assert_eq!(tasks.len(), 2);
}

#[tokio::test]
async fn exact_snapshot_claim_has_one_cross_instance_winner() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("scheduled_tasks.json");
    let creator = DiskBackedScheduleStore::new(path.clone());
    let task = creator
        .add_cron_task("0 * * * *", CronPayload::prompt("hourly"), true, true, None)
        .await
        .unwrap();
    let left = DiskBackedScheduleStore::new(path.clone());
    let right = DiskBackedScheduleStore::new(path);
    let expected = left
        .list_all_cron_tasks()
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.id == task.id)
        .unwrap();

    let (a, b) = tokio::join!(
        left.claim_cron_task(&expected, 42, false),
        right.claim_cron_task(&expected, 42, false),
    );
    let winners = usize::from(a.unwrap().is_some()) + usize::from(b.unwrap().is_some());
    assert_eq!(winners, 1);
}

#[tokio::test]
async fn invalid_cron_rows_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("scheduled_tasks.json");
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
    let error = store
        .list_all_cron_tasks()
        .await
        .expect_err("invalid durable rows must not be silently dropped");
    assert!(error.to_string().contains("task 'bad' has invalid cron"));
}
