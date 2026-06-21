use crate::protocol::BridgeInMessage;
use crate::protocol::BridgeOutMessage;
use crate::server::BridgeServer;

#[tokio::test]
async fn test_send_and_receive() {
    let server = BridgeServer::new();
    let mut rx = server.subscribe_outgoing();

    server
        .send(BridgeOutMessage::Text {
            content: "hello".into(),
        })
        .unwrap();

    let msg = rx.recv().await.unwrap();
    match msg {
        BridgeOutMessage::Text { content } => assert_eq!(content, "hello"),
        _ => panic!("unexpected message type"),
    }
}

#[tokio::test]
async fn test_incoming_channel() {
    let mut server = BridgeServer::new();
    let tx = server.incoming_sender();
    let mut rx = server.take_incoming_receiver().unwrap();

    tx.send(BridgeInMessage::Ping).await.unwrap();
    let msg = rx.recv().await.unwrap();
    assert!(matches!(msg, BridgeInMessage::Ping));
}

#[tokio::test]
async fn test_send_text_helper() {
    let server = BridgeServer::new();
    let mut rx = server.subscribe_outgoing();

    server.send_text("test message").unwrap();
    let msg = rx.recv().await.unwrap();
    match msg {
        BridgeOutMessage::Text { content } => assert_eq!(content, "test message"),
        _ => panic!("unexpected message"),
    }
}
