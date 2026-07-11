use std::sync::{Arc, Mutex};

use coco_app_server::{
    AppServer, AttachSurfaceOptions, ConnectionKey, LocalClientAdapter, LocalClientDispatchError,
    LocalClientRequestContext, LocalClientRequestFuture, LocalClientRequestHandler,
    SessionSurfaceCounts, SurfaceCapabilities, SurfaceCapability,
};
use coco_types::{
    CoreEvent, ServerNotification, ServerRequest, ServerRequestUserInputParams, SessionEnvelope,
    SessionState, SurfaceLifecycleEffectKind, TurnId,
};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestHandle(&'static str);

fn test_session_id(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid test session id")
}

fn durable_envelope(session_id: SessionId, seq: i64) -> SessionEnvelope {
    SessionEnvelope::durable(
        session_id,
        None,
        None,
        seq,
        CoreEvent::Protocol(ServerNotification::SessionStateChanged {
            state: SessionState::Running,
        }),
    )
}

fn test_server_request(label: &str) -> ServerRequest {
    ServerRequest::RequestUserInput(ServerRequestUserInputParams {
        request_id: format!("payload-request-{label}"),
        prompt: "continue?".to_string(),
        description: None,
        choices: Vec::new(),
        default: None,
    })
}

struct RecordingLocalRequestHandler {
    calls: Arc<Mutex<Vec<(ConnectionKey, String)>>>,
    result: serde_json::Value,
    error: Option<LocalClientDispatchError>,
}

impl Default for RecordingLocalRequestHandler {
    fn default() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: serde_json::Value::Null,
            error: None,
        }
    }
}

impl LocalClientRequestHandler for RecordingLocalRequestHandler {
    fn handle_local_client_request(
        &self,
        context: LocalClientRequestContext,
        request: ClientRequest,
    ) -> LocalClientRequestFuture {
        let calls = Arc::clone(&self.calls);
        let result = self.result.clone();
        let error = self.error.clone();
        Box::pin(async move {
            calls.lock().expect("calls lock").push((
                context.connection_key(),
                request.method().as_str().to_string(),
            ));
            match error {
                Some(error) => Err(error),
                None => Ok(result),
            }
        })
    }
}

struct RecordingClientRequestHandler {
    calls: Arc<Mutex<Vec<ClientRequest>>>,
    result: serde_json::Value,
    error: Option<LocalClientDispatchError>,
}

impl RecordingClientRequestHandler {
    fn ok(result: serde_json::Value) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            result,
            error: None,
        }
    }

    fn error(error: LocalClientDispatchError) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: serde_json::Value::Null,
            error: Some(error),
        }
    }
}

impl LocalClientRequestHandler for RecordingClientRequestHandler {
    fn handle_local_client_request(
        &self,
        _context: LocalClientRequestContext,
        request: ClientRequest,
    ) -> LocalClientRequestFuture {
        let calls = Arc::clone(&self.calls);
        let result = self.result.clone();
        let error = self.error.clone();
        Box::pin(async move {
            calls.lock().expect("calls lock").push(request);
            match error {
                Some(error) => Err(error),
                None => Ok(result),
            }
        })
    }
}

fn minimal_turn_params(prompt: &str) -> TurnStartParams {
    TurnStartParams {
        target: coco_types::InteractiveTarget {
            session_id: test_session_id("placeholder-session"),
            surface_id: SurfaceId::from("placeholder-surface"),
        },
        prompt: prompt.to_string(),
        history_override: Vec::new(),
        images: Vec::new(),
        slash_metadata: None,
        model_selection: None,
        permission_mode: None,
        thinking_level: None,
    }
}

