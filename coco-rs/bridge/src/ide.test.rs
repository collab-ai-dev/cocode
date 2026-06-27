use coco_types::ToolName;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_ide_bridge_message_file_open_serialization() {
    let msg = IdeBridgeMessage::FileOpen {
        path: "/src/main.rs".to_string(),
        line: Some(42),
        column: Some(10),
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"file_open\""));
    assert!(json.contains("\"line\":42"));

    let deserialized: IdeBridgeMessage = serde_json::from_str(&json).unwrap();
    match deserialized {
        IdeBridgeMessage::FileOpen { path, line, column } => {
            assert_eq!(path, "/src/main.rs");
            assert_eq!(line, Some(42));
            assert_eq!(column, Some(10));
        }
        _ => panic!("expected FileOpen"),
    }
}

#[test]
fn test_ide_bridge_message_file_edit_serialization() {
    let msg = IdeBridgeMessage::FileEdit {
        path: "/src/lib.rs".to_string(),
        content: "fn main() {}".to_string(),
        is_diff: false,
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"file_edit\""));

    let deserialized: IdeBridgeMessage = serde_json::from_str(&json).unwrap();
    match deserialized {
        IdeBridgeMessage::FileEdit {
            path,
            content,
            is_diff,
        } => {
            assert_eq!(path, "/src/lib.rs");
            assert_eq!(content, "fn main() {}");
            assert!(!is_diff);
        }
        _ => panic!("expected FileEdit"),
    }
}

#[test]
fn test_ide_bridge_message_diagnostic_serialization() {
    let msg = IdeBridgeMessage::Diagnostic {
        path: "/src/main.rs".to_string(),
        diagnostics: vec![IdeDiagnostic {
            severity: DiagnosticSeverity::Error,
            message: "unused variable".to_string(),
            line: 10,
            column: 5,
            end_line: Some(10),
            end_column: Some(8),
            source: Some("rust-analyzer".to_string()),
            code: Some("E0599".to_string()),
        }],
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"diagnostic\""));
    assert!(json.contains("\"severity\":\"error\""));

    let deserialized: IdeBridgeMessage = serde_json::from_str(&json).unwrap();
    match deserialized {
        IdeBridgeMessage::Diagnostic { diagnostics, .. } => {
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
            assert_eq!(diagnostics[0].message, "unused variable");
        }
        _ => panic!("expected Diagnostic"),
    }
}

#[test]
fn test_ide_bridge_message_status_update_serialization() {
    let msg = IdeBridgeMessage::StatusUpdate {
        session_id: "session-123".to_string(),
        state: SessionState::Running,
        model: Some("claude-3-5-sonnet".to_string()),
        activity: Some(SessionActivity {
            activity_type: ActivityType::ToolStart,
            summary: "Reading main.rs".to_string(),
            timestamp: 1234567890,
        }),
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"status_update\""));
    assert!(json.contains("\"state\":\"running\""));

    let deserialized: IdeBridgeMessage = serde_json::from_str(&json).unwrap();
    match deserialized {
        IdeBridgeMessage::StatusUpdate {
            session_id,
            state,
            model,
            activity,
        } => {
            assert_eq!(session_id, "session-123");
            assert_eq!(state, SessionState::Running);
            assert_eq!(model.as_deref(), Some("claude-3-5-sonnet"));
            assert!(activity.is_some());
            assert_eq!(activity.unwrap().summary, "Reading main.rs");
        }
        _ => panic!("expected StatusUpdate"),
    }
}

#[test]
fn test_ide_bridge_message_permission_request_serialization() {
    let msg = IdeBridgeMessage::PermissionRequest {
        request_id: "req-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_use_id: "tu-1".to_string(),
        input: serde_json::json!({"command": "rm -rf /tmp/test"}),
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"permission_request\""));

    let deserialized: IdeBridgeMessage = serde_json::from_str(&json).unwrap();
    match deserialized {
        IdeBridgeMessage::PermissionRequest {
            tool_name, input, ..
        } => {
            assert_eq!(tool_name, "Bash");
            assert_eq!(input["command"], "rm -rf /tmp/test");
        }
        _ => panic!("expected PermissionRequest"),
    }
}

