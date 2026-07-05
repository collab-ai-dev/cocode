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
