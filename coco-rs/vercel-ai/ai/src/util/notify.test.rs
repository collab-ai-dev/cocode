use super::*;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

#[test]
fn test_notify() {
    let notify = Notify::new();
    let count = Arc::new(AtomicUsize::new(0));

    let count_clone = count.clone();
    notify.subscribe(move |_| {
        count_clone.fetch_add(1, Ordering::SeqCst);
    });

    notify.notify("hello".to_string());
    notify.notify("world".to_string());

    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(notify.pending_count(), 2);
}

#[test]
fn test_drain_pending() {
    let notify: Notify<i32> = Notify::new();
    notify.notify(1);
    notify.notify(2);
    notify.notify(3);

    let pending = notify.drain_pending();
    assert_eq!(pending, vec![1, 2, 3]);
    assert_eq!(notify.pending_count(), 0);
}

#[test]
fn test_shared_notify() {
    let notify = create_notify::<String>();
    assert_eq!(notify.subscriber_count(), 0);
}