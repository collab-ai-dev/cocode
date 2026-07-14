//! AppServer transport and wire framing.
//!
//! This Phase A slice owns JSON-RPC frame shapes and NDJSON per-record
//! encoding/decoding. Concrete transport tasks and adapter integration land
//! later.

use serde::Deserialize;
use serde::Serialize;
#[cfg(unix)]
use std::path::PathBuf;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
#[cfg(windows)]
use tokio::io::ReadHalf;
#[cfg(windows)]
use tokio::io::WriteHalf;

pub const JSONRPC_VERSION: &str = "2.0";
pub const DEFAULT_MAX_NDJSON_FRAME_BYTES: usize = 1024 * 1024;

pub type NdjsonStdioConnection =
    NdjsonDuplexConnection<BufReader<tokio::io::Stdin>, tokio::io::Stdout>;

#[cfg(unix)]
pub type NdjsonUnixConnection = NdjsonDuplexConnection<
    BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
>;

#[cfg(windows)]
pub type NdjsonNamedPipeClientConnection = NdjsonDuplexConnection<
    BufReader<ReadHalf<tokio::net::windows::named_pipe::NamedPipeClient>>,
    WriteHalf<tokio::net::windows::named_pipe::NamedPipeClient>,
>;

#[cfg(windows)]
pub type NdjsonNamedPipeServerConnection = NdjsonDuplexConnection<
    BufReader<ReadHalf<tokio::net::windows::named_pipe::NamedPipeServer>>,
    WriteHalf<tokio::net::windows::named_pipe::NamedPipeServer>,
>;

#[cfg(unix)]
pub struct NdjsonUnixListener {
    listener: Option<tokio::net::UnixListener>,
    max_frame_bytes: usize,
    socket_path: Option<PathBuf>,
    /// ` (dev, ino)` of the bound socket, recorded at bind so `Drop` unlinks
    /// only this listener's own socket and never a successor's at the same path.
    socket_identity: Option<(u64, u64)>,
}

