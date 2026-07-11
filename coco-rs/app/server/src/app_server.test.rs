use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use coco_types::ServerRequestUserInputParams;

use coco_error::ErrorExt;

use super::*;
use crate::AttachSurfaceOptions;
use crate::ConnectionKey;
use crate::SurfaceCapabilities;
use crate::SurfaceCapability;
use crate::SurfaceLimits;
use crate::SurfaceRole;

fn test_session_id(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid test session id")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestHandle(&'static str);

fn test_server_request() -> ServerRequest {
    ServerRequest::RequestUserInput(ServerRequestUserInputParams {
        request_id: "payload-request-id".to_string(),
        prompt: "continue?".to_string(),
        description: None,
        choices: Vec::new(),
        default: None,
    })
}

#[test]
fn new_with_surface_limits_configures_routing_limits() {
    let server = AppServer::<TestHandle>::new_with_surface_limits(
        4,
        8,
        SurfaceLimits {
            max_surfaces_per_connection: 1,
            max_passive_surfaces_per_session: 16,
        },
    );
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let (request_tx, _request_rx) = tokio::sync::mpsc::channel(8);
    let (lifecycle_tx, _lifecycle_rx) = tokio::sync::mpsc::channel(8);
    let connection = ConnectionKey::for_test(1);
    let session_id = test_session_id("sess-1");
    server.connect_with_request_and_lifecycle_senders(
        connection,
        event_tx,
        request_tx,
        lifecycle_tx,
    );
    server
        .attach_surface_with_options(
            connection,
            SurfaceId::from("surface-1"),
            session_id.clone(),
            AttachSurfaceOptions::default(),
        )
        .expect("attach first surface");

    let err = server
        .attach_surface_with_options(
            connection,
            SurfaceId::from("surface-2"),
            session_id,
            AttachSurfaceOptions::default(),
        )
        .expect_err("second surface should exceed configured connection limit");

    assert!(matches!(err, AttachError::SurfaceLimit { .. }));
}

#[tokio::test]
async fn spawn_load_owner_task_promotes_slot_without_origin_waiter() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let session_id = test_session_id("sess-1");
    let factory_runs = Arc::new(AtomicUsize::new(0));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let factory_runs_1 = Arc::clone(&factory_runs);

    let AppLoadStart::Started { completion } = server
        .spawn_load(session_id.clone(), async move {
            factory_runs_1.fetch_add(1, Ordering::SeqCst);
            release_rx.await.expect("release load");
            Ok(TestHandle("loaded"))
        })
        .expect("start load")
    else {
        panic!("expected started load");
    };
    drop(completion);

    let factory_runs_2 = Arc::clone(&factory_runs);
    let AppLoadStart::Loading(mut waiter) = server
        .spawn_load(session_id.clone(), async move {
            factory_runs_2.fetch_add(10, Ordering::SeqCst);
            Ok(TestHandle("duplicate"))
        })
        .expect("observe loading")
    else {
        panic!("expected loading");
    };

    release_tx.send(()).expect("release factory");
    let handle = waiter.wait().await.expect("load success");

    assert_eq!(handle, TestHandle("loaded"));
    assert_eq!(
        server.registry().get(&session_id),
        Some(TestHandle("loaded"))
    );
    assert_eq!(factory_runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn spawn_load_failure_removes_loading_slot() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = test_session_id("sess-1");
    let error_session_id = session_id.clone();

    let AppLoadStart::Started { mut completion } = server
        .spawn_load(session_id.clone(), async move {
            Err(crate::registry::NotFoundSnafu {
                session_id: error_session_id,
            }
            .build())
        })
        .expect("start load")
    else {
        panic!("expected started load");
    };

    let err = completion.wait().await.expect_err("load should fail");

    assert!(matches!(err, RegistryError::NotFound { .. }));
    assert_eq!(server.registry().slot_count(), 0);
    let AppLoadStart::Started { .. } = server
        .spawn_load(session_id, async { Ok(TestHandle("retry")) })
        .expect("retry after failure")
    else {
        panic!("expected retry to reserve a fresh slot");
    };
}

#[tokio::test]
async fn spawn_close_owner_task_archives_after_cascade_without_origin_waiter() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = test_session_id("sess-1");
    server
        .registry()
        .begin_load(session_id.clone())
        .expect("reserve session");
    server
        .registry()
        .complete_load_success(&session_id, TestHandle("handle"))
        .expect("session live");

    let connection = ConnectionKey::for_test(40);
    let surface_id = SurfaceId::from("surface-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let (lifecycle_tx, mut lifecycle_rx) = tokio::sync::mpsc::channel(8);
    {
        let mut routing = server.routing().write().expect("routing lock");
        routing.connect_with_lifecycle_sender(connection, tx, lifecycle_tx);
        routing
            .attach_surface(connection, surface_id.clone(), session_id.clone())
            .expect("attach surface");
    }

    let close_runs = Arc::new(AtomicUsize::new(0));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let close_runs_1 = Arc::clone(&close_runs);
    let AppCloseStart::Started { completion } = server
        .spawn_close(session_id.clone(), move |handle| async move {
            assert_eq!(handle, TestHandle("handle"));
            close_runs_1.fetch_add(1, Ordering::SeqCst);
            release_rx.await.expect("release close");
        })
        .expect("start close")
    else {
        panic!("expected started close");
    };
    drop(completion);

    let close_runs_2 = Arc::clone(&close_runs);
    let AppCloseStart::Closing(mut waiter) = server
        .spawn_close(session_id.clone(), move |_| async move {
            close_runs_2.fetch_add(10, Ordering::SeqCst);
        })
        .expect("observe closing")
    else {
        panic!("expected closing");
    };

    release_tx.send(()).expect("release close");
    let delivery = lifecycle_rx.recv().await.expect("lifecycle delivery");
    assert_eq!(delivery.surface_id, surface_id.clone());
    assert_eq!(
        delivery.kind,
        SurfaceLifecycleEffectKind::SessionEnded {
            session_id: session_id.clone()
        }
    );
    waiter.wait().await.expect("close completion");

    assert_eq!(close_runs.load(Ordering::SeqCst), 1);
    assert_eq!(server.registry().slot_count(), 0);
    let routing = server.routing().read().expect("routing lock");
    assert_eq!(routing.surface_session(&surface_id), None);
    // once the terminal `SessionEnded` effect is delivered the closed
    // surface's routing metadata is purged, not retained as SessionClosed.
    assert_eq!(
        routing.surface_attachment(&surface_id).map(|a| a.state),
        None
    );
}

#[tokio::test]
async fn spawn_close_on_loading_waits_for_load_then_archives_once() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = test_session_id("sess-1");
    let (release_load_tx, release_load_rx) = tokio::sync::oneshot::channel();
    let AppLoadStart::Started { .. } = server
        .spawn_load(session_id.clone(), async move {
            release_load_rx.await.expect("release load");
            Ok(TestHandle("loaded"))
        })
        .expect("start load")
    else {
        panic!("expected started load");
    };

    let connection = ConnectionKey::for_test(39);
    let surface_id = SurfaceId::from("surface-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let (lifecycle_tx, mut lifecycle_rx) = tokio::sync::mpsc::channel(8);
    {
        let mut routing = server.routing().write().expect("routing lock");
        routing.connect_with_lifecycle_sender(connection, tx, lifecycle_tx);
        routing
            .attach_surface(connection, surface_id.clone(), session_id.clone())
            .expect("attach surface");
    }

    let close_runs = Arc::new(AtomicUsize::new(0));
    let close_runs_1 = Arc::clone(&close_runs);
    let AppCloseStart::Loading(mut close_completion) = server
        .spawn_close(session_id.clone(), move |handle| async move {
            assert_eq!(handle, TestHandle("loaded"));
            close_runs_1.fetch_add(1, Ordering::SeqCst);
        })
        .expect("close loading")
    else {
        panic!("expected loading close");
    };

    let close_runs_2 = Arc::clone(&close_runs);
    let AppCloseStart::Loading(repeated_completion) = server
        .spawn_close(session_id.clone(), move |_| async move {
            close_runs_2.fetch_add(10, Ordering::SeqCst);
        })
        .expect("repeat close loading")
    else {
        panic!("expected repeated loading close");
    };
    assert!(!repeated_completion.is_complete());

    release_load_tx.send(()).expect("release load");
    let delivery = lifecycle_rx.recv().await.expect("lifecycle delivery");
    assert_eq!(delivery.surface_id, surface_id.clone());
    assert_eq!(
        delivery.kind,
        SurfaceLifecycleEffectKind::SessionEnded {
            session_id: session_id.clone()
        }
    );
    close_completion.wait().await.expect("close completion");

    assert_eq!(close_runs.load(Ordering::SeqCst), 1);
    assert!(repeated_completion.is_complete());
    assert_eq!(server.registry().slot_count(), 0);
    let routing = server.routing().read().expect("routing lock");
    assert_eq!(routing.surface_session(&surface_id), None);
    // closed-surface metadata is purged after the terminal effect.
    assert_eq!(
        routing.surface_attachment(&surface_id).map(|a| a.state),
        None
    );
}

#[tokio::test]
async fn spawn_shutdown_closes_live_sessions_concurrently() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let session_id_1 = test_session_id("sess-1");
    let session_id_2 = test_session_id("sess-2");
    for (session_id, handle) in [
        (session_id_1.clone(), TestHandle("handle-1")),
        (session_id_2.clone(), TestHandle("handle-2")),
    ] {
        server
            .registry()
            .begin_load(session_id.clone())
            .expect("reserve session");
        server
            .registry()
            .complete_load_success(&session_id, handle)
            .expect("session live");
    }

    let close_runs = Arc::new(AtomicUsize::new(0));
    let close_runs_for_shutdown = Arc::clone(&close_runs);
    let shutdown = server.spawn_shutdown(move |handle| {
        let close_runs = Arc::clone(&close_runs_for_shutdown);
        async move {
            match handle {
                TestHandle("handle-1") => {
                    close_runs.fetch_add(1, Ordering::SeqCst);
                }
                TestHandle("handle-2") => {
                    close_runs.fetch_add(10, Ordering::SeqCst);
                }
                other => panic!("unexpected handle: {other:?}"),
            }
        }
    });

    assert!(shutdown.errors.is_empty());
    let mut closed_session_ids = shutdown
        .sessions
        .iter()
        .map(|session| session.session_id.clone())
        .collect::<Vec<_>>();
    closed_session_ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    assert_eq!(closed_session_ids, vec![session_id_1, session_id_2]);

    for session in shutdown.sessions {
        let mut completion = session.completion;
        completion.wait().await.expect("shutdown close");
    }

    assert_eq!(close_runs.load(Ordering::SeqCst), 11);
    assert_eq!(server.registry().slot_count(), 0);
    assert_eq!(server.registry().live_count(), 0);
}

