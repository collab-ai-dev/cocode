use super::*;

#[tokio::test]
async fn in_memory_schedule_store_adds_lists_marks_and_removes() {
    let store = InMemoryScheduleStore::new();

    let task = store
        .add_cron_task(
            "0 9 * * *",
            CronPayload::prompt("summarize work"),
            true,
            false,
            None,
        )
        .await
        .unwrap();
    assert_eq!(task.prompt(), Some("summarize work"));
    assert_eq!(task.script(), None);
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

/// Records written before script jobs existed carry only `prompt`, so they
/// must still load — the untagged payload is the whole migration story.
#[test]
fn legacy_prompt_only_record_deserializes_as_a_prompt_job() {
    let task: CronTask = serde_json::from_str(
        r#"{"id":"ab12","cron":"0 9 * * *","prompt":"back up","createdAt":17,"recurring":true}"#,
    )
    .expect("legacy record must load");
    assert_eq!(task.prompt(), Some("back up"));
    assert_eq!(task.display_summary(), "back up");
    assert!(task.is_recurring());
}

#[test]
fn script_record_round_trips_through_disk_shape() {
    let task = new_cron_task(
        "*/5 * * * *",
        CronPayload::Script {
            script: "check.sh".into(),
            on_output: ScriptOutputAction::WakeAgent,
        },
        true,
        true,
        None,
    );
    let json = serde_json::to_string(&task).expect("serialize");
    assert!(json.contains(r#""script":"check.sh""#), "{json}");
    assert!(json.contains(r#""onOutput":"wake_agent""#), "{json}");
    // Runtime-only fields never reach disk.
    assert!(!json.contains("durable"), "{json}");

    let back: CronTask = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.script(), Some("check.sh"));
    assert_eq!(back.prompt(), None);
    assert_eq!(back.display_summary(), "$ check.sh");
}

/// A script record without `onOutput` (hand-edited file) defaults to Notify
/// rather than silently waking the agent.
#[test]
fn script_record_without_on_output_defaults_to_notify() {
    let task: CronTask = serde_json::from_str(
        r#"{"id":"cd34","cron":"0 * * * *","script":"ping.sh","createdAt":1}"#,
    )
    .expect("script record must load");
    assert_eq!(
        task.payload,
        CronPayload::Script {
            script: "ping.sh".into(),
            on_output: ScriptOutputAction::Notify,
        }
    );
}
