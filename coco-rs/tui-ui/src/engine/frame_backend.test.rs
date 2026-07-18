use std::io;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use ratatui::backend::Backend;

use super::*;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().expect("capture").clone()
    }
}

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("capture").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn synchronized_frame_is_presented_only_at_esu_and_reports_physical_latency() {
    let capture = Capture::default();
    let mut backend = FrameCrosstermBackend::new(
        capture.clone(),
        FrameWriterOptions {
            write_delay: Duration::from_millis(20),
        },
    )
    .expect("backend");

    backend.begin_synchronized_update().expect("BSU");
    Backend::flush(&mut backend).expect("inner flush");
    assert!(
        capture.bytes().is_empty(),
        "inner flush must not expose a partial frame"
    );
    backend.end_synchronized_update().expect("ESU/present");
    assert!(
        backend
            .drain_barrier()
            .wait_drained(Duration::from_secs(1))
            .expect("drain")
    );

    let bytes = String::from_utf8_lossy(&capture.bytes()).into_owned();
    assert!(bytes.contains("\x1b[?2026h"), "{bytes:?}");
    assert!(bytes.contains("\x1b[?2026l"), "{bytes:?}");
    let stats = backend.drain_barrier().latest_write_stats().expect("stats");
    assert!(stats.elapsed >= Duration::from_millis(20), "{stats:?}");
}

#[test]
fn delayed_frame_is_followed_by_one_final_restore_tail() {
    let capture = Capture::default();
    let mut backend = FrameCrosstermBackend::new(
        capture.clone(),
        FrameWriterOptions {
            write_delay: Duration::from_millis(30),
        },
    )
    .expect("backend");

    backend.begin_synchronized_update().expect("BSU");
    backend.hide_cursor().expect("old frame cursor");
    backend.end_synchronized_update().expect("old frame ESU");
    backend.begin_terminal_restore().expect("restore prefix");
    backend.finish_terminal_restore().expect("restore suffix");
    backend
        .write_drop_trailing_newline()
        .expect("present teardown tail");
    assert!(
        backend
            .drain_output(Duration::from_secs(1))
            .expect("drain teardown")
    );

    let bytes = capture.bytes();
    let restore = crate::engine::restore_seq::RESTORE_SEQ;
    let restore_start = bytes
        .windows(restore.len())
        .position(|window| window == restore)
        .expect("restore sequence");
    let old_esu = bytes
        .windows(b"\x1b[?2026l".len())
        .position(|window| window == b"\x1b[?2026l")
        .expect("old ESU");
    assert!(old_esu < restore_start);
    assert!(bytes.ends_with(b"\r\n"));
}
