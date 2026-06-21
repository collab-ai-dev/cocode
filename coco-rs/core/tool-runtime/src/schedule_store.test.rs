use super::*;

#[tokio::test]
async fn in_memory_schedule_store_adds_lists_marks_and_removes() {
    let store = InMemoryScheduleStore::new();

    let task = store
        .add_cron_task("0 9 * * *", "summarize work", true, false, None)
        .await
        .unwrap();
    assert_eq!(task.prompt, "summarize work");
    assert!(task.is_recurring());
    assert_eq!(task.durable, Some(false));

    let listed = store.list_all_cron_tasks().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, task.id);
    assert!(listed[0].last_fired_at.is_none());

    store
        .mark_cron_tasks_fired(&[&task.id], 1_700_000_000_000)
        .await
        .unwrap();
    let listed = store.list_all_cron_tasks().await.unwrap();
    assert_eq!(listed[0].last_fired_at, Some(1_700_000_000_000));

    store.remove_cron_tasks(&[&task.id]).await.unwrap();
    assert!(store.list_all_cron_tasks().await.unwrap().is_empty());
}

#[tokio::test]
async fn in_memory_trigger_store_round_trips() {
    let store = InMemoryScheduleStore::new();

    let trigger = store
        .create_trigger("deploy", Some("deployment hook"))
        .await
        .unwrap();
    assert_eq!(store.get_trigger(&trigger.id).await.unwrap().name, "deploy");
    assert_eq!(
        store.run_trigger(&trigger.id).await.unwrap(),
        "Triggered deploy"
    );
}
