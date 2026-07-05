use super::*;

#[test]
fn enabled_flag_matches_compiled_backend() {
    // The public `ENABLED` const must track exactly the cfg that gates the
    // real backend, so callers can trust it to decide whether to bother.
    let expected = cfg!(all(feature = "jemalloc", not(target_os = "windows")));
    assert_eq!(ENABLED, expected);
}

#[test]
fn purge_is_ok_regardless_of_backend() {
    // With the feature off this is a stub `Ok(())`; with it on the real
    // `arena.4096.purge` ctl must also succeed (jemalloc is linked). Either
    // way, purging must never surface an error to callers on a healthy build.
    assert!(purge_all_arenas().is_ok());
}

#[cfg(not(all(feature = "jemalloc", not(target_os = "windows"))))]
#[test]
fn stats_snapshot_is_none_without_backend() {
    assert_eq!(stats_snapshot(), None);
}

#[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
#[test]
fn stats_snapshot_reads_succeed_with_backend() {
    // The four ctl reads must resolve once jemalloc is linked. Values aren't
    // asserted (they depend on live heap state), only that the snapshot is
    // populated rather than failing mid-read.
    assert!(stats_snapshot().is_some());
}

#[cfg(not(all(feature = "jemalloc", not(target_os = "windows"))))]
#[test]
fn heap_profiling_stubs_without_backend() {
    // Feature off ⇒ never available, and both mutators are silent no-ops so
    // callers can invoke them unconditionally behind an `available` gate.
    assert!(!heap_profiling_available());
    assert!(set_heap_profiling_active(true).is_ok());
    assert!(dump_heap_profile(std::path::Path::new("/nonexistent/x.heap")).is_ok());
}

#[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
#[test]
fn heap_profiling_calls_are_safe_with_backend() {
    // Test processes don't start with `prof:true`, so availability is false
    // and the mutators must fail cleanly (a mallctl errno), never crash. If a
    // developer's env *does* enable prof the calls instead succeed — either
    // way nothing panics, which is the contract under test.
    let _ = heap_profiling_available();
    let _ = set_heap_profiling_active(false);
    let _ = dump_heap_profile(std::path::Path::new("/nonexistent/x.heap"));
}

#[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
#[test]
fn heap_profile_dump_e2e() {
    // Full activate → dump → non-empty file round-trip. Workspace builds bake
    // `prof:true` into libjemalloc (JEMALLOC_SYS_WITH_MALLOC_CONF in
    // .cargo/config.toml), so this normally exercises the real path; the guard
    // keeps it from flaking if the binary was built without that baked conf.
    if !heap_profiling_available() {
        return;
    }
    set_heap_profiling_active(true).expect("activate prof sampling");
    let dir = std::env::temp_dir().join(format!("coco-jemalloc-prof-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create dump dir");
    let path = dir.join("e2e.heap");
    dump_heap_profile(&path).expect("prof.dump");
    let len = std::fs::metadata(&path).expect("dump file exists").len();
    assert!(len > 0, "heap profile dump is empty");
    let _ = std::fs::remove_dir_all(&dir);
}