#[tokio::test]
async fn spawn_shutdown_includes_loading_and_observes_closing_sessions() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let loading_session_id = test_session_id("sess-loading");
    let closing_session_id = test_session_id("sess-closing");
    let (release_load_tx, release_load_rx) = tokio::sync::oneshot::channel();
    let AppLoadStart::Started { .. } = server
        .spawn_load(loading_session_id.clone(), async move {
            release_load_rx.await.expect("release load");
            Ok(TestHandle("loaded"))
        })
        .expect("start load")
    else {
        panic!("expected started load");
    };

    server
        .registry()
        .begin_load(closing_session_id.clone())
        .expect("reserve closing session");
    server
        .registry()
        .complete_load_success(&closing_session_id, TestHandle("closing"))
        .expect("closing session live");
    let (release_close_tx, release_close_rx) = tokio::sync::oneshot::channel();
    let AppCloseStart::Started { completion, .. } = server
        .spawn_close(closing_session_id.clone(), move |_| async move {
            release_close_rx.await.expect("release close");
        })
        .expect("start close")
    else {
        panic!("expected started close");
    };
    drop(completion);

    let shutdown_close_runs = Arc::new(AtomicUsize::new(0));
    let close_runs_for_shutdown = Arc::clone(&shutdown_close_runs);
    let shutdown = server.spawn_shutdown(move |handle| {
        let shutdown_close_runs = Arc::clone(&close_runs_for_shutdown);
        async move {
            match handle {
                TestHandle("loaded") => {
                    shutdown_close_runs.fetch_add(1, Ordering::SeqCst);
                }
                other => panic!("unexpected shutdown close handle: {other:?}"),
            }
        }
    });

    assert!(shutdown.errors.is_empty());
    let mut closed_session_ids = shutdown
        .sessions
        .iter()
        .map(|session| session.session_id.clone())
        .collect::<Vec<_>>();
    closed_session_ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    assert_eq!(
        closed_session_ids,
        vec![closing_session_id.clone(), loading_session_id.clone()]
    );

    release_load_tx.send(()).expect("release load");
    release_close_tx.send(()).expect("release close");
    for session in shutdown.sessions {
        let mut completion = session.completion;
        completion.wait().await.expect("shutdown close");
    }

    assert_eq!(shutdown_close_runs.load(Ordering::SeqCst), 1);
    assert_eq!(server.registry().slot_count(), 0);
}