#[tokio::test]
async fn local_server_client_typed_methods_dispatch_and_decode_results() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let client = LocalServerClient::connect_local(&adapter);
    let session_id = test_session_id("sess-local-typed-client");
    let handler = RecordingLocalRequestHandler {
        result: serde_json::json!({ "session_id": session_id }),
        ..RecordingLocalRequestHandler::default()
    };

    let result = client
        .session_start(&handler, SessionStartParams::default())
        .await
        .expect("session start succeeds");

    assert_eq!(
        result.session_id,
        test_session_id("sess-local-typed-client")
    );
    {
        let calls = handler.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "session/start");
    }

    let unit_handler = RecordingLocalRequestHandler::default();
    let typed_session = LocalSessionClient {
        session_id: session_id.clone(),
        surface_id: SurfaceId::from("surface-local-typed-client"),
    };
    client
        .user_input_resolve(
            &unit_handler,
            UserInputResolveParams {
                target: typed_session.interactive_target(),
                request_id: "input-1".to_string(),
                answer: "yes".to_string(),
            },
        )
        .await
        .expect("user input resolve succeeds");
    {
        let calls = unit_handler.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "input/resolveUserInput");
    }

    let usage = coco_types::SessionUsageSnapshot::empty(test_session_id("sess-local-typed-client"));
    let cost_handler = RecordingLocalRequestHandler {
        result: serde_json::to_value(SessionCostResult {
            text: "No usage yet.".to_string(),
            usage,
        })
        .expect("cost result serializes"),
        ..RecordingLocalRequestHandler::default()
    };
    let cost = client
        .session_cost(&cost_handler, typed_session.session_target())
        .await
        .expect("session cost succeeds");
    assert_eq!(cost.text, "No usage yet.");
    {
        let calls = cost_handler.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "session/cost");
    }

    let task_list_handler = RecordingLocalRequestHandler {
        result: serde_json::to_value(TaskListResult { tasks: Vec::new() })
            .expect("task list result serializes"),
        ..RecordingLocalRequestHandler::default()
    };
    let task_list = client
        .task_list(&task_list_handler, &typed_session)
        .await
        .expect("task list succeeds");
    assert!(task_list.tasks.is_empty());
    {
        let calls = task_list_handler.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "task/list");
    }

    let background_handler = RecordingLocalRequestHandler {
        result: serde_json::to_value(BackgroundAllTasksResult {
            task_ids: vec!["task-1".to_string()],
        })
        .expect("background-all result serializes"),
        ..RecordingLocalRequestHandler::default()
    };
    let backgrounded = client
        .background_all_tasks(&background_handler, &typed_session)
        .await
        .expect("background-all succeeds");
    assert_eq!(backgrounded.task_ids, vec!["task-1".to_string()]);
    let calls = background_handler.calls.lock().expect("calls lock");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, "control/backgroundAllTasks");
}

