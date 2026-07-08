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
pub fn ndjson_named_pipe_server_connection(
    stream: tokio::net::windows::named_pipe::NamedPipeServer,
) -> NdjsonNamedPipeServerConnection {
    ndjson_named_pipe_server_connection_with_max_frame_bytes(stream, DEFAULT_MAX_NDJSON_FRAME_BYTES)
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
        let listener = tokio::net::UnixListener::bind(&path)
            .map_err(|source| TransportFrameError::Io { source })?;
        Ok(Self {
            listener: Some(listener),
            max_frame_bytes,
            socket_path: Some(path),
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
        self.listener
            .take()
            .expect("listener is present before into_inner")
    }
}

#[cfg(unix)]
impl Drop for NdjsonUnixListener {
    fn drop(&mut self) {
        drop(self.listener.take());
        let Some(path) = self.socket_path.take() else {
            return;
        };
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            return;
        };
        if std::os::unix::fs::FileTypeExt::is_socket(&metadata.file_type()) {
            let _ = std::fs::remove_file(path);
        }
    }
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

#[cfg(unix)]
pub fn bind_ndjson_unix_listener_with_max_frame_bytes(
    path: impl AsRef<std::path::Path>,
    max_frame_bytes: usize,
) -> Result<NdjsonUnixListener, TransportFrameError> {
    NdjsonUnixListener::bind_with_max_frame_bytes(path, max_frame_bytes)
}

#[cfg(windows)]
pub fn bind_ndjson_named_pipe_listener(
    pipe_name: impl AsRef<str>,
) -> Result<NdjsonNamedPipeListener, TransportFrameError> {
    NdjsonNamedPipeListener::bind(pipe_name)
}

#[cfg(windows)]
pub fn bind_ndjson_named_pipe_listener_with_max_frame_bytes(
    pipe_name: impl AsRef<str>,
    max_frame_bytes: usize,
) -> Result<NdjsonNamedPipeListener, TransportFrameError> {
    NdjsonNamedPipeListener::bind_with_max_frame_bytes(pipe_name, max_frame_bytes)
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
mod tests {
    use serde_json::json;
    use tokio::io::AsyncReadExt;
    use tokio::io::BufReader;
    use tokio::io::split;

    use super::*;

    #[test]
    fn request_round_trips_with_string_id_and_params() {
        let frame = JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::String("req-1".to_string()),
            "session/subscribe",
            Some(json!({ "sessionId": "sess-1", "afterSeq": 4 })),
        ));

        let encoded = serde_json::to_value(&frame).expect("encode frame");
        let decoded: JsonRpcFrame = serde_json::from_value(encoded.clone()).expect("decode frame");

        assert_eq!(decoded, frame);
        assert_eq!(
            encoded,
            json!({
                "jsonrpc": "2.0",
                "id": "req-1",
                "method": "session/subscribe",
                "params": { "sessionId": "sess-1", "afterSeq": 4 }
            })
        );
    }

    #[test]
    fn notification_has_no_id() {
        let frame = JsonRpcFrame::Notification(JsonRpcNotification::new(
            "session/event",
            Some(json!({ "surfaceId": "surface-1" })),
        ));

        let encoded = serde_json::to_value(&frame).expect("encode frame");

        assert_eq!(
            encoded,
            json!({
                "jsonrpc": "2.0",
                "method": "session/event",
                "params": { "surfaceId": "surface-1" }
            })
        );
    }

    #[test]
    fn response_frames_preserve_number_and_null_ids() {
        let success = JsonRpcFrame::Success(JsonRpcSuccess::new(JsonRpcId::Number(7), json!(true)));
        let error = JsonRpcFrame::Error(JsonRpcErrorResponse::new(
            JsonRpcId::Null,
            JsonRpcErrorObject::new(-32600, "invalid request", Some(json!({ "field": "id" }))),
        ));

        assert_eq!(
            serde_json::from_value::<JsonRpcFrame>(
                serde_json::to_value(&success).expect("encode success")
            )
            .expect("decode success"),
            success
        );
        assert_eq!(
            serde_json::from_value::<JsonRpcFrame>(
                serde_json::to_value(&error).expect("encode error")
            )
            .expect("decode error"),
            error
        );
    }

    #[test]
    fn invalid_jsonrpc_version_is_rejected() {
        let err = serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "1.0",
            "id": "req-1",
            "method": "session/read"
        }))
        .expect_err("invalid version should fail");

        assert!(err.to_string().contains("unsupported jsonrpc version"));
    }

    #[test]
    fn ndjson_encode_appends_newline_and_escapes_inner_newlines() {
        let frame = JsonRpcFrame::Notification(JsonRpcNotification::new(
            "session/event",
            Some(json!({ "message": "hello\nworld" })),
        ));

        let encoded = encode_ndjson_frame(&frame).expect("encode ndjson");

        assert!(encoded.ends_with(b"\n"));
        assert_eq!(encoded.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert_eq!(
            decode_ndjson_frame(&encoded).expect("decode encoded frame"),
            frame
        );
    }

    #[test]
    fn ndjson_decode_accepts_lf_and_crlf_records() {
        let lf = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":true}\n";
        let crlf = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":true}\r\n";

        let expected =
            JsonRpcFrame::Success(JsonRpcSuccess::new(JsonRpcId::Number(7), json!(true)));

        assert_eq!(decode_ndjson_frame(lf).expect("decode lf"), expected);
        assert_eq!(decode_ndjson_frame(crlf).expect("decode crlf"), expected);
    }

    #[test]
    fn ndjson_decode_rejects_empty_records() {
        assert!(matches!(
            decode_ndjson_frame(b"\n"),
            Err(TransportFrameError::EmptyFrame)
        ));
        assert!(matches!(
            decode_ndjson_frame(b"\r\n"),
            Err(TransportFrameError::EmptyFrame)
        ));
    }

    #[test]
    fn ndjson_decode_rejects_oversized_records_before_parsing() {
        let oversized = br#"{"jsonrpc":"2.0","id":7,"result":true}"#;
        let err =
            decode_ndjson_frame_with_limit(oversized, 8).expect_err("oversized frame should fail");

        match err {
            TransportFrameError::FrameTooLarge { actual, max } => {
                assert_eq!(actual, oversized.len());
                assert_eq!(max, 8);
            }
            other => panic!("expected FrameTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn ndjson_decode_reports_malformed_json() {
        let err = decode_ndjson_frame(b"{not-json}\n").expect_err("malformed json should fail");

        assert!(matches!(err, TransportFrameError::Decode { .. }));
    }

    #[tokio::test]
    async fn ndjson_reader_returns_frames_until_clean_eof() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":\"req-1\",\"method\":\"session/read\"}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"session/event\"}\n"
        );
        let mut reader = NdjsonFrameReader::new(BufReader::new(input.as_bytes()));

        assert_eq!(
            reader.read_frame().await.expect("read request"),
            Some(JsonRpcFrame::Request(JsonRpcRequest::new(
                JsonRpcId::String("req-1".to_string()),
                "session/read",
                None
            )))
        );
        assert_eq!(
            reader.read_frame().await.expect("read notification"),
            Some(JsonRpcFrame::Notification(JsonRpcNotification::new(
                "session/event",
                None
            )))
        );
        assert_eq!(reader.read_frame().await.expect("read eof"), None);
    }

    #[tokio::test]
    async fn ndjson_reader_accepts_final_record_without_newline() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":true}";
        let mut reader = NdjsonFrameReader::new(BufReader::new(&input[..]));

        assert_eq!(
            reader.read_frame().await.expect("read partial eof"),
            Some(JsonRpcFrame::Success(JsonRpcSuccess::new(
                JsonRpcId::Number(7),
                json!(true)
            )))
        );
        assert_eq!(reader.read_frame().await.expect("read eof"), None);
    }

    #[tokio::test]
    async fn ndjson_reader_rejects_oversized_record_before_decode() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":true}\n";
        let mut reader = NdjsonFrameReader::with_max_frame_bytes(BufReader::new(&input[..]), 8);

        let err = reader
            .read_frame()
            .await
            .expect_err("oversized frame should fail");

        match err {
            TransportFrameError::FrameTooLarge { actual, max } => {
                assert_eq!(actual, input.len());
                assert_eq!(max, 8);
            }
            other => panic!("expected FrameTooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ndjson_writer_serializes_one_frame_per_line() {
        let (client, server) = tokio::io::duplex(1024);
        let frame = JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::String("req-1".to_string()),
            "session/read",
            Some(json!({ "sessionId": "sess-1" })),
        ));
        let mut writer = NdjsonFrameWriter::new(client);

        writer.write_frame(&frame).await.expect("write frame");
        drop(writer);

        let mut bytes = Vec::new();
        BufReader::new(server)
            .read_to_end(&mut bytes)
            .await
            .expect("read written bytes");

        assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert_eq!(
            decode_ndjson_frame(&bytes).expect("decode written bytes"),
            frame
        );
    }

    #[tokio::test]
    async fn ndjson_duplex_connection_sends_and_receives_frames() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = split(client_stream);
        let (server_read, server_write) = split(server_stream);
        let mut client = NdjsonDuplexConnection::new(BufReader::new(client_read), client_write);
        let mut server = NdjsonDuplexConnection::new(BufReader::new(server_read), server_write);
        let request = JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::String("req-1".to_string()),
            "session/read",
            Some(json!({ "sessionId": "sess-1" })),
        ));
        let response = JsonRpcFrame::Success(JsonRpcSuccess::new(
            JsonRpcId::String("req-1".to_string()),
            json!({ "ok": true }),
        ));

        client
            .send_frame(&request)
            .await
            .expect("client sends request");
        assert_eq!(
            server.recv_frame().await.expect("server reads request"),
            Some(request)
        );

        server
            .send_frame(&response)
            .await
            .expect("server sends response");
        assert_eq!(
            client.recv_frame().await.expect("client reads response"),
            Some(response)
        );
    }

    #[tokio::test]
    async fn ndjson_duplex_connection_close_blocks_future_sends_and_receives() {
        let input = b"{\"jsonrpc\":\"2.0\",\"method\":\"session/event\"}\n";
        let mut connection =
            NdjsonDuplexConnection::new(BufReader::new(&input[..]), tokio::io::sink());

        connection.close().await.expect("close connection");

        assert!(!connection.is_open());
        assert!(matches!(
            connection
                .send_frame(&JsonRpcFrame::Notification(JsonRpcNotification::new(
                    "session/event",
                    None
                )))
                .await,
            Err(TransportFrameError::Closed)
        ));
        assert!(matches!(
            connection.recv_frame().await,
            Err(TransportFrameError::Closed)
        ));
    }

    #[tokio::test]
    async fn ndjson_duplex_connection_clean_eof_marks_closed() {
        let mut connection =
            NdjsonDuplexConnection::new(BufReader::new(&b""[..]), tokio::io::sink());

        assert_eq!(connection.recv_frame().await.expect("read eof"), None);
        assert!(!connection.is_open());
        assert!(matches!(
            connection
                .send_frame(&JsonRpcFrame::Notification(JsonRpcNotification::new(
                    "session/event",
                    None
                )))
                .await,
            Err(TransportFrameError::Closed)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ndjson_unix_connection_sends_and_receives_frames() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("app-server.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind unix listener");
        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept unix stream");
            ndjson_unix_connection(stream)
        });

        let mut client = connect_ndjson_unix(&socket_path)
            .await
            .expect("connect unix stream");
        let mut server = server_task.await.expect("server task");
        let request = JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::String("req-uds".to_string()),
            "control/keepAlive",
            Some(json!({})),
        ));
        let response = JsonRpcFrame::Success(JsonRpcSuccess::new(
            JsonRpcId::String("req-uds".to_string()),
            json!({ "ok": true }),
        ));

        client
            .send_frame(&request)
            .await
            .expect("client sends request");
        assert_eq!(
            server.recv_frame().await.expect("server reads request"),
            Some(request)
        );

        server
            .send_frame(&response)
            .await
            .expect("server sends response");
        assert_eq!(
            client.recv_frame().await.expect("client reads response"),
            Some(response)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ndjson_unix_listener_accepts_framed_connections() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("app-server.sock");
        let listener = bind_ndjson_unix_listener(&socket_path).expect("bind unix listener");
        let server_task =
            tokio::spawn(async move { listener.accept().await.expect("accept unix stream") });

        let mut client = connect_ndjson_unix(&socket_path)
            .await
            .expect("connect unix stream");
        let mut server = server_task.await.expect("server task");
        let request = JsonRpcFrame::Request(JsonRpcRequest::new(
            JsonRpcId::String("req-listener".to_string()),
            "control/keepAlive",
            Some(json!({})),
        ));
        let response = JsonRpcFrame::Success(JsonRpcSuccess::new(
            JsonRpcId::String("req-listener".to_string()),
            json!({ "ok": true }),
        ));

        client
            .send_frame(&request)
            .await
            .expect("client sends request");
        assert_eq!(
            server.recv_frame().await.expect("server reads request"),
            Some(request)
        );

        server
            .send_frame(&response)
            .await
            .expect("server sends response");
        assert_eq!(
            client.recv_frame().await.expect("client reads response"),
            Some(response)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ndjson_unix_listener_removes_socket_path_on_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("app-server.sock");
        {
            let _listener = bind_ndjson_unix_listener(&socket_path).expect("bind unix listener");
            assert!(socket_path.exists());
        }

        assert!(
            !socket_path.exists(),
            "dropping the listener should remove its socket file"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ndjson_unix_listener_into_inner_transfers_socket_cleanup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("app-server.sock");
        let listener = bind_ndjson_unix_listener(&socket_path).expect("bind unix listener");
        let inner = listener.into_inner();
        drop(inner);

        assert!(
            socket_path.exists(),
            "into_inner hands socket lifecycle to the caller"
        );
        std::fs::remove_file(&socket_path).expect("cleanup socket");
    }
}