#[tokio::test]
async fn spawn_replace_commits_then_closes_old_without_origin_waiter() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    server
        .registry()
        .begin_load(old_session_id.clone())
        .expect("reserve old");
    server
        .registry()
        .complete_load_success(&old_session_id, TestHandle("old"))
        .expect("old live");

    let connection = ConnectionKey::for_test(38);
    let caller = SurfaceId::from("surface-caller");
    let peer = SurfaceId::from("surface-peer");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let (lifecycle_tx, mut lifecycle_rx) = tokio::sync::mpsc::channel(8);
    {
        let mut routing = server.routing().write().expect("routing lock");
        routing.connect_with_lifecycle_sender(connection, tx, lifecycle_tx);
        routing
            .attach_surface_with_options(
                connection,
                caller.clone(),
                old_session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach caller");
        routing
            .attach_surface(connection, peer.clone(), old_session_id.clone())
            .expect("attach peer");
    }

    let (release_build_tx, release_build_rx) = tokio::sync::oneshot::channel();
    let (close_started_tx, close_started_rx) = tokio::sync::oneshot::channel();
    let (release_close_tx, release_close_rx) = tokio::sync::oneshot::channel();
    let AppReplaceStart::Started { completion } = server
        .spawn_replace(
            old_session_id.clone(),
            new_session_id.clone(),
            caller.clone(),
            async move {
                release_build_rx.await.expect("release build");
                Ok(TestHandle("new"))
            },
            move |old_handle| async move {
                assert_eq!(old_handle, TestHandle("old"));
                close_started_tx.send(()).expect("signal close started");
                release_close_rx.await.expect("release close");
            },
        )
        .expect("start replace");
    drop(completion);
    let AppLoadStart::Loading(mut new_waiter) = server
        .spawn_load(new_session_id.clone(), async {
            Ok(TestHandle("duplicate"))
        })
        .expect("observe new loading")
    else {
        panic!("expected new loading");
    };

    release_build_tx.send(()).expect("release build");
    let new_handle = new_waiter.wait().await.expect("new committed");
    let caller_delivery = lifecycle_rx.recv().await.expect("caller lifecycle");
    assert_eq!(caller_delivery.surface_id, caller.clone());
    assert_eq!(
        caller_delivery.kind,
        SurfaceLifecycleEffectKind::SessionStarted {
            session_id: new_session_id.clone()
        }
    );
    let peer_delivery = lifecycle_rx.recv().await.expect("peer lifecycle");
    assert_eq!(peer_delivery.surface_id, peer.clone());
    assert_eq!(
        peer_delivery.kind,
        SurfaceLifecycleEffectKind::SessionReplaced {
            old_session_id: old_session_id.clone(),
            new_session_id: new_session_id.clone(),
        }
    );
    close_started_rx.await.expect("close started");
    let AppLoadStart::Closing(mut old_close) = server
        .spawn_load(old_session_id.clone(), async {
            Ok(TestHandle("old-duplicate"))
        })
        .expect("observe old closing")
    else {
        panic!("expected old closing");
    };

    assert_eq!(new_handle, TestHandle("new"));
    {
        let routing = server.routing().read().expect("routing lock");
        assert_eq!(routing.surface_session(&caller), Some(&new_session_id));
        assert_eq!(routing.surface_session(&peer), None);
    }

    release_close_tx.send(()).expect("release close");
    old_close.wait().await.expect("old close complete");

    assert_eq!(
        server.registry().get(&new_session_id),
        Some(TestHandle("new"))
    );
    assert_eq!(server.registry().get(&old_session_id), None);
}