#[tokio::test]
async fn local_session_handle_helpers_dispatch_session_requests() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = test_session_id("sess-local-handle");
    server
        .registry()
        .begin_load(session_id.clone())
        .expect("reserve session");
    server
        .registry()
        .complete_load_success(&session_id, TestHandle("handle"))
        .expect("session live");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let client = LocalServerClient::connect_local(&adapter);
    let interactive = client
        .attach_interactive_session(session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach interactive");
    let passive = client
        .subscribe_session(session_id.clone(), Some(0), AttachSurfaceOptions::default())
        .expect("subscribe passive");

    let query_handler = RecordingClientRequestHandler::ok(serde_json::json!({
        "turn_id": "turn-local-handle"
    }));
    let query = client
        .query_session(&query_handler, &interactive, minimal_turn_params("hello"))
        .await
        .expect("query succeeds");
    assert_eq!(query.turn_id, TurnId::from("turn-local-handle"));
    {
        let calls = query_handler.calls.lock().expect("calls lock");
        let ClientRequest::TurnStart(params) = &calls[0] else {
            panic!("expected turn/start request");
        };
        assert_eq!(params.prompt, "hello");
    }

    let interrupt_handler = RecordingClientRequestHandler::ok(serde_json::Value::Null);
    client
        .interrupt_session(&interrupt_handler, &interactive)
        .await
        .expect("interrupt succeeds");
    assert!(matches!(
        &interrupt_handler.calls.lock().expect("calls lock")[0],
        ClientRequest::TurnInterrupt(_)
    ));

    let read_handler = RecordingClientRequestHandler::ok(serde_json::json!({
        "session": {
            "session_id": session_id,
            "model": "gpt-test",
            "cwd": "/tmp",
            "created_at": "2026-07-08T00:00:00Z",
            "message_count": 0,
            "total_tokens": 0
        },
        "messages": [],
        "has_more": false
    }));
    let read = client
        .read_passive_session(&read_handler, &passive, Some("4".to_string()), Some(2))
        .await
        .expect("read succeeds");
    assert_eq!(
        read.session.session_id,
        test_session_id("sess-local-handle")
    );
    {
        let calls = read_handler.calls.lock().expect("calls lock");
        let ClientRequest::SessionRead(params) = &calls[0] else {
            panic!("expected session/read request");
        };
        assert_eq!(
            params.target.session_id,
            test_session_id("sess-local-handle")
        );
        assert_eq!(params.cursor.as_deref(), Some("4"));
        assert_eq!(params.limit, Some(2));
    }

    let turns_handler = RecordingClientRequestHandler::ok(serde_json::json!({
        "session": {
            "session_id": session_id,
            "model": "gpt-test",
            "cwd": "/tmp",
            "created_at": "2026-07-08T00:00:00Z",
            "message_count": 2,
            "total_tokens": 0
        },
        "turns": [{
            "index": 1,
            "start_cursor": "2",
            "message_count": 2
        }],
        "has_more": false
    }));
    let turns = client
        .list_passive_session_turns(&turns_handler, &passive, Some("1".to_string()), Some(1))
        .await
        .expect("turn list succeeds");
    assert_eq!(turns.turns[0].start_cursor, "2");
    {
        let calls = turns_handler.calls.lock().expect("calls lock");
        let ClientRequest::SessionTurnsList(params) = &calls[0] else {
            panic!("expected session/turns/list request");
        };
        assert_eq!(
            params.target.session_id,
            test_session_id("sess-local-handle")
        );
        assert_eq!(params.cursor.as_deref(), Some("1"));
        assert_eq!(params.limit, Some(1));
    }
}

#[tokio::test]
async fn local_close_session_returns_handle_on_failure() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = test_session_id("sess-close-failure");
    server
        .registry()
        .begin_load(session_id.clone())
        .expect("reserve session");
    server
        .registry()
        .complete_load_success(&session_id, TestHandle("handle"))
        .expect("session live");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut client = LocalServerClient::connect_local(&adapter);
    let interactive = client
        .attach_interactive_session(session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach interactive");
    let handler = RecordingClientRequestHandler::error(LocalClientDispatchError::invalid_params(
        "archive failed",
    ));

    let Err((returned, ClientError::Server { message, .. })) =
        client.close_session(&handler, interactive).await
    else {
        panic!("expected close failure");
    };

    assert_eq!(returned.session_id(), &session_id);
    assert_eq!(message, "archive failed");
    let calls = handler.calls.lock().expect("calls lock");
    let ClientRequest::SessionArchive(params) = &calls[0] else {
        panic!("expected session/archive request");
    };
    assert_eq!(params.target.session_id(), &session_id);
}

