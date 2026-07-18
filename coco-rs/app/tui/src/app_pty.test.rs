//! Real-PTY acceptance scenarios for plan items A1-A3.

#![allow(clippy::expect_used)]

use std::cell::Cell;
use std::fs::File;
use std::io;
use std::io::Read;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;

use crossterm::cursor::SetCursorStyle;
use ratatui::backend::Backend;
use ratatui::backend::ClearType;
use ratatui::backend::CrosstermBackend;
use ratatui::backend::WindowSize;
use ratatui::buffer::Buffer;
use ratatui::buffer::Cell as BufferCell;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use ratatui::widgets::Paragraph;

use super::App;
use crate::events::TuiEvent;
use crate::terminal::Tui;
use coco_tui_ui::engine::CursorClaim;
use coco_tui_ui::engine::compatibility::TerminalCompatibility;
use coco_tui_ui::engine::terminal::SurfaceBackend;
use coco_tui_ui::engine::terminal::SurfaceTerminal;
use coco_types::CoreEvent;

/// Crossterm's normal cursor query reads process-global stdin. This wrapper
/// keeps the production ANSI writer but tracks cursor/size locally so the app
/// can be driven against an isolated PTY without a DSR response pump.
struct PtyBackend {
    inner: CrosstermBackend<File>,
    size: Rc<Cell<Size>>,
    cursor: Position,
}

impl Backend for PtyBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a BufferCell)>,
    {
        self.inner.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.cursor = position;
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        Ok(self.size.get())
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size.get(),
            pixels: Size::new(0, 0),
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }

    fn scroll_region_up(
        &mut self,
        region: std::ops::Range<u16>,
        line_count: u16,
    ) -> io::Result<()> {
        self.inner.scroll_region_up(region, line_count)
    }

    fn scroll_region_down(
        &mut self,
        region: std::ops::Range<u16>,
        line_count: u16,
    ) -> io::Result<()> {
        self.inner.scroll_region_down(region, line_count)
    }
}

impl SurfaceBackend for PtyBackend {
    fn clear_scrollback_and_screen(&mut self) -> io::Result<()> {
        self.inner.clear_scrollback_and_screen()
    }

    fn set_cursor_style(&mut self, style: SetCursorStyle) -> io::Result<()> {
        self.inner.set_cursor_style(style)
    }

    fn begin_synchronized_update(&mut self) -> io::Result<()> {
        self.inner.begin_synchronized_update()
    }

    fn end_synchronized_update(&mut self) -> io::Result<()> {
        self.inner.end_synchronized_update()
    }

    fn enter_modal_alt_screen(&mut self) -> io::Result<()> {
        self.inner.enter_modal_alt_screen()
    }

    fn leave_modal_alt_screen(&mut self) -> io::Result<()> {
        self.inner.leave_modal_alt_screen()
    }

    fn begin_terminal_restore(&mut self) -> io::Result<()> {
        self.inner.begin_terminal_restore()
    }

    fn finish_terminal_restore(&mut self) -> io::Result<()> {
        self.inner.finish_terminal_restore()
    }

    fn write_drop_trailing_newline(&mut self) -> io::Result<()> {
        self.inner.write_drop_trailing_newline()
    }

    fn insert_history_rows_direct(
        &mut self,
        rendered: &Buffer,
        source_start_row: u16,
        row_count: u16,
        target_top: u16,
        scratch: &mut String,
    ) -> io::Result<Option<usize>> {
        self.inner.insert_history_rows_direct(
            rendered,
            source_start_row,
            row_count,
            target_top,
            scratch,
        )
    }
}

struct TestPty {
    resize_fd: File,
    size: Rc<Cell<Size>>,
    output: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}

/// Open a real kernel PTY without adding unsafe code to the TUI. `rustix`
/// owns the platform FFI; the slave receives production ANSI and the
/// non-blocking master captures exactly what a terminal sees.
fn test_pty(width: u16, height: u16) -> (TestPty, SurfaceTerminal<PtyBackend>) {
    use rustix::fs::Mode;
    use rustix::fs::OFlags;
    use rustix::pty::OpenptFlags;

    let master =
        rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).expect("open PTY master");
    rustix::pty::grantpt(&master).expect("grant PTY slave");
    rustix::pty::unlockpt(&master).expect("unlock PTY slave");
    let slave_name = rustix::pty::ptsname(&master, Vec::new()).expect("resolve PTY slave");
    let slave = rustix::fs::open(
        slave_name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY,
        Mode::empty(),
    )
    .expect("open PTY slave");
    set_winsize(&slave, width, height);
    let flags = rustix::fs::fcntl_getfl(&master).expect("read PTY flags");
    rustix::fs::fcntl_setfl(&master, flags | OFlags::NONBLOCK)
        .expect("make PTY master non-blocking");

    let master = File::from(master);
    let resize_fd = master.try_clone().expect("clone PTY master");
    let output = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let output = Arc::clone(&output);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut master = master;
            let mut chunk = [0u8; 4096];
            while !stop.load(Ordering::Acquire) {
                match master.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => output
                        .lock()
                        .expect("PTY output lock")
                        .extend_from_slice(&chunk[..read]),
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(err) if err.raw_os_error() == Some(libc::EIO) => break,
                    Err(err) => panic!("read PTY output: {err}"),
                }
            }
        })
    };
    let size = Rc::new(Cell::new(Size::new(width, height)));
    let backend = PtyBackend {
        inner: CrosstermBackend::new(File::from(slave)),
        size: Rc::clone(&size),
        cursor: Position::ORIGIN,
    };
    let terminal = SurfaceTerminal::new(backend).expect("build PTY terminal");
    (
        TestPty {
            resize_fd,
            size,
            output,
            stop,
            reader: Some(reader),
        },
        terminal,
    )
}

