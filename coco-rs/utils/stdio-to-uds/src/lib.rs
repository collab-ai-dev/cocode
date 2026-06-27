#![deny(clippy::print_stdout)]

use std::io;
use std::io::Write;
use std::net::Shutdown;
use std::path::Path;
use std::path::PathBuf;
use std::thread;

#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[cfg(windows)]
use uds_windows::UnixStream;

#[derive(Debug, thiserror::Error)]
pub enum StdioBridgeError {
    #[error("failed to connect to socket at {path}")]
    Connect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to clone socket for reading")]
    CloneSocket(#[source] io::Error),

    #[error("failed to copy data from stdin to socket")]
    StdinCopy(#[source] io::Error),

    #[error("failed to shutdown socket writer")]
    Shutdown(#[source] io::Error),

    #[error("reader thread panicked while copying socket data to stdout")]
    ReaderPanicked,

    #[error("failed to copy data from socket to stdout")]
    StdoutCopy(#[source] io::Error),
}

pub type Result<T, E = StdioBridgeError> = std::result::Result<T, E>;

/// Connects to the Unix Domain Socket at `socket_path` and relays data between
/// standard input/output and the socket.
pub fn run(socket_path: &Path) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path).map_err(|e| StdioBridgeError::Connect {
        path: socket_path.to_path_buf(),
        source: e,
    })?;

    let mut reader = stream.try_clone().map_err(StdioBridgeError::CloneSocket)?;

    let stdout_thread = thread::spawn(move || -> io::Result<()> {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        io::copy(&mut reader, &mut handle)?;
        handle.flush()?;
        Ok(())
    });

    let stdin = io::stdin();
    {
        let mut handle = stdin.lock();
        io::copy(&mut handle, &mut stream).map_err(StdioBridgeError::StdinCopy)?;
    }

    stream
        .shutdown(Shutdown::Write)
        .map_err(StdioBridgeError::Shutdown)?;

    let stdout_result = stdout_thread
        .join()
        .map_err(|_| StdioBridgeError::ReaderPanicked)?;
    stdout_result.map_err(StdioBridgeError::StdoutCopy)?;

    Ok(())
}
