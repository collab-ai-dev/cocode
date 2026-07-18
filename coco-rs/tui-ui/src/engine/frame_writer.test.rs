use std::io;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use super::FrameDelivery;
use super::FrameWriter;
use super::FrameWriterOptions;
use super::MAX_PENDING_BYTES;

#[derive(Clone, Default)]
struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

impl CapturedWriter {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().expect("capture lock").clone()
    }
}

impl Write for CapturedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("capture lock").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "test failure"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn buffers_until_present_and_drains() {
    let capture = CapturedWriter::default();
    let mut writer =
        FrameWriter::new(capture.clone(), FrameWriterOptions::default()).expect("frame writer");
    let barrier = writer.barrier();

    writer.write_all(b"frame").expect("buffer frame");
    assert!(capture.bytes().is_empty());
    writer
        .present(FrameDelivery::Incremental)
        .expect("present frame");
    assert!(barrier.wait_drained(Duration::from_secs(1)).expect("drain"));
    assert_eq!(capture.bytes(), b"frame");
    assert_eq!(barrier.counters().queued, 1);
    assert_eq!(barrier.counters().written, 1);
}

#[test]
fn self_contained_frame_replaces_the_pending_slot() {
    let capture = CapturedWriter::default();
    let mut writer = FrameWriter::new(
        capture.clone(),
        FrameWriterOptions {
            write_delay: Duration::from_millis(40),
        },
    )
    .expect("frame writer");
    let barrier = writer.barrier();

    writer.write_all(b"A").expect("first frame");
    writer
        .present(FrameDelivery::SelfContained)
        .expect("present first");
    assert!(
        barrier
            .wait_until_in_flight(Duration::from_secs(1))
            .expect("observe in flight")
    );
    writer.write_all(b"B").expect("second frame");
    writer
        .present(FrameDelivery::SelfContained)
        .expect("present second");
    writer.write_all(b"C").expect("third frame");
    writer
        .present(FrameDelivery::SelfContained)
        .expect("present third");

    assert!(barrier.wait_drained(Duration::from_secs(1)).expect("drain"));
    assert_eq!(capture.bytes(), b"AC");
    assert_eq!(barrier.counters().dropped, 1);
}

#[test]
fn incremental_frames_coalesce_without_losing_order() {
    let capture = CapturedWriter::default();
    let mut writer = FrameWriter::new(
        capture.clone(),
        FrameWriterOptions {
            write_delay: Duration::from_millis(40),
        },
    )
    .expect("frame writer");
    let barrier = writer.barrier();

    writer.write_all(b"A").expect("first frame");
    writer
        .present(FrameDelivery::Incremental)
        .expect("present first");
    assert!(
        barrier
            .wait_until_in_flight(Duration::from_secs(1))
            .expect("observe in flight")
    );
    for bytes in [b"B".as_slice(), b"C".as_slice()] {
        writer.write_all(bytes).expect("next frame");
        writer
            .present(FrameDelivery::Incremental)
            .expect("present next");
    }

    assert!(barrier.wait_drained(Duration::from_secs(1)).expect("drain"));
    assert_eq!(capture.bytes(), b"ABC");
    assert_eq!(barrier.counters().dropped, 0);
    assert_eq!(barrier.counters().written, 3);
}

#[test]
fn drain_timeout_is_bounded() {
    let capture = CapturedWriter::default();
    let mut writer = FrameWriter::new(
        capture,
        FrameWriterOptions {
            write_delay: Duration::from_millis(80),
        },
    )
    .expect("frame writer");
    let barrier = writer.barrier();
    writer.write_all(b"slow").expect("frame");
    writer.present(FrameDelivery::Incremental).expect("present");

    assert!(
        !barrier
            .wait_drained(Duration::from_millis(5))
            .expect("bounded drain")
    );
    assert!(
        barrier
            .wait_drained(Duration::from_secs(1))
            .expect("eventual drain")
    );
}

#[test]
fn oversized_pending_backlog_fails_without_queueing_unbounded_bytes() {
    let capture = CapturedWriter::default();
    let mut writer = FrameWriter::new(capture, FrameWriterOptions::default()).expect("writer");
    let barrier = writer.barrier();
    writer
        .write_all(&vec![b'x'; MAX_PENDING_BYTES + 1])
        .expect("buffer oversized frame");

    let error = writer
        .present(FrameDelivery::Incremental)
        .expect_err("backlog cap");

    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert_eq!(barrier.counters().queued, 0);
}

#[test]
fn teardown_tail_bypasses_backlog_cap_and_remains_last() {
    let sink = CapturedWriter::default();
    let mut writer = FrameWriter::new(
        sink.clone(),
        FrameWriterOptions {
            write_delay: Duration::from_millis(50),
        },
    )
    .expect("writer");
    writer.write_all(b"old").expect("old frame");
    writer
        .present(FrameDelivery::Incremental)
        .expect("present old frame");
    writer
        .write_all(&vec![b'x'; MAX_PENDING_BYTES + 1])
        .expect("oversized local frame");
    assert_eq!(
        writer
            .present(FrameDelivery::Incremental)
            .expect_err("ordinary frame must respect cap")
            .kind(),
        io::ErrorKind::WouldBlock
    );
    writer.write_all(b"RESTORE").expect("restore tail");
    writer
        .present(FrameDelivery::Teardown)
        .expect("teardown must remain queueable");
    assert!(
        writer
            .barrier()
            .wait_drained(Duration::from_secs(2))
            .expect("drain")
    );

    let bytes = sink.bytes();
    assert_eq!(bytes, b"oldRESTORE");
    assert!(
        !bytes.contains(&b'x'),
        "rejected ordinary payload leaked through teardown"
    );
}

#[test]
fn writer_error_is_published_to_the_drain_barrier() {
    let mut writer =
        FrameWriter::new(FailingWriter, FrameWriterOptions::default()).expect("start worker");
    let barrier = writer.barrier();
    writer.write_all(b"frame").expect("buffer frame");
    writer
        .present(FrameDelivery::Incremental)
        .expect("queue frame");

    let error = barrier
        .wait_drained(Duration::from_secs(1))
        .expect_err("physical write failure");

    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
}
