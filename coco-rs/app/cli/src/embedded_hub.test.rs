use clap::Parser;

use super::*;

#[cfg(not(feature = "serve-hub"))]
#[tokio::test]
async fn serve_hub_without_feature_returns_actionable_error() {
    let mut cli = Cli::parse_from(["coco", "--serve-hub"]);

    let Err(error) = start_if_requested(&mut cli).await else {
        panic!("serve-hub should require feature");
    };
    let message = error.to_string();

    assert!(message.contains("serve-hub"));
    assert!(message.contains("coco-hub-server serve"));
}

#[cfg(feature = "serve-hub")]
#[tokio::test]
async fn serve_hub_with_feature_sets_event_hub_url() {
    let mut cli = Cli::parse_from(["coco", "--serve-hub", "--hub-port", "0"]);

    let guard = start_if_requested(&mut cli)
        .await
        .expect("embedded hub starts")
        .expect("guard returned");

    let url = cli.event_hub_url.as_deref().expect("url set");
    assert!(url.starts_with("ws://127.0.0.1:"));
    assert!(url.ends_with("/v1/connect"));
    drop(guard);
}
