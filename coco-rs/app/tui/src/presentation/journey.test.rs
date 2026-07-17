use super::journey_lines;
use crate::i18n::locale_test_guard;
use crate::state::JourneyMode;
use crate::state::JourneyState;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;
use coco_types::{
    AgentSkillLifecycleWire, JourneyBusiestDayWire, JourneyDialogPayload, JourneyEvent,
    JourneyNodeBodyWire, JourneyNodeWire, JourneyRecord, JourneyStatsWire, SkillTelemetryWire,
    TimelineBucketWire,
};
use ratatui::text::Line;

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn joined(lines: &[Line<'_>]) -> String {
    lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
}

fn agent_skill(
    title: &str,
    lifecycle: AgentSkillLifecycleWire,
    success: i64,
    date: &str,
) -> JourneyNodeWire {
    JourneyNodeWire {
        title: title.into(),
        description: format!("what {title} does"),
        first_seen_ms: 0,
        last_activity_ms: 0,
        date_label: date.into(),
        body: JourneyNodeBodyWire::AgentSkill {
            path: format!("/skills/.agent/{title}/SKILL.md"),
            lifecycle,
            telemetry: SkillTelemetryWire {
                success_count: success,
                failure_count: 1,
                patch_count: 2,
                ..Default::default()
            },
        },
        history: vec![JourneyRecord::new(
            1,
            None,
            JourneyEvent::SkillLearned { name: title.into() },
        )],
    }
}

fn memory(title: &str, date: &str) -> JourneyNodeWire {
    JourneyNodeWire {
        title: title.into(),
        description: format!("memory about {title}"),
        first_seen_ms: 0,
        last_activity_ms: 0,
        date_label: date.into(),
        body: JourneyNodeBodyWire::Memory {
            filename: format!("{title}.md"),
        },
        history: Vec::new(),
    }
}

fn sample_state() -> JourneyState {
    let payload = JourneyDialogPayload {
        nodes: vec![
            agent_skill(
                "fix-nextest-filter",
                AgentSkillLifecycleWire::Learning {
                    progress: coco_types::SkillQuarantineWire {
                        invocations: 2,
                        required: 5,
                    },
                },
                2,
                "15 Jul",
            ),
            agent_skill(
                "wt-rebase-conflicts",
                AgentSkillLifecycleWire::Learned,
                6,
                "14 Jul",
            ),
            memory("coco-voice-and-disk-gotcha", "12 Jul"),
            agent_skill(
                "parse-log-format",
                AgentSkillLifecycleWire::Retired,
                1,
                "30 Jun",
            ),
        ],
        buckets: vec![
            TimelineBucketWire {
                start_ms: 0,
                label: "4 Jul".to_string(),
                skills: 7,
                memories: 2,
                recency: 0.1,
            },
            TimelineBucketWire {
                start_ms: 0,
                label: "15 Jul".to_string(),
                skills: 5,
                memories: 3,
                recency: 1.0,
            },
        ],
        stats: JourneyStatsWire {
            learning: 1,
            learned: 1,
            retired: 1,
            user_skills: 0,
            memories: 1,
            busiest_day: Some(JourneyBusiestDayWire {
                label: "15 Jul".into(),
                count: 3,
            }),
        },
    };
    JourneyState::from_wire(payload)
}

fn body(state: &JourneyState) -> String {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let (_title, lines, _color) = journey_lines(state, styles, 10);
    joined(&lines)
}

#[test]
fn snapshot_journey_list() {
    insta::assert_snapshot!("journey_list", body(&sample_state()));
}

#[test]
fn snapshot_journey_empty() {
    let state = JourneyState::from_wire(JourneyDialogPayload {
        nodes: Vec::new(),
        buckets: Vec::new(),
        stats: JourneyStatsWire::default(),
    });
    insta::assert_snapshot!("journey_empty", body(&state));
}

#[test]
fn snapshot_journey_detail() {
    let mut state = sample_state();
    state.selected = 0;
    state.mode = JourneyMode::Detail;
    insta::assert_snapshot!("journey_detail", body(&state));
}

#[test]
fn snapshot_journey_delete_confirm() {
    let mut state = sample_state();
    // Select the memory node and open the delete confirm (default No).
    state.selected = 2;
    state.mode = JourneyMode::DeleteMemoryConfirm {
        yes_selected: false,
    };
    insta::assert_snapshot!("journey_delete_confirm", body(&state));
}

#[test]
fn month_bucket_label_is_not_clipped() {
    // "Jul 2026" is the widest label `bucketize` emits (month granularity). The
    // day-granularity fixtures above never exercise it, which is exactly how a
    // 7-column budget shipped clipping it to "Jul 202" — a corrupt-looking year.
    let mut state = sample_state();
    state.buckets = vec![TimelineBucketWire {
        start_ms: 0,
        label: "Jul 2026".to_string(),
        skills: 3,
        memories: 1,
        recency: 1.0,
    }];
    let rendered = body(&state);
    assert!(
        rendered.contains("Jul 2026"),
        "month label must survive intact, got:\n{rendered}"
    );
}
