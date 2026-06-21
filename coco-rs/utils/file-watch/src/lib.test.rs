use super::*;
use notify::EventKind;
use pretty_assertions::assert_eq;
use tokio::time::timeout;

fn path(name: &str) -> PathBuf {
    PathBuf::from(name)
}

// -----------------------------------------------------------------------
// ThrottledPaths
// -----------------------------------------------------------------------

#[test]
fn throttle_first_emit_immediate() {
    let mut tp = ThrottledPaths::new(Duration::from_secs(1));
    let now = Instant::now();
    tp.add(vec![path("a")]);
    let result = tp.take_ready(now);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), vec![path("a")]);
}

#[test]
fn throttle_coalesces_within_interval() {
    let mut tp = ThrottledPaths::new(Duration::from_secs(1));
    let start = Instant::now();

    tp.add(vec![path("a")]);
    let _ = tp.take_ready(start).unwrap();

    // Within the throttle window — should not emit.
    tp.add(vec![path("b"), path("c")]);
    assert!(tp.take_ready(start).is_none());

    // After the window — should emit coalesced.
    let later = start + Duration::from_secs(1);
    let result = tp.take_ready(later).unwrap();
    assert_eq!(result, vec![path("b"), path("c")]);
}

#[test]
fn throttle_flushes_on_shutdown() {
    let mut tp = ThrottledPaths::new(Duration::from_secs(1));
    let start = Instant::now();

    tp.add(vec![path("a")]);
    let _ = tp.take_ready(start).unwrap();

    tp.add(vec![path("b")]);
    assert!(tp.take_ready(start).is_none());

    let flushed = tp.take_pending(start).unwrap();
    assert_eq!(flushed, vec![path("b")]);
}

#[test]
fn throttle_configurable_interval() {
    let mut tp = ThrottledPaths::new(Duration::from_millis(200));
    let start = Instant::now();

    tp.add(vec![path("a")]);
    let _ = tp.take_ready(start).unwrap();

    tp.add(vec![path("b")]);
    // 100ms < 200ms interval — still throttled.
    assert!(tp.take_ready(start + Duration::from_millis(100)).is_none());
    // 200ms = interval — should emit.
    assert!(tp.take_ready(start + Duration::from_millis(200)).is_some());
}

#[test]
fn throttle_is_empty() {
    let mut tp = ThrottledPaths::new(Duration::from_secs(1));
    assert!(tp.is_empty());
    tp.add(vec![path("a")]);
    assert!(!tp.is_empty());
    let _ = tp.take_ready(Instant::now());
    assert!(tp.is_empty());
}

#[test]
fn throttle_next_deadline_none_when_empty() {
    let tp = ThrottledPaths::new(Duration::from_secs(1));
    assert!(tp.next_deadline(Instant::now()).is_none());
}

// -----------------------------------------------------------------------
// FileWatcher (noop mode)
// -----------------------------------------------------------------------

#[test]
fn noop_watch_unwatch_no_panic() {
    let watcher: FileWatcher<String> = FileWatcherBuilder::new().build_noop();
    watcher.watch(path("/nonexistent"), RecursiveMode::Recursive);
    watcher.unwatch(Path::new("/nonexistent"));
    let _rx = watcher.subscribe();
}

// -----------------------------------------------------------------------
// FileWatcher (watch deduplication / upgrade)
// -----------------------------------------------------------------------

#[tokio::test]
async fn watch_deduplicates_paths() {
    let watcher: FileWatcher<Vec<PathBuf>> = FileWatcherBuilder::new()
        .build(
            |event| Some(event.paths.clone()),
            |mut acc, new| {
                acc.extend(new);
                acc
            },
        )
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();

    // Watching the same path twice should not error.
    watcher.watch(dir_path.clone(), RecursiveMode::NonRecursive);
    watcher.watch(dir_path, RecursiveMode::NonRecursive);

    // Verify internal state: only one entry.
    let inner = watcher.inner.as_ref().unwrap().lock().unwrap();
    assert_eq!(inner.watched_paths.len(), 1);
}

#[tokio::test]
async fn watch_upgrades_recursive_mode() {
    let watcher: FileWatcher<Vec<PathBuf>> = FileWatcherBuilder::new()
        .build(
            |event| Some(event.paths.clone()),
            |mut acc, new| {
                acc.extend(new);
                acc
            },
        )
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();

    watcher.watch(dir_path.clone(), RecursiveMode::NonRecursive);
    watcher.watch(dir_path.clone(), RecursiveMode::Recursive);

    let inner = watcher.inner.as_ref().unwrap().lock().unwrap();
    assert_eq!(
        inner.watched_paths.get(&dir_path),
        Some(&RecursiveMode::Recursive)
    );
}

// -----------------------------------------------------------------------
// Event loop integration
// -----------------------------------------------------------------------