#[tokio::test]
async fn spawn_replace_detached_commits_then_closes_old_without_origin_waiter() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    server
        .registry()
        .begin_load(old_session_id.clone())
        .expect("reserve old");
    server
        .registry()
        .complete_load_success(&old_session_id, TestHandle("old"))
        .expect("old live");

    let (release_build_tx, release_build_rx) = tokio::sync::oneshot::channel();
    let (close_started_tx, close_started_rx) = tokio::sync::oneshot::channel();
    let (release_close_tx, release_close_rx) = tokio::sync::oneshot::channel();
    let AppReplaceStart::Started { completion } = server
        .spawn_replace_detached(
            old_session_id.clone(),
            new_session_id.clone(),
            async move {
                release_build_rx.await.expect("release build");
                Ok(TestHandle("new"))
            },
            move |old_handle| async move {
                assert_eq!(old_handle, TestHandle("old"));
                close_started_tx.send(()).expect("signal close started");
                release_close_rx.await.expect("release close");
            },
        )
        .expect("start detached replace");
    drop(completion);
    let AppLoadStart::Loading(mut new_waiter) = server
        .spawn_load(new_session_id.clone(), async {
            Ok(TestHandle("duplicate"))
        })
        .expect("observe new loading")
    else {
        panic!("expected new loading");
    };

    release_build_tx.send(()).expect("release build");
    let new_handle = new_waiter.wait().await.expect("new committed");
    close_started_rx.await.expect("close started");
    let AppLoadStart::Closing(mut old_close) = server
        .spawn_load(old_session_id.clone(), async {
            Ok(TestHandle("old-duplicate"))
        })
        .expect("observe old closing")
    else {
        panic!("expected old closing");
    };

    assert_eq!(new_handle, TestHandle("new"));
    release_close_tx.send(()).expect("release close");
    old_close.wait().await.expect("old close complete");

    assert_eq!(
        server.registry().get(&new_session_id),
        Some(TestHandle("new"))
    );
    assert_eq!(server.registry().get(&old_session_id), None);
}

