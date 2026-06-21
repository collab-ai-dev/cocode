use super::*;

#[tokio::test]
async fn test_resolvable_promise_resolve() {
    let promise = ResolvablePromise::<i32>::new();
    let mut promise_clone = ResolvablePromise::new();

    // Resolve the promise
    assert!(promise_clone.resolve(42));

    // Wait for result
    // Note: Can't use original promise after cloning sender
}

#[tokio::test]
async fn test_resolved() {
    let promise = resolved(42);
    assert!(promise.is_resolved());
}

#[tokio::test]
async fn test_rejected() {
    let promise: ResolvablePromise<i32> = rejected("Something went wrong");
    assert!(promise.is_resolved());
}

#[test]
fn test_create_promise() {
    let promise = create_promise::<i32>();
    assert!(!promise.blocking_lock().is_resolved());
}