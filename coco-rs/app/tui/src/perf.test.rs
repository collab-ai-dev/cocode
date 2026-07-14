use super::*;

#[test]
fn parses_macos_ps_memory_output_and_converts_kib_to_bytes() {
    let parsed = parse_ps_memory_output("  12345  67890\n").expect("parse ps output");
    let sample = parsed.into_sample(7);
    assert_eq!(sample.rss_bytes, 12_641_280);
    assert_eq!(sample.vsz_bytes, 69_519_360);
    assert_eq!(sample.sample_ms, 7);
}

#[test]
fn parses_ps_output_with_header_or_extra_whitespace() {
    let parsed = parse_ps_memory_output("RSS VSZ\n 42\t9001\n").expect("parse ps output");
    assert_eq!(parsed.rss_kib, 42);
    assert_eq!(parsed.vsz_kib, 9001);
}

#[test]
fn memory_sample_source_prefers_physical_footprint() {
    let mut sample = PsMemoryKb {
        rss_kib: 42,
        vsz_kib: 9001,
    }
    .into_sample(7);
    assert_eq!(sample.source_label(), "macos_ps");

    sample.physical_footprint_bytes = Some(123);
    sample.physical_footprint_peak_bytes = Some(456);
    assert_eq!(sample.source_label(), "macos_task_info+ps");
}

#[test]
fn memory_periodic_and_threshold_can_be_disabled_independently() {
    let mut config = TuiPerformanceConfig {
        frame_enabled: false,
        frame_sample_every_n_frames: 10,
        frame_slow_threshold_ms: 16,
        frame_stage_slow_threshold_us: 1000,
        memory_enabled: true,
        memory_sample_interval_secs: 0,
        memory_delta_threshold_bytes: 0,
        heap_profile_enabled: false,
    };
    assert!(MemoryPerfTracker::enabled(config));
    assert!(!MemoryPerfTracker::periodic_enabled(config));

    config.memory_sample_interval_secs = 30;
    assert!(MemoryPerfTracker::periodic_enabled(config));
    assert_eq!(
        MemoryPerfTracker::periodic_interval(config),
        Duration::from_secs(30)
    );
}

#[test]
fn trigger_labels_merge_threshold_with_single_sample() {
    assert_eq!(
        trigger_label(MemorySampleKind::Lifecycle, true),
        "lifecycle+threshold"
    );
    assert_eq!(
        trigger_label(MemorySampleKind::Periodic, true),
        "periodic+threshold"
    );
    assert_eq!(
        trigger_label(MemorySampleKind::Lifecycle, false),
        "lifecycle"
    );
    assert_eq!(trigger_label(MemorySampleKind::Periodic, false), "periodic");
}

#[test]
fn retained_stats_follow_transcript_append_truncate_reset() {
    let mut state = AppState::new();
    state
        .session
        .transcript
        .on_message_appended(std::sync::Arc::new(
            coco_messages::create_user_message_with_uuid(uuid::Uuid::new_v4(), "hello"),
        ));
    let after_append = retained_memory_stats(&state, 123);
    assert!(after_append.message_history_payload_bytes > 0);
    assert!(after_append.transcript_cell_text_bytes >= "hello".len());
    assert_eq!(after_append.history_replay_cache_bytes, 123);

    state.session.transcript.on_message_truncated(0);
    let after_truncate = retained_memory_stats(&state, 0);
    assert_eq!(after_truncate.transcript_cell_text_bytes, 0);
    assert_eq!(after_truncate.history_replay_cache_bytes, 0);

    state.session.last_agent_markdown = Some("cached markdown".to_string());
    state.session.transcript.on_session_reset();
    state.session.last_agent_markdown = None;
    let after_reset = retained_memory_stats(&state, 0);
    assert_eq!(after_reset.message_history_payload_bytes, 0);
    assert_eq!(after_reset.last_markdown_bytes, 0);
}