fn set_winsize(fd: &impl rustix::fd::AsFd, width: u16, height: u16) {
    rustix::termios::tcsetwinsize(
        fd,
        rustix::termios::Winsize {
            ws_row: height,
            ws_col: width,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .expect("set PTY window size");
}

impl TestPty {
    fn resize(&self, width: u16, height: u16) {
        set_winsize(&self.resize_fd, width, height);
        self.size.set(Size::new(width, height));
    }

    fn drain(&mut self) -> Vec<u8> {
        // The reader prevents the kernel PTY buffer from back-pressuring a
        // full viewport paint. Give it one scheduling quantum, then take the
        // bytes accumulated since the prior assertion.
        std::thread::sleep(Duration::from_millis(5));
        std::mem::take(&mut *self.output.lock().expect("PTY output lock"))
    }
}

impl Drop for TestPty {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            reader.join().expect("join PTY reader");
        }
    }
}

fn pty_test_app(
    out_of_band_repainter: bool,
) -> (
    App<PtyBackend>,
    tokio::sync::mpsc::Sender<CoreEvent>,
    TestPty,
) {
    let (pty, terminal) = test_pty(80, 24);
    let mut tui = Tui::new_for_test(terminal, TerminalCompatibility::NativeScrollback);
    tui.set_out_of_band_repainter_for_test(out_of_band_repainter);
    let (command_tx, _command_rx, event_tx, event_rx) = super::create_channels();
    let app = App::with_terminal(tui, command_tx, event_rx, std::path::PathBuf::from("."));
    (app, event_tx, pty)
}

#[test]
fn real_pty_idle_frame_emits_zero_bytes() {
    let (mut pty, mut terminal) = test_pty(40, 6);
    terminal.set_viewport_area(Rect::new(0, 0, 40, 3));

    let draw = |terminal: &mut SurfaceTerminal<PtyBackend>| {
        terminal.begin_synchronized_update()?;
        let draw_result = terminal.draw_viewport(|frame| {
            frame.render_widget(Paragraph::new("stable PTY frame"), frame.area());
            frame.set_cursor_claim(CursorClaim {
                position: Position { x: 4, y: 1 },
                style: SetCursorStyle::SteadyBar,
            });
        });
        draw_result.and(terminal.end_synchronized_update())
    };

    draw(&mut terminal).expect("first PTY frame");
    assert!(!pty.drain().is_empty(), "first frame must paint the PTY");
    draw(&mut terminal).expect("second PTY frame");
    assert_eq!(
        pty.drain(),
        Vec::<u8>::new(),
        "an identical frame must not write BSU, ESU, cells, or cursor escapes"
    );
}

#[tokio::test]
async fn real_pty_resize_burst_paints_only_the_settled_size() {
    let (mut app, _event_tx, mut pty) = pty_test_app(false);
    for width in [100, 92, 84, 76] {
        pty.resize(width, 30);
        assert!(
            !app.handle_event(TuiEvent::Resize { width, height: 30 })
                .await
        );
    }

    app.redraw().expect("suppressed intermediate PTY frame");
    assert!(
        pty.drain().is_empty(),
        "the resize quiet period must suppress intermediate PTY bytes"
    );
    tokio::time::sleep(crate::resize_debounce::RESIZE_QUIET_PERIOD).await;
    app.redraw().expect("settled PTY frame");
    assert_eq!(app.state.ui.terminal_size, Size::new(76, 30));
    assert!(!pty.drain().is_empty(), "settled size must paint the PTY");
}

#[tokio::test]
async fn real_pty_focus_gain_forces_full_repaint_only_when_gated() {
    let (mut plain, _event_tx, mut plain_pty) = pty_test_app(false);
    plain.redraw().expect("initial plain PTY frame");
    assert!(!plain_pty.drain().is_empty());
    assert!(
        plain
            .handle_event(TuiEvent::FocusChanged { focused: true })
            .await
    );
    plain.redraw().expect("plain focus PTY frame");
    let plain_bytes = plain_pty.drain();
    assert!(!plain_bytes.is_empty(), "focus must reassert the cursor");

    let (mut multiplexed, _event_tx, mut multiplexed_pty) = pty_test_app(true);
    multiplexed.redraw().expect("initial multiplexed PTY frame");
    assert!(!multiplexed_pty.drain().is_empty());
    assert!(
        multiplexed
            .handle_event(TuiEvent::FocusChanged { focused: true })
            .await
    );
    multiplexed.redraw().expect("healed multiplexed PTY frame");
    let healed_bytes = multiplexed_pty.drain();
    assert!(
        healed_bytes.len() > plain_bytes.len() + 100,
        "gated heal must repaint cells: plain={} healed={}",
        plain_bytes.len(),
        healed_bytes.len(),
    );
}
