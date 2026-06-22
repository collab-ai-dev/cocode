use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::debug;
use tracing::warn;

use crate::ExecServerRuntimePaths;
use crate::connection::CHANNEL_CAPACITY;
use crate::connection::JsonRpcConnection;
use crate::connection::JsonRpcConnectionEvent;
use crate::rpc::RpcNotificationSender;
use crate::rpc::RpcServerOutboundMessage;
use crate::rpc::encode_server_message;
use crate::rpc::invalid_request;
use crate::rpc::method_not_found;
use crate::server::ExecServerHandler;
use crate::server::registry::build_router;
use crate::server::session_registry::SessionRegistry;

#[derive(Clone)]
pub(crate) struct ConnectionProcessor {
    session_registry: Arc<SessionRegistry>,
    runtime_paths: ExecServerRuntimePaths,
}

impl ConnectionProcessor {
    pub(crate) fn new(runtime_paths: ExecServerRuntimePaths) -> Self {
        Self {
            session_registry: SessionRegistry::new(),
            runtime_paths,
        }
    }

    pub(crate) async fn run_connection(&self, connection: JsonRpcConnection) {
        run_connection(
            connection,
            Arc::clone(&self.session_registry),
            self.runtime_paths.clone(),
        )
        .await;
    }
}

async fn run_connection(
    connection: JsonRpcConnection,
    session_registry: Arc<SessionRegistry>,
    runtime_paths: ExecServerRuntimePaths,
) {
    let router = Arc::new(build_router());
    let JsonRpcConnection {
        outgoing_tx: json_outgoing_tx,
        mut incoming_rx,
        mut disconnected_rx,
        task_handles: connection_tasks,
        transport: _transport,
    } = connection;
    let (outgoing_tx, mut outgoing_rx) =
        mpsc::channel::<RpcServerOutboundMessage>(CHANNEL_CAPACITY);
    let notifications = RpcNotificationSender::new(outgoing_tx.clone());
    let handler = Arc::new(ExecServerHandler::new(
        session_registry,
        notifications,
        runtime_paths,
    ));
    let mut request_tasks = JoinSet::new();

    let outbound_task = tokio::spawn(async move {
        while let Some(message) = outgoing_rx.recv().await {
            let json_message = match encode_server_message(message) {
                Ok(json_message) => json_message,
                Err(err) => {
                    warn!("failed to serialize exec-server outbound message: {err}");
                    break;
                }
            };
            if json_outgoing_tx.send(json_message).await.is_err() {
                break;
            }
        }
    });

    // Process inbound events sequentially to preserve initialize/initialized ordering.
    while let Some(event) = incoming_rx.recv().await {
        if !handler.is_session_attached() {
            debug!("exec-server connection evicted after session resume");
            break;
        }
        match event {
            JsonRpcConnectionEvent::MalformedMessage { reason } => {
                warn!("ignoring malformed exec-server message: {reason}");
                if outgoing_tx
                    .send(RpcServerOutboundMessage::Error {
                        request_id: crate::jsonrpc_lite::RequestId::Integer(-1),
                        error: invalid_request(reason),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            JsonRpcConnectionEvent::Message(message) => match message {
                crate::jsonrpc_lite::JSONRPCMessage::Request(request) => {
                    if handler.is_initialized()
                        && request.method != crate::protocol::INITIALIZE_METHOD
                    {
                        let method = request.method.clone();
                        if router.request_route(method.as_str()).is_none() {
                            if outgoing_tx
                                .send(RpcServerOutboundMessage::Error {
                                    request_id: request.id,
                                    error: method_not_found(format!(
                                        "exec-server stub does not implement `{method}` yet"
                                    )),
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                            continue;
                        }
                        let handler = Arc::clone(&handler);
                        let router = Arc::clone(&router);
                        let outgoing_tx = outgoing_tx.clone();
                        request_tasks.spawn(async move {
                            let message = {
                                let Some(route) = router.request_route(method.as_str()) else {
                                    return;
                                };
                                route(handler, request).await
                            };
                            if let Some(message) = message {
                                let _ = outgoing_tx.send(message).await;
                            }
                        });
                    } else if let Some(route) = router.request_route(request.method.as_str()) {
                        let message = tokio::select! {
                            message = route(Arc::clone(&handler), request) => message,
                            _ = disconnected_rx.changed() => {
                                debug!("exec-server transport disconnected while handling request");
                                break;
                            }
                        };
                        if let Some(message) = message
                            && outgoing_tx.send(message).await.is_err()
                        {
                            break;
                        }
                    } else if outgoing_tx
                        .send(RpcServerOutboundMessage::Error {
                            request_id: request.id,
                            error: method_not_found(format!(
                                "exec-server stub does not implement `{}` yet",
                                request.method
                            )),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                crate::jsonrpc_lite::JSONRPCMessage::Notification(notification) => {
                    let Some(route) = router.notification_route(notification.method.as_str())
                    else {
                        warn!(
                            "closing exec-server connection after unexpected notification: {}",
                            notification.method
                        );
                        break;
                    };
                    let result = tokio::select! {
                        result = route(Arc::clone(&handler), notification) => result,
                        _ = disconnected_rx.changed() => {
                            debug!(
                                "exec-server transport disconnected while handling notification"
                            );
                            break;
                        }
                    };
                    if let Err(err) = result {
                        warn!("closing exec-server connection after protocol error: {err}");
                        break;
                    }
                }
                crate::jsonrpc_lite::JSONRPCMessage::Response(response) => {
                    warn!(
                        "closing exec-server connection after unexpected client response: {:?}",
                        response.id
                    );
                    break;
                }
                crate::jsonrpc_lite::JSONRPCMessage::Error(error) => {
                    warn!(
                        "closing exec-server connection after unexpected client error: {:?}",
                        error.id
                    );
                    break;
                }
            },
            JsonRpcConnectionEvent::Disconnected { reason } => {
                if let Some(reason) = reason {
                    debug!("exec-server connection disconnected: {reason}");
                }
                break;
            }
        }
    }

    request_tasks.abort_all();
    while request_tasks.join_next().await.is_some() {}
    handler.shutdown().await;
    drop(handler);
    drop(outgoing_tx);
    for task in connection_tasks {
        task.abort();
        let _ = task.await;
    }
    let _ = outbound_task.await;
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::jsonrpc_lite::JSONRPCMessage;
    use crate::jsonrpc_lite::JSONRPCNotification;
    use crate::jsonrpc_lite::JSONRPCRequest;
    use crate::jsonrpc_lite::JSONRPCResponse;
    use crate::jsonrpc_lite::RequestId;
    use coco_utils_path_uri::PathUri;
    use pretty_assertions::assert_eq;
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufReader;
    use tokio::io::DuplexStream;
    use tokio::io::Lines;
    use tokio::io::duplex;
    use tokio::task::JoinHandle;
    use tokio::time::timeout;

    use super::run_connection;
    use crate::ExecServerRuntimePaths;
    use crate::ProcessId;
    use crate::connection::JsonRpcConnection;
    use crate::protocol::ENVIRONMENT_INFO_METHOD;
    use crate::protocol::EXEC_METHOD;
    use crate::protocol::EXEC_READ_METHOD;
    use crate::protocol::EXEC_TERMINATE_METHOD;
    use crate::protocol::EnvironmentInfo;
    use crate::protocol::ExecParams;
    use crate::protocol::ExecResponse;
    use crate::protocol::INITIALIZE_METHOD;
    use crate::protocol::INITIALIZED_METHOD;
    use crate::protocol::InitializeParams;
    use crate::protocol::InitializeResponse;
    use crate::protocol::ReadParams;
    use crate::protocol::TerminateParams;
    use crate::protocol::TerminateResponse;
    use crate::server::session_registry::SessionRegistry;

    #[tokio::test]
    async fn connection_accepts_pipelined_scalar_requests() {
        let registry = SessionRegistry::new();
        let (mut writer, mut lines, task) = spawn_test_connection(registry, "pipelined-scalar");

        send_request(
            &mut writer,
            /*id*/ 1,
            INITIALIZE_METHOD,
            &InitializeParams {
                client_name: "exec-server-test".to_string(),
                resume_session_id: None,
            },
        )
        .await;
        let _: InitializeResponse = read_response(&mut lines, /*expected_id*/ 1).await;
        send_notification(&mut writer, INITIALIZED_METHOD, &()).await;

        send_request(&mut writer, /*id*/ 2, ENVIRONMENT_INFO_METHOD, &()).await;
        send_request(&mut writer, /*id*/ 3, ENVIRONMENT_INFO_METHOD, &()).await;

        let _: EnvironmentInfo = read_response(&mut lines, /*expected_id*/ 2).await;
        let _: EnvironmentInfo = read_response(&mut lines, /*expected_id*/ 3).await;

        drop(writer);
        drop(lines);
        timeout(Duration::from_secs(1), task)
            .await
            .expect("processor should exit")
            .expect("processor should join");
    }

    #[tokio::test]
    async fn transport_disconnect_detaches_session_during_in_flight_read() {
        let registry = SessionRegistry::new();
        let (mut first_writer, mut first_lines, first_task) =
            spawn_test_connection(Arc::clone(&registry), "first");

        send_request(
            &mut first_writer,
            /*id*/ 1,
            INITIALIZE_METHOD,
            &InitializeParams {
                client_name: "exec-server-test".to_string(),
                resume_session_id: None,
            },
        )
        .await;
        let initialize_response: InitializeResponse =
            read_response(&mut first_lines, /*expected_id*/ 1).await;
        send_notification(&mut first_writer, INITIALIZED_METHOD, &()).await;

        let process_id = ProcessId::from("proc-long-poll");
        send_request(
            &mut first_writer,
            /*id*/ 2,
            EXEC_METHOD,
            &exec_params(process_id.clone()),
        )
        .await;
        let _: ExecResponse = read_response(&mut first_lines, /*expected_id*/ 2).await;

        send_request(
            &mut first_writer,
            /*id*/ 3,
            EXEC_READ_METHOD,
            &ReadParams {
                process_id: process_id.clone(),
                after_seq: None,
                max_bytes: None,
                wait_ms: Some(5_000),
            },
        )
        .await;
        drop(first_writer);
        tokio::time::sleep(Duration::from_millis(25)).await;

        let (mut second_writer, mut second_lines, second_task) =
            spawn_test_connection(Arc::clone(&registry), "second");
        send_request(
            &mut second_writer,
            /*id*/ 1,
            INITIALIZE_METHOD,
            &InitializeParams {
                client_name: "exec-server-test".to_string(),
                resume_session_id: Some(initialize_response.session_id.clone()),
            },
        )
        .await;
        let second_initialize_response = timeout(
            Duration::from_secs(1),
            read_response::<InitializeResponse>(&mut second_lines, /*expected_id*/ 1),
        )
        .await
        .expect("resume initialize should not wait for the old read to finish");
        assert_eq!(
            second_initialize_response.session_id,
            initialize_response.session_id
        );
        timeout(Duration::from_secs(1), first_task)
            .await
            .expect("first processor should exit")
            .expect("first processor should join");
        send_notification(&mut second_writer, INITIALIZED_METHOD, &()).await;

        send_request(
            &mut second_writer,
            /*id*/ 2,
            EXEC_TERMINATE_METHOD,
            &TerminateParams { process_id },
        )
        .await;
        let _: TerminateResponse = read_response(&mut second_lines, /*expected_id*/ 2).await;

        drop(second_writer);
        drop(second_lines);
        timeout(Duration::from_secs(1), second_task)
            .await
            .expect("second processor should exit")
            .expect("second processor should join");
    }

    #[tokio::test]
    async fn long_poll_read_does_not_block_later_terminate_request() {
        let registry = SessionRegistry::new();
        let (mut writer, mut lines, task) = spawn_test_connection(registry, "concurrent-requests");

        send_request(
            &mut writer,
            /*id*/ 1,
            INITIALIZE_METHOD,
            &InitializeParams {
                client_name: "exec-server-test".to_string(),
                resume_session_id: None,
            },
        )
        .await;
        let _: InitializeResponse = read_response(&mut lines, /*expected_id*/ 1).await;
        send_notification(&mut writer, INITIALIZED_METHOD, &()).await;

        let process_id = ProcessId::from("proc-concurrent-terminate");
        send_request(
            &mut writer,
            /*id*/ 2,
            EXEC_METHOD,
            &exec_params(process_id.clone()),
        )
        .await;
        let _: ExecResponse = read_response(&mut lines, /*expected_id*/ 2).await;

        send_request(
            &mut writer,
            /*id*/ 3,
            EXEC_READ_METHOD,
            &ReadParams {
                process_id: process_id.clone(),
                after_seq: None,
                max_bytes: None,
                wait_ms: Some(5_000),
            },
        )
        .await;
        send_request(
            &mut writer,
            /*id*/ 4,
            EXEC_TERMINATE_METHOD,
            &TerminateParams { process_id },
        )
        .await;

        let _: TerminateResponse = read_response_eventually(&mut lines, /*expected_id*/ 4).await;

        drop(writer);
        drop(lines);
        timeout(Duration::from_secs(1), task)
            .await
            .expect("processor should exit")
            .expect("processor should join");
    }

    fn spawn_test_connection(
        registry: Arc<SessionRegistry>,
        label: &str,
    ) -> (DuplexStream, Lines<BufReader<DuplexStream>>, JoinHandle<()>) {
        let (client_writer, server_reader) = duplex(1 << 20);
        let (server_writer, client_reader) = duplex(1 << 20);
        let connection =
            JsonRpcConnection::from_stdio(server_reader, server_writer, label.to_string());
        let task = tokio::spawn(run_connection(connection, registry, test_runtime_paths()));
        (client_writer, BufReader::new(client_reader).lines(), task)
    }

    fn test_runtime_paths() -> ExecServerRuntimePaths {
        ExecServerRuntimePaths::new(
            std::env::current_exe().expect("current exe"),
            /*coco_linux_sandbox_exe*/ None,
        )
        .expect("runtime paths")
    }

    async fn send_request<P: Serialize>(
        writer: &mut DuplexStream,
        id: i64,
        method: &str,
        params: &P,
    ) {
        write_message(
            writer,
            &JSONRPCMessage::Request(JSONRPCRequest {
                id: RequestId::Integer(id),
                method: method.to_string(),
                params: Some(serde_json::to_value(params).expect("serialize params")),
                trace: None,
            }),
        )
        .await;
    }

    async fn send_notification<P: Serialize>(writer: &mut DuplexStream, method: &str, params: &P) {
        write_message(
            writer,
            &JSONRPCMessage::Notification(JSONRPCNotification {
                method: method.to_string(),
                params: Some(serde_json::to_value(params).expect("serialize params")),
            }),
        )
        .await;
    }

    async fn write_message(writer: &mut DuplexStream, message: &JSONRPCMessage) {
        let encoded = serde_json::to_vec(message).expect("serialize JSON-RPC message");
        writer.write_all(&encoded).await.expect("write request");
        writer.write_all(b"\n").await.expect("write newline");
    }

    async fn read_response<T: DeserializeOwned>(
        lines: &mut Lines<BufReader<DuplexStream>>,
        expected_id: i64,
    ) -> T {
        let line = lines
            .next_line()
            .await
            .expect("read response")
            .expect("response line");
        match serde_json::from_str::<JSONRPCMessage>(&line).expect("decode JSON-RPC response") {
            JSONRPCMessage::Response(JSONRPCResponse { id, result }) => {
                assert_eq!(id, RequestId::Integer(expected_id));
                serde_json::from_value(result).expect("decode response result")
            }
            JSONRPCMessage::Error(error) => panic!("unexpected JSON-RPC error: {error:?}"),
            other => panic!("expected JSON-RPC response, got {other:?}"),
        }
    }

    async fn read_response_eventually<T: DeserializeOwned>(
        lines: &mut Lines<BufReader<DuplexStream>>,
        expected_id: i64,
    ) -> T {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for response {expected_id}"
            );
            let line = timeout(remaining, lines.next_line())
                .await
                .expect("response should arrive before timeout")
                .expect("response read should succeed")
                .expect("response line should be present");
            match serde_json::from_str::<JSONRPCMessage>(&line).expect("decode JSON-RPC message") {
                JSONRPCMessage::Response(JSONRPCResponse { id, result })
                    if id == RequestId::Integer(expected_id) =>
                {
                    return serde_json::from_value(result).expect("decode response result");
                }
                JSONRPCMessage::Error(error) if error.id == RequestId::Integer(expected_id) => {
                    panic!("unexpected JSON-RPC error: {error:?}");
                }
                _ => {}
            }
        }
    }

    fn exec_params(process_id: ProcessId) -> ExecParams {
        let mut env = HashMap::new();
        if let Some(path) = std::env::var_os("PATH") {
            env.insert("PATH".to_string(), path.to_string_lossy().into_owned());
        }
        ExecParams {
            process_id,
            argv: sleep_then_print_argv(),
            cwd: PathUri::from_path(std::env::current_dir().expect("cwd")).expect("cwd URI"),
            env_policy: None,
            env,
            tty: false,
            pipe_stdin: false,
            arg0: None,
            sandbox: None,
            enforce_managed_network: false,
        }
    }

    fn sleep_then_print_argv() -> Vec<String> {
        if cfg!(windows) {
            vec![
                std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string()),
                "/C".to_string(),
                "ping -n 3 127.0.0.1 >NUL && echo late".to_string(),
            ]
        } else {
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "sleep 1; printf late".to_string(),
            ]
        }
    }
}