#[tokio::test]
async fn local_replace_session_helpers_return_new_handle_or_original_on_failure() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let old_session_id = test_session_id("sess-replace-old");
    server
        .registry()
        .begin_load(old_session_id.clone())
        .expect("reserve old session");
    server
        .registry()
        .complete_load_success(&old_session_id, TestHandle("handle"))
        .expect("old session live");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let client = LocalServerClient::connect_local(&adapter);
    let interactive = client
        .attach_interactive_session(old_session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach interactive");

    let start_handler = RecordingClientRequestHandler::ok(serde_json::json!({
        "session_id": "sess-replace-started",
        "surface_id": "surface-replace-started"
    }));
    let started = client
        .replace_session_with_start(&start_handler, interactive, SessionStartParams::default())
        .await
        .expect("replace with start succeeds");
    assert_eq!(
        started.session_id(),
        &test_session_id("sess-replace-started")
    );
    assert_eq!(
        started.surface_id(),
        &SurfaceId::from("surface-replace-started")
    );
    {
        let calls = start_handler.calls.lock().expect("calls lock");
        assert!(matches!(&calls[0], ClientRequest::SessionReplace(_)));
    }

    let resume_handler = RecordingClientRequestHandler::ok(serde_json::json!({
        "session_id": "sess-replace-resumed",
        "surface_id": "surface-replace-started"
    }));
    let resumed = client
        .replace_session_with_resume(
            &resume_handler,
            started,
            SessionResumeParams {
                target: coco_types::SessionTarget {
                    session_id: test_session_id("sess-replace-resumed"),
                },
            },
        )
        .await
        .expect("replace with resume succeeds");
    assert_eq!(
        resumed.session_id(),
        &test_session_id("sess-replace-resumed")
    );
    assert_eq!(
        resumed.surface_id(),
        &SurfaceId::from("surface-replace-started")
    );

    let failing_handler = RecordingClientRequestHandler::error(
        LocalClientDispatchError::invalid_params("resume failed"),
    );
    let Err((returned, ClientError::Server { message, .. })) = client
        .replace_session_with_resume(
            &failing_handler,
            resumed,
            SessionResumeParams {
                target: coco_types::SessionTarget {
                    session_id: test_session_id("sess-replace-fail"),
                },
            },
        )
        .await
    else {
        panic!("expected replace failure");
    };
    assert_eq!(
        returned.session_id(),
        &test_session_id("sess-replace-resumed")
    );
    assert_eq!(
        returned.surface_id(),
        &SurfaceId::from("surface-replace-started")
    );
    assert_eq!(message, "resume failed");
}

#[tokio::test]
async fn local_server_client_maps_dispatch_errors_to_server_errors() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let client = LocalServerClient::connect_local(&adapter);
    let handler = RecordingLocalRequestHandler {
        error: Some(LocalClientDispatchError::invalid_params(
            "bad local request",
        )),
        ..RecordingLocalRequestHandler::default()
    };

    let Err(ClientError::Server { message, .. }) = client.keep_alive(&handler).await else {
        panic!("expected server error");
    };

    assert_eq!(message, "bad local request");
}

#[test]
fn local_attach_passive_session_attaches_without_replay() {
    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
    let session_id = test_session_id("sess-passive-live");
    server
        .registry()
        .begin_load(session_id.clone())
        .expect("reserve session");
    server
        .registry()
        .complete_load_success(&session_id, TestHandle("handle"))
        .expect("session live");
    // A durable event exists in the ring; a no-replay attach must NOT return it.
    server.route_envelope(durable_envelope(session_id.clone(), 1));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let client = LocalServerClient::connect_local(&adapter);

    let passive = client
        .attach_passive_session(session_id.clone())
        .expect("attach passive live-only");

    assert_eq!(passive.session_id(), &session_id);
    assert!(
        passive.replayed().is_empty(),
        "live-only attach has no replay"
    );
    assert_eq!(
        server.list_live_sessions()[0].surface_counts,
        SessionSurfaceCounts {
            attached: 1,
            closed: 0,
        }
    );
}

