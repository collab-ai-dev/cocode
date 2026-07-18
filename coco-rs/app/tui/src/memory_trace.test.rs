use std::fs;

use pretty_assertions::assert_eq;

use super::*;
use crate::perf::ProcessMemorySample;

#[test]
fn thresholds_double_and_rearm_only_after_halving_hysteresis() {
    let mut thresholds = Thresholds::new(100);

    assert_eq!(thresholds.observe(99), None);
    assert_eq!(
        thresholds.observe(100),
        Some(Crossing {
            gauge_bytes: 100,
            crossed_threshold_bytes: 100,
            next_threshold_bytes: 200,
        })
    );
    assert_eq!(thresholds.observe(150), None);
    assert_eq!(thresholds.next_bytes, 200);
    assert_eq!(thresholds.observe(99), None);
    assert_eq!(thresholds.next_bytes, 100);
    assert!(thresholds.observe(100).is_some());
}

#[test]
fn thresholds_skip_multiple_buckets_with_one_crossing() {
    let mut thresholds = Thresholds::new(100);

    let crossing = thresholds.observe(450).expect("crossing");

    assert_eq!(crossing.crossed_threshold_bytes, 100);
    assert_eq!(crossing.next_threshold_bytes, 800);
}

#[test]
fn jsonl_sink_rotates_and_keeps_each_record_parseable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let trace = MemoryTrace::open_at(dir.path(), 42, 700).expect("trace");
    let observation = MemoryObservation {
        process: Some(ProcessMemorySample {
            rss_bytes: 64,
            vsz_bytes: 128,
            physical_footprint_bytes: None,
            physical_footprint_peak_bytes: None,
            sample_ms: 1,
            source: crate::perf::ProcessMemorySource::MacOsPs,
        }),
        jemalloc: None,
        retained: RetainedMemoryStats::default(),
    };

    for _ in 0..8 {
        trace.record_sample(
            MemoryPhase::Periodic,
            MemorySampleKind::Periodic,
            observation,
        );
    }

    let active = dir.path().join("coco.42.jsonl");
    let rotated = backup_path(&active, 1);
    assert!(
        rotated.exists(),
        "size rotation must retain a prior segment"
    );
    for path in [active, rotated] {
        let content = fs::read_to_string(path).expect("read JSONL");
        for line in content.lines() {
            let value: serde_json::Value = serde_json::from_str(line).expect("valid JSON record");
            assert_eq!(value["event"], "sample");
        }
    }
}

#[test]
fn stats_dump_truncation_preserves_utf8_boundaries() {
    let text = "界".repeat(10);

    let (truncated, did_truncate) = truncate_utf8(&text, 8);

    assert!(did_truncate);
    assert_eq!(truncated, "界界");
}

#[test]
fn threshold_and_purge_events_are_persisted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let trace = MemoryTrace::open_at(dir.path(), 77, 2 * 1024 * 1024).expect("trace");
    let observation = MemoryObservation {
        process: Some(ProcessMemorySample {
            rss_bytes: 600 * 1024 * 1024,
            vsz_bytes: 700 * 1024 * 1024,
            physical_footprint_bytes: None,
            physical_footprint_peak_bytes: None,
            sample_ms: 1,
            source: crate::perf::ProcessMemorySource::MacOsPs,
        }),
        jemalloc: None,
        retained: RetainedMemoryStats::default(),
    };

    trace.record_observation(
        MemoryPhase::Periodic,
        MemorySampleKind::Periodic,
        observation,
    );
    trace.record_purge(
        MemoryPhase::ContextCleared,
        None,
        None,
        Duration::from_millis(4),
        Some("test"),
    );

    let content = fs::read_to_string(dir.path().join("coco.77.jsonl")).expect("trace JSONL");
    let events = content
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON event"))
        .map(|value| value["event"].as_str().expect("event tag").to_string())
        .collect::<Vec<_>>();
    assert_eq!(events, ["sample", "threshold_crossing", "purge"]);
}

#[test]
fn directory_pruning_bounds_old_pid_artifacts_and_keeps_current_pid() {
    let dir = tempfile::tempdir().expect("tempdir");
    for pid in 1..=5 {
        fs::write(dir.path().join(format!("coco.{pid}.jsonl")), b"{}\n").expect("seed trace");
    }
    let current = dir.path().join("coco.99.jsonl");
    fs::write(&current, b"{}\n").expect("seed current trace");

    prune_directory(dir.path(), 99, 2, Duration::ZERO, SystemTime::now()).expect("prune");

    let paths = fs::read_dir(dir.path())
        .expect("list traces")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert!(current.exists());
    assert_eq!(
        paths.len(),
        3,
        "two old artifacts plus current are retained"
    );
}

#[test]
fn sample_jobs_preserve_reservation_order_when_workers_start_out_of_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let trace = MemoryTrace::open_at(dir.path(), 88, 2 * 1024 * 1024).expect("trace");
    let first = trace
        .sample_job(MemoryPhase::Startup, MemorySampleKind::Lifecycle)
        .expect("first job");
    let second = trace
        .sample_job(MemoryPhase::FirstDraw, MemorySampleKind::Lifecycle)
        .expect("second job");
    let observation = |rss_bytes| {
        Some(MemoryObservation {
            process: Some(ProcessMemorySample {
                rss_bytes,
                vsz_bytes: rss_bytes,
                physical_footprint_bytes: None,
                physical_footprint_peak_bytes: None,
                sample_ms: 1,
                source: crate::perf::ProcessMemorySource::MacOsPs,
            }),
            jemalloc: None,
            retained: RetainedMemoryStats::default(),
        })
    };

    let second_worker = std::thread::spawn(move || second.run(observation(2)));
    first.run(observation(1));
    second_worker.join().expect("second worker");

    let content = fs::read_to_string(dir.path().join("coco.88.jsonl")).expect("trace JSONL");
    let phases = content
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON event"))
        .filter(|value| value["event"] == "sample")
        .map(|value| value["phase"].as_str().expect("phase").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(phases, ["startup", "first_draw"]);
}

#[test]
fn purge_job_waits_for_its_pre_purge_sample() {
    let dir = tempfile::tempdir().expect("tempdir");
    let trace = MemoryTrace::open_at(dir.path(), 89, 2 * 1024 * 1024).expect("trace");
    let sample = trace
        .sample_job(MemoryPhase::ContextCleared, MemorySampleKind::Lifecycle)
        .expect("sample job");
    let purge = trace.purge_job();
    assert!(
        !purge.is_ready_for_test(),
        "the purge ticket must be behind its reserved pre-purge sample"
    );
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let purge_worker = std::thread::spawn(move || {
        purge.run(|trace| {
            entered_tx.send(()).expect("purge entered");
            trace.record_purge(
                MemoryPhase::ContextCleared,
                None,
                None,
                Duration::from_millis(1),
                None,
            );
        });
    });

    sample.run(Some(MemoryObservation {
        process: None,
        jemalloc: None,
        retained: RetainedMemoryStats::default(),
    }));
    entered_rx.recv().expect("purge entered after sample");
    purge_worker.join().expect("purge worker");

    let content = fs::read_to_string(dir.path().join("coco.89.jsonl")).expect("trace JSONL");
    let events = content
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON event"))
        .map(|value| value["event"].as_str().expect("event tag").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(events, ["sample", "purge"]);
}