#[tokio::test]
async fn event_loop_classifies_and_broadcasts() {
    // Build a watcher that classifies events by collecting paths.
    let (raw_tx, raw_rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
    let (tx, mut rx) = broadcast::channel::<Vec<PathBuf>>(8);

    spawn_event_loop(
        raw_rx,
        tx,
        Duration::from_secs(1),
        |event| {
            let paths: Vec<PathBuf> = event.paths.clone();
            if paths.is_empty() { None } else { Some(paths) }
        },
        |mut acc, new| {
            acc.extend(new);
            acc
        },
    );

    let mut event = notify::Event::new(EventKind::Any);
    event = event.add_path(path("/tmp/a"));

    raw_tx.send(Ok(event)).unwrap();

    let received = timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, vec![path("/tmp/a")]);
}

#[tokio::test]
async fn event_loop_flushes_on_close() {
    let (raw_tx, raw_rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
    let (tx, mut rx) = broadcast::channel::<Vec<PathBuf>>(8);

    spawn_event_loop(
        raw_rx,
        tx,
        Duration::from_secs(1),
        |event| {
            let paths: Vec<PathBuf> = event.paths.clone();
            if paths.is_empty() { None } else { Some(paths) }
        },
        |mut acc, new| {
            acc.extend(new);
            acc
        },
    );

    // First event is emitted immediately.
    let mut event1 = notify::Event::new(EventKind::Any);
    event1 = event1.add_path(path("/tmp/a"));
    raw_tx.send(Ok(event1)).unwrap();

    let first = timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first, vec![path("/tmp/a")]);

    // Second event is within the throttle window, so it's pending.
    let mut event2 = notify::Event::new(EventKind::Any);
    event2 = event2.add_path(path("/tmp/b"));
    raw_tx.send(Ok(event2)).unwrap();

    // Give the loop time to receive the event before dropping the sender.
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(raw_tx);

    // The pending event should be flushed on close.
    let second = timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second, vec![path("/tmp/b")]);
}

// -----------------------------------------------------------------------
// is_content_change — read/metadata filtering (self-feed-loop guard)
// -----------------------------------------------------------------------

#[test]
fn content_change_keeps_real_mutations() {
    use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};
    assert!(is_content_change(EventKind::Create(CreateKind::File)));
    assert!(is_content_change(EventKind::Modify(ModifyKind::Data(
        DataChange::Any
    ))));
    assert!(is_content_change(EventKind::Modify(ModifyKind::Name(
        RenameMode::To
    ))));
    assert!(is_content_change(EventKind::Remove(RemoveKind::File)));
    // Coarse fallbacks pass through so non-inotify backends are not dropped.
    assert!(is_content_change(EventKind::Any));
    assert!(is_content_change(EventKind::Other));
}

#[test]
fn content_change_drops_reads_and_metadata() {
    use notify::event::{AccessKind, AccessMode, MetadataKind, ModifyKind};
    // IN_OPEN — the read that self-feeds when the reaction re-reads the file.
    assert!(!is_content_change(EventKind::Access(AccessKind::Open(
        AccessMode::Any
    ))));
    assert!(!is_content_change(EventKind::Access(AccessKind::Read)));
    assert!(!is_content_change(EventKind::Access(AccessKind::Close(
        AccessMode::Write
    ))));
    // IN_ATTRIB — the atime bump that self-feeds under a strictatime mount.
    assert!(!is_content_change(EventKind::Modify(ModifyKind::Metadata(
        MetadataKind::Any
    ))));
}

/// Regression for the runaway config-reload loop: a read-open on a watched
/// file must be filtered before `classify`, while a real write on the same
/// path is still delivered.
#[tokio::test]
async fn event_loop_filters_read_open_but_keeps_write() {
    use notify::event::{AccessKind, AccessMode, DataChange, ModifyKind};
    let (raw_tx, raw_rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
    let (tx, mut rx) = broadcast::channel::<Vec<PathBuf>>(8);

    spawn_event_loop(
        raw_rx,
        tx,
        Duration::from_secs(1),
        |event| {
            let paths: Vec<PathBuf> = event.paths.clone();
            if paths.is_empty() { None } else { Some(paths) }
        },
        |mut acc, new| {
            acc.extend(new);
            acc
        },
    );

    // Read-open (as emitted when the reload re-reads models.json) — dropped.
    let open = notify::Event::new(EventKind::Access(AccessKind::Open(AccessMode::Any)))
        .add_path(path("/tmp/models.json"));
    raw_tx.send(Ok(open)).unwrap();
    assert!(
        timeout(Duration::from_millis(200), rx.recv())
            .await
            .is_err(),
        "read-open event must be filtered before classify"
    );

    // A genuine content change on the same path is still delivered.
    let modify = notify::Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
        .add_path(path("/tmp/models.json"));
    raw_tx.send(Ok(modify)).unwrap();
    let received = timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, vec![path("/tmp/models.json")]);
}
