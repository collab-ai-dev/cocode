use std::time::Duration;

use coco_app_server_transport::JsonRpcFrame;
use coco_app_server_transport::NdjsonDuplexConnection;
use futures::SinkExt;
use futures::StreamExt;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WebSocketMessage;

use super::RemoteJsonRpcIncoming;
use super::RemoteTransportError;

pub struct RemoteNdjsonConnection<R, W> {
    pub(super) incoming: RemoteJsonRpcIncoming,
    pub(super) outbound: mpsc::Receiver<JsonRpcFrame>,
    pub(super) transport: NdjsonDuplexConnection<R, W>,
    pub(super) write_timeout: Option<Duration>,
}

pub struct RemoteWebSocketConnection<S> {
    pub(super) incoming: RemoteJsonRpcIncoming,
    pub(super) outbound: mpsc::Receiver<JsonRpcFrame>,
    pub(super) websocket: WebSocketStream<S>,
    pub(super) write_timeout: Option<Duration>,
}

pub type RemoteDefaultWebSocketConnection =
    RemoteWebSocketConnection<MaybeTlsStream<tokio::net::TcpStream>>;

#[cfg(unix)]
pub type RemoteNdjsonUnixConnection = RemoteNdjsonConnection<
    tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
>;

#[cfg(windows)]
pub type RemoteNdjsonNamedPipeConnection = RemoteNdjsonConnection<
    tokio::io::BufReader<tokio::io::ReadHalf<tokio::net::windows::named_pipe::NamedPipeClient>>,
    tokio::io::WriteHalf<tokio::net::windows::named_pipe::NamedPipeClient>,
>;

impl<R, W> RemoteNdjsonConnection<R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    pub async fn run(self) -> Result<(), RemoteTransportError> {
        let RemoteNdjsonConnection {
            incoming,
            mut outbound,
            transport,
            write_timeout,
        } = self;
        let (mut reader, mut writer) = transport.split();
        // Every exit path breaks out of the loop so `incoming.disconnect()` always
        // runs; no `?` short-circuits past it.
        let result = loop {
            tokio::select! {
                frame = reader.read_frame() => {
                    match frame {
                        Ok(Some(frame)) => {
                            if let Err(source) = incoming.handle_frame(frame).await {
                                break Err(RemoteTransportError::Client { source });
                            }
                        }
                        Ok(None) => break Ok(()),
                        Err(source) => break Err(RemoteTransportError::Transport { source }),
                    }
                }
                frame = outbound.recv() => {
                    let Some(frame) = frame else {
                        break Ok(());
                    };
                    match write_frame_with_timeout(writer.write_frame(&frame), write_timeout).await {
                        Ok(Ok(())) => {}
                        Ok(Err(source)) => break Err(RemoteTransportError::Transport { source }),
                        Err(_elapsed) => break Err(RemoteTransportError::SlowConsumer),
                    }
                }
            }
        };
        incoming.disconnect().await;
        result
    }
}

/// Race an outbound write against `write_timeout`. `None` disables the bound.
/// Returns `Err (Elapsed)` only on timeout.
async fn write_frame_with_timeout<F, E>(
    write: F,
    write_timeout: Option<Duration>,
) -> Result<Result<(), E>, tokio::time::error::Elapsed>
where
    F: std::future::Future<Output = Result<(), E>>,
{
    match write_timeout {
        Some(timeout) => tokio::time::timeout(timeout, write).await,
        None => Ok(write.await),
    }
}

impl<S> RemoteWebSocketConnection<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn run(self) -> Result<(), RemoteTransportError> {
        let RemoteWebSocketConnection {
            incoming,
            mut outbound,
            mut websocket,
            write_timeout,
        } = self;
        // Every exit path breaks out of the loop so `incoming.disconnect()` always
        // runs; no `?` short-circuits past it.
        let result = loop {
            tokio::select! {
                message = websocket.next() => {
                    let Some(message) = message else {
                        break Ok(());
                    };
                    let message = match message {
                        Ok(message) => message,
                        Err(source) => break Err(RemoteTransportError::WebSocket { source }),
                    };
                    match remote_json_rpc_frame_from_websocket_message(message) {
                        Ok(RemoteWebSocketInboundFrame::Frame(frame)) => {
                            if let Err(source) = incoming.handle_frame(frame).await {
                                break Err(RemoteTransportError::Client { source });
                            }
                        }
                        Ok(RemoteWebSocketInboundFrame::Ignore) => {}
                        Ok(RemoteWebSocketInboundFrame::Closed) => break Ok(()),
                        Err(error) => break Err(error),
                    }
                }
                frame = outbound.recv() => {
                    let Some(frame) = frame else {
                        let _ = websocket.close(None).await;
                        break Ok(());
                    };
                    let write = write_remote_websocket_json_rpc_frame(&mut websocket, &frame);
                    match write_frame_with_timeout(write, write_timeout).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => break Err(error),
                        Err(_elapsed) => break Err(RemoteTransportError::SlowConsumer),
                    }
                }
            }
        };
        incoming.disconnect().await;
        result
    }
}

enum RemoteWebSocketInboundFrame {
    Frame(JsonRpcFrame),
    Ignore,
    Closed,
}

fn remote_json_rpc_frame_from_websocket_message(
    message: WebSocketMessage,
) -> Result<RemoteWebSocketInboundFrame, RemoteTransportError> {
    match message {
        WebSocketMessage::Text(text) => serde_json::from_str(text.as_ref())
            .map(RemoteWebSocketInboundFrame::Frame)
            .map_err(|source| RemoteTransportError::DecodeWebSocketFrame { source }),
        WebSocketMessage::Binary(bytes) => serde_json::from_slice(bytes.as_ref())
            .map(RemoteWebSocketInboundFrame::Frame)
            .map_err(|source| RemoteTransportError::DecodeWebSocketFrame { source }),
        WebSocketMessage::Close(_) => Ok(RemoteWebSocketInboundFrame::Closed),
        WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_) => {
            Ok(RemoteWebSocketInboundFrame::Ignore)
        }
        WebSocketMessage::Frame(_) => Ok(RemoteWebSocketInboundFrame::Ignore),
    }
}

async fn write_remote_websocket_json_rpc_frame<S>(
    websocket: &mut WebSocketStream<S>,
    frame: &JsonRpcFrame,
) -> Result<(), RemoteTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let text = serde_json::to_string(frame)
        .map_err(|source| RemoteTransportError::EncodeWebSocketFrame { source })?;
    websocket
        .send(WebSocketMessage::Text(text.into()))
        .await
        .map_err(|source| RemoteTransportError::WebSocket { source })
}
