use super::*;
use coco_tool_runtime::CronTask;

fn task(id: &str, cron: &str, prompt: &str) -> CronTask {
    CronTask {
        id: id.into(),
        cron: cron.into(),
        prompt: prompt.into(),
        created_at: 0,
        last_fired_at: None,
        recurring: None,
        permanent: None,
        durable: None,
        agent_id: None,
    }
}

#[test]
fn missed_notification_single_has_guidance_and_human_schedule() {
    let t = task("a", "0 9 * * *", "back up files");
    let out = build_missed_notification(&[&t]);
    assert!(out.contains("was missed"), "got: {out}");
    assert!(out.contains("AskUserQuestion"), "got: {out}");
    assert!(out.contains("Every day at 9:00 AM"), "got: {out}");
    assert!(out.contains("back up files"), "got: {out}");
}

#[test]
fn missed_notification_plural_lists_all() {
    let a = task("a", "0 9 * * *", "one");
    let b = task("b", "0 10 * * *", "two");
    let out = build_missed_notification(&[&a, &b]);
    assert!(out.contains("tasks were missed"), "got: {out}");
    assert!(out.contains("one") && out.contains("two"), "got: {out}");
}

#[test]
fn missed_notification_fences_longer_than_inner_backticks() {
    let t = task("a", "0 9 * * *", "run ```code``` now");
    let out = build_missed_notification(&[&t]);
    // The inner run is 3 backticks → the fence must be ≥4 so it can't be closed early.
    assert!(
        out.contains("````"),
        "fence must exceed inner run, got: {out}"
    );
}

#[test]
fn scheduled_loop_sentinel_uses_shared_state_for_short_reminders() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    std::fs::write(dir.path().join(".claude").join("loop.md"), "stable task\n")
        .expect("write loop");
    let mut state = coco_skills::bundled::loop_skill::LoopSentinelState::default();

    let first = expand_scheduled_prompt(
        coco_skills::bundled::loop_skill::LOOP_FILE_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    );
    let second = expand_scheduled_prompt(
        coco_skills::bundled::loop_skill::LOOP_FILE_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    );

    assert!(first.contains("# /loop tick — tasks from "));
    assert!(first.contains("stable task"));
    assert!(second.contains("# /loop tick — loop.md tasks"));
    assert!(!second.contains("stable task"));
}

#[test]
fn scheduled_non_sentinel_prompt_is_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut state = coco_skills::bundled::loop_skill::LoopSentinelState::default();

    let prompt = expand_scheduled_prompt(
        "check deployment",
        dir.path(),
        dir.path(),
        &mut state,
        false,
    );

    assert_eq!(prompt, "check deployment");
}