#[test]
fn local_server_client_attaches_interactive_and_passive_surfaces() {
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
    server.route_envelope(durable_envelope(session_id.clone(), 1));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut client = LocalServerClient::connect_local(&adapter);

    let interactive = client
        .attach_interactive_session(session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach interactive");
    let passive = client
        .subscribe_session(session_id.clone(), Some(0), AttachSurfaceOptions::default())
        .expect("subscribe passive");

    assert_eq!(interactive.session_id(), &session_id);
    assert_eq!(passive.session_id(), &session_id);
    assert_eq!(passive.replayed().len(), 1);
    assert_eq!(
        server.list_live_sessions()[0].surface_counts,
        SessionSurfaceCounts {
            attached: 2,
            closed: 0,
        }
    );
    let outcome = server.route_envelope(durable_envelope(session_id, 2));
    assert_eq!(outcome.delivered, 2);
    assert_eq!(
        client
            .events_mut()
            .try_recv()
            .expect("first surface event")
            .envelope
            .session_seq,
        Some(2)
    );
    assert_eq!(
        client
            .events_mut()
            .try_recv()
            .expect("second surface event")
            .envelope
            .session_seq,
        Some(2)
    );
}

#[tokio::test]
async fn local_server_client_next_event_buffers_other_surfaces() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let first_session = test_session_id("sess-1");
    let second_session = test_session_id("sess-2");
    for session_id in [&first_session, &second_session] {
        server
            .registry()
            .begin_load(session_id.clone())
            .expect("reserve session");
        server
            .registry()
            .complete_load_success(session_id, TestHandle("handle"))
            .expect("session live");
    }
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut client = LocalServerClient::connect_local(&adapter);
    let first = client
        .subscribe_session(
            first_session.clone(),
            Some(0),
            AttachSurfaceOptions::default(),
        )
        .expect("subscribe first");
    let second = client
        .subscribe_session(
            second_session.clone(),
            Some(0),
            AttachSurfaceOptions::default(),
        )
        .expect("subscribe second");

    server.route_envelope(durable_envelope(second_session.clone(), 1));
    server.route_envelope(durable_envelope(first_session.clone(), 1));

    let first_event = client
        .next_passive_event(&first)
        .await
        .expect("first event");
    assert_eq!(first_event.session_id, first_session);
    let buffered_second = client
        .try_next_passive_event(&second)
        .expect("buffered second event");
    assert_eq!(buffered_second.session_id, second_session);
}