#[cfg(windows)]
pub struct NdjsonNamedPipeListener {
    pipe_name: String,
    server: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    max_frame_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    String(String),
    Number(i64),
    Null,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcFrame {
    Request(JsonRpcRequest),
    Success(JsonRpcSuccess),
    Error(JsonRpcErrorResponse),
    Notification(JsonRpcNotification),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: JsonRpcVersion,
    pub id: JsonRpcId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: JsonRpcVersion,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcSuccess {
    pub jsonrpc: JsonRpcVersion,
    pub id: JsonRpcId,
    pub result: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: JsonRpcVersion,
    pub id: JsonRpcId,
    pub error: JsonRpcErrorObject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonRpcVersion;

#[derive(Debug, thiserror::Error)]
pub enum TransportFrameError {
    #[error("transport closed")]
    Closed,
    #[error("transport I/O error: {source}")]
    Io { source: std::io::Error },
    #[error("empty NDJSON frame")]
    EmptyFrame,
    #[error("NDJSON frame is {actual} bytes, exceeding the {max} byte limit")]
    FrameTooLarge { actual: usize, max: usize },
    #[error("failed to decode JSON-RPC frame: {source}")]
    Decode { source: serde_json::Error },
    #[error("failed to encode JSON-RPC frame: {source}")]
    Encode { source: serde_json::Error },
}

pub struct NdjsonFrameReader<R> {
    reader: R,
    max_frame_bytes: usize,
    buffer: Vec<u8>,
}

impl<R> NdjsonFrameReader<R>
where
    R: AsyncBufRead + Unpin,
{
    pub fn new(reader: R) -> Self {
        Self::with_max_frame_bytes(reader, DEFAULT_MAX_NDJSON_FRAME_BYTES)
    }

    pub fn with_max_frame_bytes(reader: R, max_frame_bytes: usize) -> Self {
        Self {
            reader,
            max_frame_bytes,
            buffer: Vec::new(),
        }
    }

    pub async fn read_frame(&mut self) -> Result<Option<JsonRpcFrame>, TransportFrameError> {
        loop {
            let (consumed, complete_record) = {
                let available = self
                    .reader
                    .fill_buf()
                    .await
                    .map_err(|source| TransportFrameError::Io { source })?;
                if available.is_empty() {
                    if self.buffer.is_empty() {
                        return Ok(None);
                    }
                    let line = std::mem::take(&mut self.buffer);
                    return decode_ndjson_frame_with_limit(&line, self.max_frame_bytes).map(Some);
                }

                let consumed = available
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map(|position| position + 1)
                    .unwrap_or(available.len());
                let actual = self.buffer.len() + consumed;
                if actual > self.max_frame_bytes {
                    return Err(TransportFrameError::FrameTooLarge {
                        actual,
                        max: self.max_frame_bytes,
                    });
                }

                self.buffer.extend_from_slice(&available[..consumed]);
                let complete_record = available[consumed - 1] == b'\n';
                (consumed, complete_record)
            };

            self.reader.consume(consumed);
            if complete_record {
                let line = std::mem::take(&mut self.buffer);
                return decode_ndjson_frame_with_limit(&line, self.max_frame_bytes).map(Some);
            }
        }
    }

    pub fn into_inner(self) -> R {
        self.reader
    }
}

pub struct NdjsonFrameWriter<W> {
    writer: W,
}

impl<W> NdjsonFrameWriter<W>
where
    W: AsyncWrite + Unpin,
{
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub async fn write_frame(&mut self, frame: &JsonRpcFrame) -> Result<(), TransportFrameError> {
        let encoded = encode_ndjson_frame(frame)?;
        self.writer
            .write_all(&encoded)
            .await
            .map_err(|source| TransportFrameError::Io { source })?;
        self.writer
            .flush()
            .await
            .map_err(|source| TransportFrameError::Io { source })
    }

    pub async fn flush(&mut self) -> Result<(), TransportFrameError> {
        self.writer
            .flush()
            .await
            .map_err(|source| TransportFrameError::Io { source })
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}

pub struct NdjsonDuplexConnection<R, W> {
    reader: NdjsonFrameReader<R>,
    writer: NdjsonFrameWriter<W>,
    open: bool,
}

impl<R, W> NdjsonDuplexConnection<R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self::with_max_frame_bytes(reader, writer, DEFAULT_MAX_NDJSON_FRAME_BYTES)
    }

    pub fn with_max_frame_bytes(reader: R, writer: W, max_frame_bytes: usize) -> Self {
        Self {
            reader: NdjsonFrameReader::with_max_frame_bytes(reader, max_frame_bytes),
            writer: NdjsonFrameWriter::new(writer),
            open: true,
        }
    }

    pub async fn recv_frame(&mut self) -> Result<Option<JsonRpcFrame>, TransportFrameError> {
        if !self.open {
            return Err(TransportFrameError::Closed);
        }

        let frame = self.reader.read_frame().await?;
        if frame.is_none() {
            self.open = false;
        }
        Ok(frame)
    }

    pub async fn send_frame(&mut self, frame: &JsonRpcFrame) -> Result<(), TransportFrameError> {
        if !self.open {
            return Err(TransportFrameError::Closed);
        }
        self.writer.write_frame(frame).await
    }

    pub async fn close(&mut self) -> Result<(), TransportFrameError> {
        if !self.open {
            return Ok(());
        }
        self.open = false;
        self.writer.flush().await
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn split(self) -> (NdjsonFrameReader<R>, NdjsonFrameWriter<W>) {
        (self.reader, self.writer)
    }

    pub fn into_inner(self) -> (R, W) {
        (self.reader.into_inner(), self.writer.into_inner())
    }
}

pub fn ndjson_stdio_connection() -> NdjsonStdioConnection {
    ndjson_stdio_connection_with_max_frame_bytes(DEFAULT_MAX_NDJSON_FRAME_BYTES)
}

pub fn ndjson_stdio_connection_with_max_frame_bytes(
    max_frame_bytes: usize,
) -> NdjsonStdioConnection {
    NdjsonDuplexConnection::with_max_frame_bytes(
        BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
        max_frame_bytes,
    )
}

#[cfg(unix)]
pub fn ndjson_unix_connection(stream: tokio::net::UnixStream) -> NdjsonUnixConnection {
    ndjson_unix_connection_with_max_frame_bytes(stream, DEFAULT_MAX_NDJSON_FRAME_BYTES)
}

#[cfg(unix)]
pub fn ndjson_unix_connection_with_max_frame_bytes(
    stream: tokio::net::UnixStream,
    max_frame_bytes: usize,
) -> NdjsonUnixConnection {
    let (read, write) = stream.into_split();
    NdjsonDuplexConnection::with_max_frame_bytes(BufReader::new(read), write, max_frame_bytes)
}

#[cfg(unix)]
pub async fn connect_ndjson_unix(
    path: impl AsRef<std::path::Path>,
) -> Result<NdjsonUnixConnection, TransportFrameError> {
    let stream = tokio::net::UnixStream::connect(path)
        .await
        .map_err(|source| TransportFrameError::Io { source })?;
    Ok(ndjson_unix_connection(stream))
}

#[cfg(windows)]
pub fn ndjson_named_pipe_client_connection(
    stream: tokio::net::windows::named_pipe::NamedPipeClient,
) -> NdjsonNamedPipeClientConnection {
    ndjson_named_pipe_client_connection_with_max_frame_bytes(stream, DEFAULT_MAX_NDJSON_FRAME_BYTES)
}

#[cfg(windows)]
pub fn ndjson_named_pipe_client_connection_with_max_frame_bytes(
    stream: tokio::net::windows::named_pipe::NamedPipeClient,
    max_frame_bytes: usize,
) -> NdjsonNamedPipeClientConnection {
    let (read, write) = tokio::io::split(stream);
    NdjsonDuplexConnection::with_max_frame_bytes(BufReader::new(read), write, max_frame_bytes)
}

#[cfg(windows)]
pub fn ndjson_named_pipe_server_connection_with_max_frame_bytes(
    stream: tokio::net::windows::named_pipe::NamedPipeServer,
    max_frame_bytes: usize,
) -> NdjsonNamedPipeServerConnection {
    let (read, write) = tokio::io::split(stream);
    NdjsonDuplexConnection::with_max_frame_bytes(BufReader::new(read), write, max_frame_bytes)
}

#[cfg(windows)]
pub fn connect_ndjson_named_pipe(
    pipe_name: impl AsRef<str>,
) -> Result<NdjsonNamedPipeClientConnection, TransportFrameError> {
    let stream = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(pipe_name.as_ref())
        .map_err(|source| TransportFrameError::Io { source })?;
    Ok(ndjson_named_pipe_client_connection(stream))
}

#[cfg(unix)]
impl NdjsonUnixListener {
    pub fn bind(path: impl AsRef<std::path::Path>) -> Result<Self, TransportFrameError> {
        Self::bind_with_max_frame_bytes(path, DEFAULT_MAX_NDJSON_FRAME_BYTES)
    }

    pub fn bind_with_max_frame_bytes(
        path: impl AsRef<std::path::Path>,
        max_frame_bytes: usize,
    ) -> Result<Self, TransportFrameError> {
        let path = path.as_ref().to_path_buf();
        let listener = bind_unix_listener_reclaiming_stale(&path)?;
        let socket_identity = unix_socket_identity(&path);
        Ok(Self {
            listener: Some(listener),
            max_frame_bytes,
            socket_path: Some(path),
            socket_identity,
        })
    }

    pub async fn accept(&self) -> Result<NdjsonUnixConnection, TransportFrameError> {
        let Some(listener) = self.listener.as_ref() else {
            return Err(TransportFrameError::Closed);
        };
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|source| TransportFrameError::Io { source })?;
        Ok(ndjson_unix_connection_with_max_frame_bytes(
            stream,
            self.max_frame_bytes,
        ))
    }

    pub fn into_inner(mut self) -> tokio::net::UnixListener {
        self.socket_path = None;
        match self.listener.take() {
            Some(listener) => listener,
            None => unreachable!("listener is present before into_inner"),
        }
    }
}

#[cfg(unix)]
impl Drop for NdjsonUnixListener {
    fn drop(&mut self) {
        drop(self.listener.take());
        let Some(path) = self.socket_path.take() else {
            return;
        };
        let Some(bound_identity) = self.socket_identity else {
            return;
        };
        // Only unlink when the file at `path` is still the exact inode this
        // listener bound. A successor that re-created the path owns a different
        // ` (dev, ino)`; leaving that file intact avoids deleting its socket. A
        // failed re-stat (missing/unreadable) also leaves the file alone.
        if unix_socket_identity(&path) == Some(bound_identity) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Bind a Unix listener, recovering from a stale socket file left behind by a
/// crashed process. On `AddrInUse`, probe liveness by connecting: a refused or
/// absent endpoint means the socket is stale, so unlink it and retry bind once;
/// a live endpoint means another server owns the path, so surface the error.
#[cfg(unix)]
fn bind_unix_listener_reclaiming_stale(
    path: &std::path::Path,
) -> Result<tokio::net::UnixListener, TransportFrameError> {
    match tokio::net::UnixListener::bind(path) {
        Ok(listener) => Ok(listener),
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            if unix_socket_is_live(path) {
                return Err(TransportFrameError::Io { source: err });
            }
            std::fs::remove_file(path).map_err(|source| TransportFrameError::Io { source })?;
            tokio::net::UnixListener::bind(path)
                .map_err(|source| TransportFrameError::Io { source })
        }
        Err(source) => Err(TransportFrameError::Io { source }),
    }
}

/// Return whether a live server is accepting on the Unix socket at `path`. A
/// successful connect means a live endpoint; any connect failure
/// (ConnectionRefused / ENOENT / …) means the socket file is stale.
#[cfg(unix)]
fn unix_socket_is_live(path: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Read the ` (dev, ino)` identity of the file at `path`, if it can be stat'd.
#[cfg(unix)]
fn unix_socket_identity(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
impl NdjsonNamedPipeListener {
    pub fn bind(pipe_name: impl AsRef<str>) -> Result<Self, TransportFrameError> {
        Self::bind_with_max_frame_bytes(pipe_name, DEFAULT_MAX_NDJSON_FRAME_BYTES)
    }

    pub fn bind_with_max_frame_bytes(
        pipe_name: impl AsRef<str>,
        max_frame_bytes: usize,
    ) -> Result<Self, TransportFrameError> {
        let pipe_name = pipe_name.as_ref().to_string();
        let server = create_named_pipe_server(&pipe_name)?;
        Ok(Self {
            pipe_name,
            server: Some(server),
            max_frame_bytes,
        })
    }

    pub async fn accept(&mut self) -> Result<NdjsonNamedPipeServerConnection, TransportFrameError> {
        let Some(server) = self.server.take() else {
            return Err(TransportFrameError::Closed);
        };
        server
            .connect()
            .await
            .map_err(|source| TransportFrameError::Io { source })?;
        self.server = Some(create_named_pipe_server(&self.pipe_name)?);
        Ok(ndjson_named_pipe_server_connection_with_max_frame_bytes(
            server,
            self.max_frame_bytes,
        ))
    }

    pub fn into_inner(mut self) -> Option<tokio::net::windows::named_pipe::NamedPipeServer> {
        self.server.take()
    }
}

#[cfg(windows)]
fn create_named_pipe_server(
    pipe_name: &str,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer, TransportFrameError> {
    tokio::net::windows::named_pipe::ServerOptions::new()
        .create(pipe_name)
        .map_err(|source| TransportFrameError::Io { source })
}

#[cfg(unix)]
pub fn bind_ndjson_unix_listener(
    path: impl AsRef<std::path::Path>,
) -> Result<NdjsonUnixListener, TransportFrameError> {
    NdjsonUnixListener::bind(path)
}

#[cfg(windows)]
pub fn bind_ndjson_named_pipe_listener(
    pipe_name: impl AsRef<str>,
) -> Result<NdjsonNamedPipeListener, TransportFrameError> {
    NdjsonNamedPipeListener::bind(pipe_name)
}

impl JsonRpcRequest {
    pub fn new(
        id: JsonRpcId,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Self {
        Self {
            jsonrpc: JsonRpcVersion,
            id,
            method: method.into(),
            params,
        }
    }
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JsonRpcVersion,
            method: method.into(),
            params,
        }
    }
}

impl JsonRpcSuccess {
    pub fn new(id: JsonRpcId, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: JsonRpcVersion,
            id,
            result,
        }
    }
}

impl JsonRpcErrorResponse {
    pub fn new(id: JsonRpcId, error: JsonRpcErrorObject) -> Self {
        Self {
            jsonrpc: JsonRpcVersion,
            id,
            error,
        }
    }
}

impl JsonRpcErrorObject {
    pub fn new(code: i32, message: impl Into<String>, data: Option<serde_json::Value>) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }
}

pub fn encode_ndjson_frame(frame: &JsonRpcFrame) -> Result<Vec<u8>, TransportFrameError> {
    let mut encoded =
        serde_json::to_vec(frame).map_err(|source| TransportFrameError::Encode { source })?;
    encoded.push(b'\n');
    Ok(encoded)
}

pub fn decode_ndjson_frame(line: &[u8]) -> Result<JsonRpcFrame, TransportFrameError> {
    decode_ndjson_frame_with_limit(line, DEFAULT_MAX_NDJSON_FRAME_BYTES)
}

pub fn decode_ndjson_frame_with_limit(
    line: &[u8],
    max_frame_bytes: usize,
) -> Result<JsonRpcFrame, TransportFrameError> {
    if line.len() > max_frame_bytes {
        return Err(TransportFrameError::FrameTooLarge {
            actual: line.len(),
            max: max_frame_bytes,
        });
    }

    let line = trim_ndjson_line_ending(line);
    if line.is_empty() {
        return Err(TransportFrameError::EmptyFrame);
    }

    serde_json::from_slice(line).map_err(|source| TransportFrameError::Decode { source })
}

fn trim_ndjson_line_ending(line: &[u8]) -> &[u8] {
    let Some(line) = line.strip_suffix(b"\n") else {
        return line;
    };
    line.strip_suffix(b"\r").unwrap_or(line)
}

impl Serialize for JsonRpcVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(JSONRPC_VERSION)
    }
}

impl<'de> Deserialize<'de> for JsonRpcVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value == JSONRPC_VERSION {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported jsonrpc version: {value}"
            )))
        }
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
