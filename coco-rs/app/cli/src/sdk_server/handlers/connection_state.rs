use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::sync::mpsc;

use crate::sdk_server::outbound::OutboundMessage;
use crate::sdk_server::transport::SdkTransport;

#[derive(Default)]
pub(super) struct ConnectionState {
    transport: RwLock<Option<Arc<dyn SdkTransport>>>,
    outbound_tx: RwLock<Option<mpsc::Sender<OutboundMessage>>>,
}

impl ConnectionState {
    pub(super) fn install_transport_for_startup(&self, transport: Arc<dyn SdkTransport>) {
        let Ok(mut slot) = self.transport.try_write() else {
            panic!("SdkServer::new: state was already locked at construction time");
        };
        *slot = Some(transport);
    }

    pub(super) async fn install_transport(&self, transport: Arc<dyn SdkTransport>) {
        let mut slot = self.transport.write().await;
        *slot = Some(transport);
    }

    pub(super) async fn transport_snapshot(&self) -> Option<Arc<dyn SdkTransport>> {
        self.transport.read().await.clone()
    }

    pub(super) async fn install_outbound_tx(&self, tx: mpsc::Sender<OutboundMessage>) {
        let mut slot = self.outbound_tx.write().await;
        *slot = Some(tx);
    }

    pub(super) async fn clear_outbound_tx(&self) {
        let mut slot = self.outbound_tx.write().await;
        *slot = None;
    }

    pub(super) async fn outbound_tx_snapshot(&self) -> Option<mpsc::Sender<OutboundMessage>> {
        self.outbound_tx.read().await.clone()
    }

    #[cfg(test)]
    pub(super) async fn has_outbound_tx(&self) -> bool {
        self.outbound_tx.read().await.is_some()
    }
}