#[test]
fn detach_passive_consumes_only_that_surface() {
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
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut client = LocalServerClient::connect_local(&adapter);
    let _interactive = client
        .attach_interactive_session(session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach interactive");
    let passive = client
        .subscribe_session(session_id, Some(0), AttachSurfaceOptions::default())
        .expect("subscribe passive");

    let detached = client.detach_passive(passive).expect("detach passive");

    assert!(detached.detached_surface.is_some());
    assert_eq!(server.list_live_sessions()[0].surface_counts.attached, 1);
}

#[test]
fn client_lists_live_sessions_with_surface_counts() {
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
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut client = LocalServerClient::connect_local(&adapter);
    let _interactive = client
        .attach_interactive_session(session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach interactive");
    let passive = client
        .subscribe_session(session_id.clone(), Some(0), AttachSurfaceOptions::default())
        .expect("subscribe passive");

    assert_eq!(
        client.list_live_sessions(),
        vec![LocalLiveSessionSummary {
            session_id: session_id.clone(),
            surface_counts: SessionSurfaceCounts {
                attached: 2,
                closed: 0,
            },
        }]
    );

    client.detach_passive(passive).expect("detach passive");

    assert_eq!(
        client.list_live_sessions(),
        vec![LocalLiveSessionSummary {
            session_id,
            surface_counts: SessionSurfaceCounts {
                attached: 1,
                closed: 0,
            },
        }]
    );
}

#[test]
fn session_event_demux_buffers_other_surfaces_on_same_connection() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let interactive_session_id = test_session_id("sess-interactive");
    let passive_session_id = test_session_id("sess-passive");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut client = LocalServerClient::connect_local(&adapter);
    let interactive = client
        .attach_interactive_session(
            interactive_session_id.clone(),
            AttachSurfaceOptions::default(),
        )
        .expect("attach interactive");
    let passive = client
        .subscribe_session(
            passive_session_id.clone(),
            Some(0),
            AttachSurfaceOptions::default(),
        )
        .expect("subscribe passive");

    server.route_envelope(durable_envelope(passive_session_id.clone(), 1));
    server.route_envelope(durable_envelope(interactive_session_id.clone(), 1));

    let interactive_event = client
        .try_next_session_event(&interactive)
        .expect("interactive event");
    let passive_event = client
        .try_next_passive_event(&passive)
        .expect("passive event");

    assert_eq!(interactive_event.session_id, interactive_session_id);
    assert_eq!(passive_event.session_id, passive_session_id);
    assert!(client.try_next_session_event(&interactive).is_none());
    assert!(client.try_next_passive_event(&passive).is_none());
}

#[test]
fn session_request_demux_buffers_other_interactive_surfaces() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let first_session_id = test_session_id("sess-first");
    let second_session_id = test_session_id("sess-second");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut client = LocalServerClient::connect_local(&adapter);
    let first = client
        .attach_interactive_session(
            first_session_id.clone(),
            AttachSurfaceOptions {
                capabilities: SurfaceCapabilities {
                    notifications: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach first interactive");
    let second = client
        .attach_interactive_session(
            second_session_id.clone(),
            AttachSurfaceOptions {
                capabilities: SurfaceCapabilities {
                    notifications: true,
                    ..SurfaceCapabilities::default()
                },
                ..AttachSurfaceOptions::default()
            },
        )
        .expect("attach second interactive");

    let first_route = server
        .route_server_request(
            first_session_id,
            SurfaceCapability::Notifications,
            Some(TurnId::from("turn-first")),
            test_server_request("first"),
        )
        .expect("route first request");
    let second_route = server
        .route_server_request(
            second_session_id,
            SurfaceCapability::Notifications,
            Some(TurnId::from("turn-second")),
            test_server_request("second"),
        )
        .expect("route second request");

    let second_delivery = client
        .try_next_session_request(&second)
        .expect("second request");
    let first_delivery = client
        .try_next_session_request(&first)
        .expect("first request");

    assert_eq!(second_delivery.request_id, second_route.pending.request_id);
    assert_eq!(first_delivery.request_id, first_route.pending.request_id);
    assert!(client.try_next_session_request(&first).is_none());
    assert!(client.try_next_session_request(&second).is_none());
}

#[test]
fn lifecycle_demux_buffers_other_surfaces_on_same_connection() {
    let server = Arc::new(AppServer::<TestHandle>::new(2, 8));
    let interactive_session_id = test_session_id("sess-interactive");
    let passive_session_id = test_session_id("sess-passive");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut client = LocalServerClient::connect_local(&adapter);
    let interactive = client
        .attach_interactive_session(
            interactive_session_id.clone(),
            AttachSurfaceOptions::default(),
        )
        .expect("attach interactive");
    let passive = client
        .subscribe_session(
            passive_session_id.clone(),
            Some(0),
            AttachSurfaceOptions::default(),
        )
        .expect("subscribe passive");

    let outcome = server.route_lifecycle_effects(vec![
        SurfaceLifecycleEffect {
            surface_id: passive.surface_id().clone(),
            kind: SurfaceLifecycleEffectKind::SessionStarted {
                session_id: passive_session_id,
            },
        },
        SurfaceLifecycleEffect {
            surface_id: interactive.surface_id().clone(),
            kind: SurfaceLifecycleEffectKind::SessionStarted {
                session_id: interactive_session_id,
            },
        },
    ]);
    assert_eq!(outcome.delivered, 2);

    let interactive_delivery = client
        .try_next_session_lifecycle(&interactive)
        .expect("interactive lifecycle");
    let passive_delivery = client
        .try_next_passive_lifecycle(&passive)
        .expect("passive lifecycle");

    assert_eq!(
        interactive_delivery.surface_id,
        interactive.surface_id().clone()
    );
    assert_eq!(passive_delivery.surface_id, passive.surface_id().clone());
    assert!(client.try_next_session_lifecycle(&interactive).is_none());
    assert!(client.try_next_passive_lifecycle(&passive).is_none());
}