#[tokio::test]
async fn spawn_replace_construct_failure_removes_new_and_keeps_old_live() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    server
        .registry()
        .begin_load(old_session_id.clone())
        .expect("reserve old");
    server
        .registry()
        .complete_load_success(&old_session_id, TestHandle("old"))
        .expect("old live");
    let error_session_id = new_session_id.clone();

    let AppReplaceStart::Started { mut completion } = server
        .spawn_replace(
            old_session_id.clone(),
            new_session_id.clone(),
            SurfaceId::from("surface-caller"),
            async move {
                Err(crate::registry::NotFoundSnafu {
                    session_id: error_session_id,
                }
                .build())
            },
            |_| async {},
        )
        .expect("start replace");

    let err = completion
        .wait()
        .await
        .expect_err("replace build should fail");

    assert!(matches!(err, RegistryError::NotFound { .. }));
    assert_eq!(
        server.registry().get(&old_session_id),
        Some(TestHandle("old"))
    );
    assert_eq!(server.registry().slot_count(), 1);
    let AppLoadStart::Started { .. } = server
        .spawn_load(new_session_id, async { Ok(TestHandle("retry")) })
        .expect("new slot should be reusable")
    else {
        panic!("expected fresh new load");
    };
}

#[test]
fn commit_replace_updates_registry_and_routing_in_one_section() {
    let server = AppServer::new(1, 8);
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    server
        .registry()
        .begin_load(old_session_id.clone())
        .expect("reserve old");
    server
        .registry()
        .complete_load_success(&old_session_id, TestHandle("old"))
        .expect("old live");
    server
        .registry()
        .begin_replace(&old_session_id, new_session_id.clone())
        .expect("reserve replacement");

    let connection = ConnectionKey::for_test(41);
    let caller = SurfaceId::from("surface-caller");
    let peer = SurfaceId::from("surface-peer");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let (lifecycle_tx, mut lifecycle_rx) = tokio::sync::mpsc::channel(8);
    {
        let mut routing = server.routing().write().expect("routing lock");
        routing.connect_with_lifecycle_sender(connection, tx, lifecycle_tx);
        routing
            .attach_surface_with_options(
                connection,
                caller.clone(),
                old_session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach caller");
        routing
            .attach_surface(connection, peer.clone(), old_session_id.clone())
            .expect("attach peer");
    }

    let commit = server
        .commit_replace_for_surface(&old_session_id, &new_session_id, TestHandle("new"), &caller)
        .expect("commit replace");

    assert_eq!(commit.old_handle, TestHandle("old"));
    assert!(!commit.old_close_completion.is_complete());
    assert_eq!(commit.routing_outcome.old_session_id, old_session_id);
    assert_eq!(commit.routing_outcome.new_session_id, new_session_id);
    assert_eq!(commit.routing_outcome.calling_surface, caller);
    assert_eq!(commit.routing_outcome.detached_surfaces, vec![peer.clone()]);
    assert_eq!(
        commit.lifecycle_effects,
        vec![
            SurfaceLifecycleEffect {
                surface_id: caller.clone(),
                kind: SurfaceLifecycleEffectKind::SessionStarted {
                    session_id: new_session_id.clone(),
                },
            },
            SurfaceLifecycleEffect {
                surface_id: peer.clone(),
                kind: SurfaceLifecycleEffectKind::SessionReplaced {
                    old_session_id: old_session_id.clone(),
                    new_session_id: new_session_id.clone(),
                },
            },
        ]
    );
    assert_eq!(
        server.registry().get(&test_session_id("sess-new")),
        Some(TestHandle("new"))
    );
    assert_eq!(server.registry().get(&test_session_id("sess-old")), None);
    let routing = server.routing().read().expect("routing lock");
    assert_eq!(
        routing.surface_session(&SurfaceId::from("surface-caller")),
        Some(&test_session_id("sess-new"))
    );
    assert_eq!(routing.surface_session(&peer), None);
    drop(routing);

    let route_outcome = server
        .routing()
        .write()
        .expect("routing lock")
        .route_lifecycle_effects(commit.lifecycle_effects);
    assert_eq!(route_outcome.delivered, 2);
    assert!(route_outcome.disconnected.is_empty());
    let caller_delivery = lifecycle_rx.try_recv().expect("caller lifecycle");
    assert_eq!(caller_delivery.surface_id, caller);
    assert_eq!(
        caller_delivery.kind,
        SurfaceLifecycleEffectKind::SessionStarted {
            session_id: new_session_id.clone(),
        }
    );
    let peer_delivery = lifecycle_rx.try_recv().expect("peer lifecycle");
    assert_eq!(peer_delivery.surface_id, peer);
    assert_eq!(
        peer_delivery.kind,
        SurfaceLifecycleEffectKind::SessionReplaced {
            old_session_id,
            new_session_id,
        }
    );
    assert!(lifecycle_rx.try_recv().is_err());
}

#[test]
fn commit_replace_rejects_missing_calling_surface_before_registry_mutation() {
    let server = AppServer::new(1, 8);
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    server
        .registry()
        .begin_load(old_session_id.clone())
        .expect("reserve old");
    server
        .registry()
        .complete_load_success(&old_session_id, TestHandle("old"))
        .expect("old live");
    server
        .registry()
        .begin_replace(&old_session_id, new_session_id.clone())
        .expect("reserve replacement");

    let err = server
        .commit_replace_for_surface(
            &old_session_id,
            &new_session_id,
            TestHandle("new"),
            &SurfaceId::from("surface-missing"),
        )
        .expect_err("missing surface should fail");

    assert!(matches!(
        err,
        AppServerError::CallingSurfaceNotAttached { .. }
    ));
    assert_eq!(err.status_code(), StatusCode::InvalidArguments);
    assert_eq!(
        server.registry().get(&old_session_id),
        Some(TestHandle("old"))
    );
    assert_eq!(server.registry().get(&new_session_id), None);
    let load_state = server
        .registry()
        .begin_load(new_session_id)
        .expect("new remains loading");
    assert!(matches!(load_state, crate::LoadStart::Loading(_)));
}

#[test]
fn commit_replace_rejects_calling_surface_on_wrong_session() {
    let server = AppServer::new(2, 8);
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    let other_session_id = test_session_id("sess-other");
    server
        .registry()
        .begin_load(old_session_id.clone())
        .expect("reserve old");
    server
        .registry()
        .complete_load_success(&old_session_id, TestHandle("old"))
        .expect("old live");
    server
        .registry()
        .begin_replace(&old_session_id, new_session_id.clone())
        .expect("reserve replacement");

    let connection = ConnectionKey::for_test(42);
    let caller = SurfaceId::from("surface-caller");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    {
        let mut routing = server.routing().write().expect("routing lock");
        routing.connect(connection, tx);
        routing
            .attach_surface(connection, caller.clone(), other_session_id)
            .expect("attach wrong session");
    }

    let err = server
        .commit_replace_for_surface(&old_session_id, &new_session_id, TestHandle("new"), &caller)
        .expect_err("wrong session should fail");

    assert!(matches!(
        err,
        AppServerError::CallingSurfaceWrongSession { .. }
    ));
    assert_eq!(
        server.registry().get(&old_session_id),
        Some(TestHandle("old"))
    );
    assert_eq!(server.registry().get(&new_session_id), None);
}

#[test]
fn complete_close_archives_surfaces_and_removes_registry_slot() {
    let server = AppServer::new(1, 8);
    let session_id = test_session_id("sess-1");
    server
        .registry()
        .begin_load(session_id.clone())
        .expect("reserve session");
    server
        .registry()
        .complete_load_success(&session_id, TestHandle("handle"))
        .expect("session live");
    let crate::CloseStart::Started { handle, completion } = server
        .registry()
        .begin_close(&session_id)
        .expect("begin close")
    else {
        panic!("expected close start");
    };
    assert_eq!(handle, TestHandle("handle"));
    assert!(!completion.is_complete());

    let connection = ConnectionKey::for_test(43);
    let interactive = SurfaceId::from("surface-interactive");
    let passive = SurfaceId::from("surface-passive");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let (lifecycle_tx, mut lifecycle_rx) = tokio::sync::mpsc::channel(8);
    {
        let mut routing = server.routing().write().expect("routing lock");
        routing.connect_with_lifecycle_sender(connection, tx, lifecycle_tx);
        routing
            .attach_surface_with_options(
                connection,
                interactive.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");
        routing
            .attach_surface(connection, passive.clone(), session_id.clone())
            .expect("attach passive");
    }

    let commit = server
        .complete_close_and_archive_surfaces(&session_id)
        .expect("complete close");

    assert!(completion.is_complete());
    assert_eq!(server.registry().slot_count(), 0);
    assert_eq!(commit.routing_outcome.closed_surfaces.len(), 2);
    let mut effects = commit.lifecycle_effects.clone();
    effects.sort_by(|left, right| {
        left.surface_id
            .to_string()
            .cmp(&right.surface_id.to_string())
    });
    assert_eq!(
        effects,
        vec![
            SurfaceLifecycleEffect {
                surface_id: interactive.clone(),
                kind: SurfaceLifecycleEffectKind::SessionEnded {
                    session_id: session_id.clone(),
                },
            },
            SurfaceLifecycleEffect {
                surface_id: passive.clone(),
                kind: SurfaceLifecycleEffectKind::SessionEnded {
                    session_id: session_id.clone(),
                },
            },
        ]
    );
    let routing = server.routing().read().expect("routing lock");
    assert_eq!(routing.surface_session(&interactive), None);
    assert_eq!(routing.surface_session(&passive), None);
    assert_eq!(
        routing.surface_attachment(&interactive).map(|a| a.state),
        Some(crate::SurfaceState::SessionClosed)
    );
    assert_eq!(
        routing.surface_attachment(&passive).map(|a| a.state),
        Some(crate::SurfaceState::SessionClosed)
    );
    drop(routing);

    let route_outcome = server
        .routing()
        .write()
        .expect("routing lock")
        .route_lifecycle_effects(commit.lifecycle_effects);
    assert_eq!(route_outcome.delivered, 2);
    assert!(route_outcome.disconnected.is_empty());
    let mut delivered = [
        lifecycle_rx.try_recv().expect("first lifecycle"),
        lifecycle_rx.try_recv().expect("second lifecycle"),
    ];
    delivered.sort_by(|left, right| {
        left.surface_id
            .to_string()
            .cmp(&right.surface_id.to_string())
    });
    assert_eq!(delivered[0].surface_id, interactive);
    assert_eq!(
        delivered[0].kind,
        SurfaceLifecycleEffectKind::SessionEnded {
            session_id: session_id.clone(),
        }
    );
    assert_eq!(delivered[1].surface_id, passive);
    assert_eq!(
        delivered[1].kind,
        SurfaceLifecycleEffectKind::SessionEnded { session_id }
    );
    assert!(lifecycle_rx.try_recv().is_err());
}

#[test]
fn complete_close_rejects_non_closing_session_before_routing_mutation() {
    let server = AppServer::new(1, 8);
    let session_id = test_session_id("sess-1");
    server
        .registry()
        .begin_load(session_id.clone())
        .expect("reserve session");
    server
        .registry()
        .complete_load_success(&session_id, TestHandle("handle"))
        .expect("session live");

    let connection = ConnectionKey::for_test(44);
    let surface_id = SurfaceId::from("surface-1");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    {
        let mut routing = server.routing().write().expect("routing lock");
        routing.connect(connection, tx);
        routing
            .attach_surface(connection, surface_id.clone(), session_id.clone())
            .expect("attach");
    }

    let err = server
        .complete_close_and_archive_surfaces(&session_id)
        .expect_err("session is not closing");

    assert!(matches!(err, AppServerError::Registry { .. }));
    assert_eq!(
        server.registry().get(&session_id),
        Some(TestHandle("handle"))
    );
    let routing = server.routing().read().expect("routing lock");
    assert_eq!(routing.surface_session(&surface_id), Some(&session_id));
    assert_eq!(
        routing.surface_attachment(&surface_id).map(|a| a.state),
        Some(crate::SurfaceState::Attached)
    );
}

#[test]
fn list_live_sessions_reports_surface_counts_and_orphans() {
    let server = AppServer::new(1, 8);
    let session_id = test_session_id("sess-1");
    server
        .registry()
        .begin_load(session_id.clone())
        .expect("reserve session");
    server
        .registry()
        .complete_load_success(&session_id, TestHandle("handle"))
        .expect("session live");

    let connection = ConnectionKey::for_test(48);
    let surface_1 = SurfaceId::from("surface-1");
    let surface_2 = SurfaceId::from("surface-2");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let (request_tx, _request_rx) = tokio::sync::mpsc::channel(8);
    let (lifecycle_tx, _lifecycle_rx) = tokio::sync::mpsc::channel(8);
    server.connect_with_request_and_lifecycle_senders(
        connection,
        event_tx,
        request_tx,
        lifecycle_tx,
    );
    server
        .attach_surface_with_options(
            connection,
            surface_1,
            session_id.clone(),
            AttachSurfaceOptions::default(),
        )
        .expect("attach first surface");
    server
        .attach_surface_with_options(
            connection,
            surface_2,
            session_id.clone(),
            AttachSurfaceOptions::default(),
        )
        .expect("attach second surface");

    let summaries = server.list_live_sessions();

    assert_eq!(
        summaries,
        vec![AppLiveSessionSummary {
            session_id: session_id.clone(),
            surface_counts: SessionSurfaceCounts {
                attached: 2,
                closed: 0,
            },
        }]
    );

    let disconnect = server.disconnect(connection);

    assert_eq!(disconnect.detached_surfaces.len(), 2);
    assert_eq!(
        server.list_live_sessions(),
        vec![AppLiveSessionSummary {
            session_id,
            surface_counts: SessionSurfaceCounts::default(),
        }]
    );
}

#[test]
fn resolve_server_request_completes_pending_reply() {
    let server = AppServer::<TestHandle>::new(1, 8);
    let session_id = test_session_id("sess-1");
    let connection = ConnectionKey::for_test(45);
    let surface_id = SurfaceId::from("surface-1");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let (request_tx, mut request_rx) = tokio::sync::mpsc::channel(8);
    {
        let mut routing = server.routing().write().expect("routing lock");
        routing.connect_with_request_sender(connection, event_tx, request_tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");
    }
    let routed = server
        .route_server_request(
            session_id.clone(),
            SurfaceCapability::Notifications,
            None,
            test_server_request(),
        )
        .expect("route request");
    let delivery = request_rx.try_recv().expect("request delivery");
    let reply = ServerRequestReply::UserInput(UserInputResolveParams {
        request_id: delivery.request_id.as_display(),
        answer: "yes".to_string(),
    });

    let resolved = server
        .resolve_server_request(&session_id, reply)
        .expect("resolve request");

    assert_eq!(resolved.pending, routed.pending);
    assert!(matches!(resolved.reply, ServerRequestReply::UserInput(_)));
    let routing = server.routing().read().expect("routing lock");
    assert!(
        routing
            .pending_server_requests_for_surface(&surface_id)
            .is_empty()
    );
}

#[test]
fn resolve_server_request_rejects_wrong_session_and_keeps_pending() {
    let server = AppServer::<TestHandle>::new(1, 8);
    let session_id = test_session_id("sess-1");
    let wrong_session_id = test_session_id("sess-wrong");
    let connection = ConnectionKey::for_test(46);
    let surface_id = SurfaceId::from("surface-1");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let (request_tx, mut request_rx) = tokio::sync::mpsc::channel(8);
    {
        let mut routing = server.routing().write().expect("routing lock");
        routing.connect_with_request_sender(connection, event_tx, request_tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        keychain: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");
    }
    let routed = server
        .route_server_request(
            session_id,
            SurfaceCapability::Keychain,
            None,
            test_server_request(),
        )
        .expect("route request");
    let delivery = request_rx.try_recv().expect("request delivery");
    let reply = ServerRequestReply::UserInput(UserInputResolveParams {
        request_id: delivery.request_id.as_display(),
        answer: "yes".to_string(),
    });

    let err = server
        .resolve_server_request(&wrong_session_id, reply)
        .expect_err("wrong session should fail");

    assert!(matches!(
        err,
        AppServerError::ServerRequestWrongSession { .. }
    ));
    let routing = server.routing().read().expect("routing lock");
    assert_eq!(
        routing.pending_server_requests_for_surface(&surface_id),
        vec![routed.pending]
    );
}

#[test]
fn app_server_exposes_pending_request_replays_for_adapter_reconnect() {
    let server = AppServer::<TestHandle>::new(1, 8);
    let session_id = test_session_id("sess-1");
    let connection = ConnectionKey::for_test(47);
    let surface_id = SurfaceId::from("surface-1");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let (request_tx, _request_rx) = tokio::sync::mpsc::channel(8);
    {
        let mut routing = server.routing().write().expect("routing lock");
        routing.connect_with_request_sender(connection, event_tx, request_tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");
    }

    let routed = server
        .route_server_request(
            session_id,
            SurfaceCapability::Notifications,
            Some(TurnId::from("turn-1")),
            test_server_request(),
        )
        .expect("route request");

    let replays = server.pending_server_request_replays_for_surface(&surface_id);

    assert_eq!(replays.len(), 1);
    assert_eq!(replays[0].pending, routed.pending);
    let ServerRequest::RequestUserInput(params) = &replays[0].request else {
        panic!("expected user input replay");
    };
    assert_eq!(params.request_id, "payload-request-id");
}
