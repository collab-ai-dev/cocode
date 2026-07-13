use std::sync::Arc;

use coco_app_server::{
    DisconnectOutcome, JsonRpcAdapterConnection, JsonRpcAdapterError, JsonRpcRequestHandler,
};
use coco_app_server_transport::JsonRpcFrame;
use coco_error::StackError;
use coco_types::CoreEvent;
use tokio::sync::mpsc;

use coco_agent_host::remote_host::{
    RemoteAppServerBridgeHost, RemoteJsonRpcConnection, RemoteOutboundMessage,
};

use super::{
    dispatcher::spawn_sdk_outbound_writer,
    transport::{SdkTransport, TransportError},
};

const APP_SERVER_SDK_FRAME_CHANNEL_CAPACITY: usize = 128;

pub async fn run_app_server_connection_over_sdk_transport_with_external_notifications_and_hub_connector(
    connection: RemoteJsonRpcConnection,
    transport: Arc<dyn SdkTransport>,
    bridge_host: RemoteAppServerBridgeHost,
    external_notifications: Vec<mpsc::Receiver<CoreEvent>>,
) -> Result<DisconnectOutcome, RemoteAppServerBridgeError> {
    let app_server = connection.app_server();
    let binding = bridge_host.open_connection_binding(
        Arc::clone(&app_server),
        connection.connection_key(),
        external_notifications,
        256,
    );
    let writer_task = spawn_sdk_outbound_writer(
        Arc::clone(&transport),
        binding.outbound_rx,
        Arc::clone(&app_server),
        bridge_host.session_seq_allocator(),
        bridge_host.hub_connector(),
    );
    let result = drive_app_server_connection_over_sdk_transport(
        connection,
        transport,
        binding.handler,
        Some(binding.outbound_tx.clone()),
    )
    .await;

    for forwarder in binding.external_forwarders {
        forwarder.abort();
        let _ = forwarder.await;
    }
    drop(binding.outbound_tx);
    writer_task
        .await
        .map_err(RemoteAppServerBridgeError::join)?;
    result
}

async fn drive_app_server_connection_over_sdk_transport<H, Handler>(
    connection: JsonRpcAdapterConnection<H>,
    transport: Arc<dyn SdkTransport>,
    handler: Arc<Handler>,
    outbound_messages: Option<mpsc::Sender<RemoteOutboundMessage>>,
) -> Result<DisconnectOutcome, RemoteAppServerBridgeError>
where
    H: Clone + Send + Sync + 'static,
    Handler: JsonRpcRequestHandler,
{
    let (inbound_tx, inbound_rx) =
        mpsc::channel::<JsonRpcFrame>(APP_SERVER_SDK_FRAME_CHANNEL_CAPACITY);
    let (outbound_tx, mut outbound_rx) =
        mpsc::channel::<JsonRpcFrame>(APP_SERVER_SDK_FRAME_CHANNEL_CAPACITY);

    let reader_transport = Arc::clone(&transport);
    let mut reader_task = tokio::spawn(async move {
        loop {
            let Some(frame) = reader_transport.recv_frame().await? else {
                break Ok(());
            };
            if inbound_tx.send(frame).await.is_err() {
                break Ok(());
            }
        }
    });

    let writer_transport = Arc::clone(&transport);
    let outbound_messages_for_frames = outbound_messages.clone();
    let writer_task = tokio::spawn(async move {
        while let Some(frame) = outbound_rx.recv().await {
            if let Some(outbound_messages) = &outbound_messages_for_frames {
                outbound_messages
                    .send(RemoteOutboundMessage::JsonRpcFrame(frame))
                    .await
                    .map_err(|_| TransportError::PeerDropped)?;
            } else {
                writer_transport.send_frame(frame).await?;
            }
        }
        Ok::<(), RemoteAppServerBridgeError>(())
    });

    let owner = connection.run_frame_channels(inbound_rx, outbound_tx, handler);
    tokio::pin!(owner);
    let owner_result = tokio::select! {
        result = &mut owner => result.map_err(RemoteAppServerBridgeError::from),
        reader = &mut reader_task => {
            match reader.map_err(RemoteAppServerBridgeError::join)? {
                Ok(()) => owner.await.map_err(RemoteAppServerBridgeError::from),
                Err(error) => {
                    let _ = owner.await;
                    Err(error)
                }
            }
        }
    };

    if !reader_task.is_finished() {
        reader_task.abort();
        let _ = reader_task.await;
    }
    writer_task
        .await
        .map_err(RemoteAppServerBridgeError::join)??;
    owner_result
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteAppServerBridgeError {
    #[error("{source}")]
    Adapter { source: JsonRpcAdapterError },
    #[error("{source}")]
    Transport { source: TransportError },
    #[error("SDK app-server bridge task failed: {source}")]
    Join { source: tokio::task::JoinError },
}

impl StackError for RemoteAppServerBridgeError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl RemoteAppServerBridgeError {
    fn join(source: tokio::task::JoinError) -> Self {
        Self::Join { source }
    }
}

impl From<JsonRpcAdapterError> for RemoteAppServerBridgeError {
    fn from(source: JsonRpcAdapterError) -> Self {
        Self::Adapter { source }
    }
}

impl From<TransportError> for RemoteAppServerBridgeError {
    fn from(source: TransportError) -> Self {
        Self::Transport { source }
    }
}
