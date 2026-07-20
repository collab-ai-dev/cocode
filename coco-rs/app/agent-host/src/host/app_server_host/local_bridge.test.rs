use std::sync::Arc;

use super::*;

#[tokio::test]
async fn bridge_command_clients_are_in_memory_views_of_one_connection() {
    let bridge = AppServerLocalBridge::new(Arc::new(AppServerHostState::default()));
    assert_eq!(
        bridge
            .app_server()
            .routing()
            .read()
            .expect("routing")
            .connection_count(),
        1
    );

    let first = bridge.connect_local_client();
    let second = bridge.connect_local_client();

    assert_eq!(first.connection_key(), bridge.client().connection_key());
    assert_eq!(second.connection_key(), bridge.client().connection_key());
    assert_eq!(
        bridge
            .app_server()
            .routing()
            .read()
            .expect("routing")
            .connection_count(),
        1
    );
}

#[tokio::test]
async fn dropping_the_bridge_disconnects_its_one_connection() {
    let bridge = AppServerLocalBridge::new(Arc::new(AppServerHostState::default()));
    let app_server = Arc::clone(bridge.app_server());
    assert_eq!(
        app_server
            .routing()
            .read()
            .expect("routing")
            .connection_count(),
        1
    );

    drop(bridge);
    tokio::task::yield_now().await;

    assert_eq!(
        app_server
            .routing()
            .read()
            .expect("routing")
            .connection_count(),
        0
    );
}
