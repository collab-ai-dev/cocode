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
