use super::FrameRequester;
use coco_tui_ui::frame_rate_limiter::MIN_FRAME_INTERVAL;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time;

/// Drop-in for codex's `tokio_util::time::FutureExt::timeout` — we
/// avoid pulling tokio-util into dev-deps. Returns `Err(_)` on timeout.
async fn recv_with_timeout(
    rx: &mut broadcast::Receiver<()>,
    dur: Duration,
) -> Result<Result<(), broadcast::error::RecvError>, tokio::time::error::Elapsed> {
    time::timeout(dur, rx.recv()).await
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn schedule_frame_immediate_triggers_once() {
    let (draw_tx, mut draw_rx) = broadcast::channel(16);
    let requester = FrameRequester::new(draw_tx);

    requester.schedule_frame();
    time::advance(Duration::from_millis(1)).await;

    recv_with_timeout(&mut draw_rx, Duration::from_millis(50))
        .await
        .expect("timed out waiting for first draw")
        .expect("broadcast closed unexpectedly");

    assert!(
        recv_with_timeout(&mut draw_rx, Duration::from_millis(20))
            .await
            .is_err(),
        "unexpected extra draw"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn schedule_frame_in_triggers_at_delay() {
    let (draw_tx, mut draw_rx) = broadcast::channel(16);
    let requester = FrameRequester::new(draw_tx);

    requester.schedule_frame_in(Duration::from_millis(50));

    time::advance(Duration::from_millis(30)).await;
    assert!(
        recv_with_timeout(&mut draw_rx, Duration::from_millis(10))
            .await
            .is_err(),
        "draw fired too early"
    );

    time::advance(Duration::from_millis(25)).await;
    recv_with_timeout(&mut draw_rx, Duration::from_millis(50))
        .await
        .expect("timed out waiting for scheduled draw")
        .expect("broadcast closed");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn coalesces_multiple_requests_into_single_draw() {
    let (draw_tx, mut draw_rx) = broadcast::channel(16);
    let requester = FrameRequester::new(draw_tx);

    requester.schedule_frame();
    requester.schedule_frame();
    requester.schedule_frame();
    time::advance(Duration::from_millis(1)).await;

    recv_with_timeout(&mut draw_rx, Duration::from_millis(50))
        .await
        .expect("timed out")
        .expect("broadcast closed");

    assert!(
        recv_with_timeout(&mut draw_rx, Duration::from_millis(20))
            .await
            .is_err(),
        "three requests should coalesce into one draw"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn coalesces_mixed_immediate_and_delayed_to_earliest() {
    let (draw_tx, mut draw_rx) = broadcast::channel(16);
    let requester = FrameRequester::new(draw_tx);

    requester.schedule_frame_in(Duration::from_millis(100));
    requester.schedule_frame();
    time::advance(Duration::from_millis(1)).await;

    recv_with_timeout(&mut draw_rx, Duration::from_millis(50))
        .await
        .expect("timed out")
        .expect("broadcast closed");

    assert!(
        recv_with_timeout(&mut draw_rx, Duration::from_millis(120))
            .await
            .is_err(),
        "delayed request should have been absorbed into the immediate one"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn rate_limits_to_120fps() {
    let (draw_tx, mut draw_rx) = broadcast::channel(16);
    let requester = FrameRequester::new(draw_tx);

    requester.schedule_frame();
    time::advance(Duration::from_millis(1)).await;
    recv_with_timeout(&mut draw_rx, Duration::from_millis(50))
        .await
        .expect("first draw timed out")
        .expect("broadcast closed");

    requester.schedule_frame();
    time::advance(Duration::from_millis(1)).await;
    assert!(
        recv_with_timeout(&mut draw_rx, Duration::from_millis(1))
            .await
            .is_err(),
        "second draw fired before min frame interval"
    );

    time::advance(MIN_FRAME_INTERVAL).await;
    recv_with_timeout(&mut draw_rx, Duration::from_millis(50))
        .await
        .expect("second draw timed out")
        .expect("broadcast closed");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn test_dummy_silently_drops_requests() {
    // The helper must not panic or hang even when nothing is listening
    // on the receiver side (it's dropped at construction).
    let requester = FrameRequester::test_dummy();
    requester.schedule_frame();
    requester.schedule_frame_in(Duration::from_millis(50));
}
