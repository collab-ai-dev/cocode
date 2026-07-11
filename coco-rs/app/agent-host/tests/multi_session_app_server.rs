use std::sync::Arc;

use coco_agent_host::{
    local_client::LocalServerClient,
    sdk_server::{AppServerSdkHandler, LocalAppSessionHandle, SdkServerState},
};
use coco_app_server::{
    AppServer, AttachSurfaceOptions, LocalClientAdapter, LocalClientSubscribeOutcome,
};
use coco_app_server_client::ClientError;
use coco_types::{
    ArchiveTarget, CoreEvent, InteractiveTarget, ServerNotification, SessionArchiveParams,
    SessionEnvelope, SessionStartParams, SessionState,
};
use tokio::sync::mpsc;

struct Fixture {
    server: Arc<AppServer<LocalAppSessionHandle>>,
    adapter: LocalClientAdapter<LocalAppSessionHandle>,
    handler: AppServerSdkHandler,
}

fn fixture() -> Fixture {
    let state = Arc::new(SdkServerState::default());
    state.install_startup_cwd(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let server = Arc::new(AppServer::new(8, 16));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 16);
    let (notif_tx, _notif_rx) = mpsc::channel(16);
    let handler = AppServerSdkHandler::with_local_app_server(state, notif_tx, Arc::clone(&server));
    Fixture {
        server,
        adapter,
        handler,
    }
}

#[tokio::test]
async fn one_connection_holds_two_independent_interactive_authorities() {
    let fixture = fixture();
    let client = LocalServerClient::connect_local(&fixture.adapter);
    let first = client
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start A");
    let second = client
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start B");

    assert_ne!(first.session_id, second.session_id);
    assert_ne!(first.surface_id, second.surface_id);
    let live = fixture.server.list_live_sessions();
    assert_eq!(live.len(), 2);
    assert!(
        live.iter()
            .all(|summary| summary.surface_counts.attached == 1)
    );
}

#[tokio::test]
async fn cross_connection_surface_authority_is_rejected_without_mutation() {
    let fixture = fixture();
    let owner = LocalServerClient::connect_local(&fixture.adapter);
    let attacker = LocalServerClient::connect_local(&fixture.adapter);
    let started = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start owner session");
    let target = InteractiveTarget {
        session_id: started.session_id.clone(),
        surface_id: started.surface_id.expect("interactive surface"),
    };

    let error = attacker
        .turn_interrupt(&fixture.handler, target)
        .await
        .expect_err("foreign connection cannot use surface");
    let ClientError::Server { data, .. } = error else {
        panic!("expected typed server error");
    };
    assert_eq!(
        data.and_then(|value| value.get("kind").cloned()),
        Some(serde_json::json!("surface_wrong_connection"))
    );
    assert_eq!(fixture.server.list_live_sessions().len(), 1);
}

#[tokio::test]
async fn mismatched_session_surface_pair_is_rejected_with_stable_kind() {
    let fixture = fixture();
    let owner = LocalServerClient::connect_local(&fixture.adapter);
    let first = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start A");
    let second = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start B");

    let error = owner
        .turn_interrupt(
            &fixture.handler,
            InteractiveTarget {
                session_id: second.session_id,
                surface_id: first.surface_id.expect("A surface"),
            },
        )
        .await
        .expect_err("surface and session must be correlated");
    let ClientError::Server { data, .. } = error else {
        panic!("expected typed server error");
    };
    assert_eq!(
        data.and_then(|value| value.get("kind").cloned()),
        Some(serde_json::json!("surface_wrong_session"))
    );
}

#[tokio::test]
async fn passive_surface_cannot_issue_interactive_mutation() {
    let fixture = fixture();
    let owner = LocalServerClient::connect_local(&fixture.adapter);
    let started = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start session");
    let observer = LocalServerClient::connect_local(&fixture.adapter);
    let passive = observer
        .attach_passive_session(started.session_id.clone())
        .expect("attach passive surface");

    let error = observer
        .turn_interrupt(
            &fixture.handler,
            InteractiveTarget {
                session_id: passive.session_id().clone(),
                surface_id: passive.surface_id().clone(),
            },
        )
        .await
        .expect_err("passive surface has no mutation authority");
    let ClientError::Server { data, .. } = error else {
        panic!("expected typed server error");
    };
    assert_eq!(
        data.and_then(|value| value.get("kind").cloned()),
        Some(serde_json::json!("surface_not_interactive"))
    );
}

#[tokio::test]
async fn disconnected_session_is_archived_through_explicit_orphan_authority() {
    let fixture = fixture();
    let owner = LocalServerClient::connect_local(&fixture.adapter);
    let started = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start orphan candidate");
    owner.disconnect();

    let archiver = LocalServerClient::connect_local(&fixture.adapter);
    archiver
        .session_archive(
            &fixture.handler,
            SessionArchiveParams {
                target: ArchiveTarget::Orphaned(coco_types::SessionTarget {
                    session_id: started.session_id.clone(),
                }),
            },
        )
        .await
        .expect("archive orphan");
    assert!(fixture.server.registry().get(&started.session_id).is_none());
}

#[tokio::test]
async fn session_events_and_replay_never_cross_session_identity() {
    let fixture = fixture();
    let client = LocalServerClient::connect_local(&fixture.adapter);
    let first = client
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start A");
    let second = client
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start B");

    for (session_id, seq) in [(first.session_id, 1), (second.session_id, 1)] {
        fixture.server.route_envelope(SessionEnvelope::durable(
            session_id.clone(),
            None,
            None,
            seq,
            CoreEvent::Protocol(ServerNotification::SessionStateChanged {
                state: SessionState::Running,
            }),
        ));
        let observer = fixture.adapter.connect();
        let LocalClientSubscribeOutcome::Attached(subscription) = observer
            .subscribe_surface(session_id.clone(), Some(0), AttachSurfaceOptions::default())
            .expect("subscribe with replay")
        else {
            panic!("expected retained replay");
        };
        let replay = subscription.replayed;
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].session_id, session_id);
    }
}
