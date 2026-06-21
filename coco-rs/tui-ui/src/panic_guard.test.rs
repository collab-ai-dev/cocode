use super::PanicRestoreGuard;
use super::suppress_panic_restore;

#[test]
fn guard_sets_and_clears_thread_local() {
    assert!(!suppress_panic_restore());
    {
        let _g = PanicRestoreGuard::new();
        assert!(suppress_panic_restore());
    }
    assert!(!suppress_panic_restore(), "flag cleared on drop");
}

#[test]
fn nested_guards_restore_outer_suppression() {
    assert!(!suppress_panic_restore());
    let outer = PanicRestoreGuard::new();
    assert!(suppress_panic_restore());
    {
        let _inner = PanicRestoreGuard::new();
        assert!(suppress_panic_restore());
    }
    // Inner dropped, outer still held → suppression must persist.
    assert!(
        suppress_panic_restore(),
        "inner drop must not re-arm restore"
    );
    drop(outer);
    assert!(!suppress_panic_restore(), "outer drop clears suppression");
}

#[test]
fn guard_clears_even_when_unwinding() {
    let _ = std::panic::catch_unwind(|| {
        let _g = PanicRestoreGuard::new();
        assert!(suppress_panic_restore());
        panic!("boom");
    });
    assert!(
        !suppress_panic_restore(),
        "flag cleared by guard drop during unwind"
    );
}