#[test]
fn test_session_state_serialization() {
    let states = vec![
        (SessionState::Initializing, "\"initializing\""),
        (SessionState::Running, "\"running\""),
        (SessionState::WaitingForInput, "\"waiting_for_input\""),
        (
            SessionState::WaitingForPermission,
            "\"waiting_for_permission\"",
        ),
        (SessionState::Completed, "\"completed\""),
        (SessionState::Failed, "\"failed\""),
        (SessionState::Interrupted, "\"interrupted\""),
    ];

    for (state, expected) in states {
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, expected);
    }
}

#[test]
fn test_diagnostic_severity_serialization() {
    let severities = vec![
        (DiagnosticSeverity::Error, "\"error\""),
        (DiagnosticSeverity::Warning, "\"warning\""),
        (DiagnosticSeverity::Info, "\"info\""),
        (DiagnosticSeverity::Hint, "\"hint\""),
    ];

    for (severity, expected) in severities {
        let json = serde_json::to_string(&severity).unwrap();
        assert_eq!(json, expected);
    }
}

#[test]
fn test_tool_verb_known_tools() {
    assert_eq!(tool_verb(ToolName::Read.as_str()), "Reading");
    assert_eq!(tool_verb(ToolName::Write.as_str()), "Writing");
    assert_eq!(tool_verb(ToolName::Edit.as_str()), "Editing");
    assert_eq!(tool_verb(ToolName::Bash.as_str()), "Running");
    assert_eq!(tool_verb(ToolName::Glob.as_str()), "Searching");
    assert_eq!(tool_verb(ToolName::Grep.as_str()), "Searching");
    assert_eq!(tool_verb(ToolName::WebFetch.as_str()), "Fetching");
    assert_eq!(tool_verb(ToolName::WebSearch.as_str()), "Searching");
    assert_eq!(tool_verb(ToolName::Lsp.as_str()), "LSP");
}

#[test]
fn test_tool_verb_unknown_tool() {
    assert_eq!(tool_verb("CustomTool"), "CustomTool");
}

#[test]
fn test_tool_summary_with_file_path() {
    let input = serde_json::json!({"file_path": "/src/main.rs"});
    assert_eq!(tool_summary("Read", &input), "Reading /src/main.rs");
}

#[test]
fn test_tool_summary_with_command() {
    let input = serde_json::json!({"command": "cargo build --release"});
    assert_eq!(tool_summary("Bash", &input), "Running cargo build --release");
}

#[test]
fn test_tool_summary_with_pattern() {
    let input = serde_json::json!({"pattern": "*.rs"});
    assert_eq!(tool_summary("Glob", &input), "Searching *.rs");
}

#[test]
fn test_tool_summary_no_target() {
    let input = serde_json::json!({});
    assert_eq!(tool_summary("Read", &input), "Reading");
}

#[test]
fn test_tool_summary_command_truncation() {
    let long_command = "x".repeat(100);
    let input = serde_json::json!({"command": long_command});
    let summary = tool_summary("Bash", &input);
    // Command should be truncated to 60 chars
    assert!(summary.len() <= 70); // "Running " (8) + 60 chars
}

#[test]
fn test_encode_decode_ndjson_roundtrip() {
    let msg = IdeBridgeMessage::Ping;
    let encoded = encode_ide_ndjson(&msg).unwrap();
    assert!(encoded.ends_with('\n'));

    let decoded = decode_ide_ndjson(&encoded).unwrap();
    assert!(matches!(decoded, IdeBridgeMessage::Ping));
}

