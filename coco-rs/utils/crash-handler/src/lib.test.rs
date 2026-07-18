use super::*;
use pretty_assertions::assert_eq;

#[test]
fn arm_disarm_and_rearm_are_consistent() {
    static SEQ: &[u8] = b"\x1b[?2026l\x1b[<u";
    assert!(!is_armed());

    arm_tui_restore(SEQ);
    assert!(is_armed());
    assert_eq!(SEQUENCE.get().copied(), Some(SEQ));
    disarm_tui_restore();
    assert!(!is_armed());

    arm_tui_restore(SEQ);
    assert!(is_armed());
    assert_eq!(SEQUENCE.get().copied(), Some(SEQ));
    disarm_tui_restore();
    assert!(!is_armed());
}

#[cfg(unix)]
#[test]
fn fatal_signal_child_restores_pty() {
    const CHILD_ENV: &str = "COCO_CRASH_HANDLER_PTY_CHILD";
    const READY: &[u8] = b"coco-crash-test-ready";
    static RESTORE: &[u8] = b"\x1b[?2026l\x1b[<u\x1b[?25h";
    if std::env::var_os(CHILD_ENV).is_some() {
        install_terminal_restore_only();
        // Enter raw mode only after the handler captured the PTY's cooked
        // state, then fault exactly as a mid-frame TUI crash would.
        let mut raw = unsafe {
            let mut value = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(libc::STDIN_FILENO, &mut value), 0);
            value
        };
        unsafe {
            libc::cfmakeraw(&mut raw);
            assert_eq!(libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw), 0);
        }
        arm_tui_restore(RESTORE);
        unsafe {
            assert_eq!(
                libc::write(
                    libc::STDOUT_FILENO,
                    READY.as_ptr().cast::<libc::c_void>(),
                    READY.len(),
                ),
                READY.len() as isize,
            );
            libc::raise(libc::SIGSEGV);
        }
        panic!("SIGSEGV unexpectedly returned");
    }

    use std::fs::File;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::fd::FromRawFd;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    use std::process::Stdio;

    let (master, slave) = unsafe {
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ),
            0,
        );
        (File::from_raw_fd(master), File::from_raw_fd(slave))
    };
    let mut cooked = unsafe {
        let mut value = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(slave.as_raw_fd(), &mut value), 0);
        value
    };
    cooked.c_lflag |= libc::ICANON | libc::ECHO;
    unsafe {
        assert_eq!(
            libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &cooked),
            0
        );
    }

    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::fatal_signal_child_restores_pty",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .stdin(Stdio::from(slave.try_clone().expect("clone PTY slave")))
        // Keep stdin on the PTY for the raw/cooked assertion. Capture fd 1
        // through a pipe so Darwin cannot discard queued master-side bytes
        // when the fault closes the last slave descriptor.
        .stdout(Stdio::piped())
        .stderr(Stdio::from(slave.try_clone().expect("clone PTY slave")))
        .spawn()
        .expect("spawn faulting child");
    let mut child_stdout = child.stdout.take().expect("captured child stdout");
    let status = child.wait().expect("wait for faulting child");
    assert_eq!(status.signal(), Some(libc::SIGSEGV));

    let restored = unsafe {
        let mut value = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(slave.as_raw_fd(), &mut value), 0);
        value
    };
    assert_ne!(
        restored.c_lflag & libc::ICANON,
        0,
        "canonical mode not restored"
    );
    assert_ne!(restored.c_lflag & libc::ECHO, 0, "echo not restored");

    let mut output = Vec::new();
    child_stdout
        .read_to_end(&mut output)
        .expect("read crash-handler stdout");
    drop(slave);
    drop(master);
    assert!(
        output.windows(READY.len()).any(|window| window == READY),
        "child readiness marker missing from PTY output: {output:?}"
    );
    assert!(
        output
            .windows(RESTORE.len())
            .any(|window| window == RESTORE),
        "restore bytes missing from PTY output: {output:?}"
    );
}