#[test]
fn test_decode_ndjson_with_whitespace() {
    let line = "  {\"type\":\"ping\"}  \n";
    let msg = decode_ide_ndjson(line).unwrap();
    assert!(matches!(msg, IdeBridgeMessage::Ping));
}

#[tokio::test]
async fn test_ide_bridge_server_creation() {
    let server = IdeBridgeServer::new();
    assert!(!server.is_running());
    assert_eq!(server.client_count().await, 0);
}

#[tokio::test]
async fn test_ide_bridge_server_broadcast() {
    let server = IdeBridgeServer::new();
    let mut rx = server.subscribe_outgoing();

    server.broadcast(IdeBridgeMessage::Ping).unwrap();

    let msg = rx.recv().await.unwrap();
    assert!(matches!(msg, IdeBridgeMessage::Ping));
}

#[tokio::test]
async fn test_ide_bridge_server_incoming_channel() {
    let mut server = IdeBridgeServer::new();
    let tx = server.incoming_sender();
    let mut rx = server.take_incoming_receiver().unwrap();

    tx.send(IdeBridgeMessage::Cancel).await.unwrap();
    let msg = rx.recv().await.unwrap();
    assert!(matches!(msg, IdeBridgeMessage::Cancel));
}

#[tokio::test]
async fn test_ide_bridge_server_record_activity() {
    let server = IdeBridgeServer::new();
    let _rx = server.subscribe_outgoing();

    let activity = SessionActivity {
        activity_type: ActivityType::ToolStart,
        summary: "Reading file".to_string(),
        timestamp: 1000,
    };

    server
        .record_activity("session-1", activity)
        .await
        .unwrap();

    let activities = server.recent_activities("session-1").await;
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].summary, "Reading file");
}

#[tokio::test]
async fn test_ide_bridge_server_activity_bounded() {
    let server = IdeBridgeServer::new();
    let _rx = server.subscribe_outgoing();

    // Add more than MAX_ACTIVITIES
    for i in 0..MAX_ACTIVITIES + 5 {
        let activity = SessionActivity {
            activity_type: ActivityType::ToolStart,
            summary: format!("Activity {i}"),
            timestamp: i as i64,
        };
        server
            .record_activity("session-1", activity)
            .await
            .unwrap();
    }

    let activities = server.recent_activities("session-1").await;
    assert_eq!(activities.len(), MAX_ACTIVITIES);
    // Should have the latest activities
    assert_eq!(activities[0].summary, "Activity 5");
}

#[tokio::test]
async fn test_ide_bridge_server_open_file() {
    let server = IdeBridgeServer::new();
    let mut rx = server.subscribe_outgoing();

    server.open_file("/src/main.rs", Some(10), None).unwrap();

    let msg = rx.recv().await.unwrap();
    match msg {
        IdeBridgeMessage::FileOpen { path, line, column } => {
            assert_eq!(path, "/src/main.rs");
            assert_eq!(line, Some(10));
            assert!(column.is_none());
        }
        _ => panic!("expected FileOpen"),
    }
}

#[tokio::test]
async fn test_ide_bridge_server_send_status() {
    let server = IdeBridgeServer::new();
    let mut rx = server.subscribe_outgoing();

    server
        .send_status("s1", SessionState::Completed, Some("claude"), None)
        .unwrap();

    let msg = rx.recv().await.unwrap();
    match msg {
        IdeBridgeMessage::StatusUpdate {
            session_id,
            state,
            model,
            ..
        } => {
            assert_eq!(session_id, "s1");
            assert_eq!(state, SessionState::Completed);
            assert_eq!(model.as_deref(), Some("claude"));
        }
        _ => panic!("expected StatusUpdate"),
    }
}

#[tokio::test]
async fn test_ide_bridge_server_stop() {
    let server = IdeBridgeServer::new();
    assert!(!server.is_running());

    // Manually set running for test
    server
        .running
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(server.is_running());

    server.stop();
    assert!(!server.is_running());
}
