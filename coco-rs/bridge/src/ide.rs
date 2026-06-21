//! IDE bridge protocol — VS Code and JetBrains integration.
//!
//! Provides typed message enums for bidirectional communication between
//! the agent and IDE extensions, plus a server that accepts connections
//! and routes messages.

use coco_types::ToolName;
use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{info, warn};

/// Maximum number of recent activities to keep per session.
const MAX_ACTIVITIES: usize = 10;

/// Maximum stderr lines to buffer from a session.
const MAX_STDERR_LINES: usize = 10;

// ---------------------------------------------------------------------------
// IDE Bridge Messages
// ---------------------------------------------------------------------------

/// Messages from IDE to agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IdeBridgeMessage {
    /// Request to open a file in the IDE editor.
    FileOpen {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        column: Option<i32>,
    },
    /// Notification that a file was edited in the IDE.
    FileEdit {
        path: String,
        /// Content after the edit (full file or diff).
        content: String,
        /// Whether this is a full replacement or a diff.
        #[serde(default)]
        is_diff: bool,
    },
    /// Diagnostic (error/warning) from the IDE's language services.
    Diagnostic {
        path: String,
        diagnostics: Vec<IdeDiagnostic>,
    },
    /// Status update from the agent to the IDE.
    StatusUpdate {
        session_id: String,
        state: SessionState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        activity: Option<SessionActivity>,
    },
    /// Permission request from agent to IDE for tool execution.
    PermissionRequest {
        request_id: String,
        tool_name: String,
        tool_use_id: String,
        input: serde_json::Value,
    },
    /// Permission response from IDE to agent.
    PermissionResponse {
        request_id: String,
        approved: bool,
    },
    /// Submit user input text.
    Submit { text: String },
    /// Cancel current operation.
    Cancel,
    /// Keepalive ping.
    Ping,
    /// Keepalive pong.
    Pong,
}

/// A diagnostic entry from the IDE.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeDiagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub line: i32,
    pub column: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Diagnostic severity levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

/// Session state for status updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Initializing,
    Running,
    WaitingForInput,
    WaitingForPermission,
    Completed,
    Failed,
    Interrupted,
}

/// Session activity for status display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionActivity {
    #[serde(rename = "type")]
    pub activity_type: ActivityType,
    pub summary: String,
    pub timestamp: i64,
}

/// Activity type for session status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    ToolStart,
    Text,
    Result,
    Error,
}

// ---------------------------------------------------------------------------
// Tool verb mapping for human-readable status
// ---------------------------------------------------------------------------

/// Map tool names to human-readable verbs for IDE status display.
pub fn tool_verb(tool_name: &str) -> &str {
    if tool_name == ToolName::Read.as_str() {
        "Reading"
    } else if tool_name == ToolName::Write.as_str() {
        "Writing"
    } else if tool_name == ToolName::Edit.as_str() {
        "Editing"
    } else if tool_name == ToolName::Bash.as_str() {
        "Running"
    } else if tool_name == ToolName::Glob.as_str()
        || tool_name == ToolName::Grep.as_str()
        || tool_name == ToolName::WebSearch.as_str()
    {
        "Searching"
    } else if tool_name == ToolName::WebFetch.as_str() {
        "Fetching"
    } else if tool_name == ToolName::NotebookEdit.as_str() {
        "Editing notebook"
    } else if tool_name == ToolName::Lsp.as_str() {
        "LSP"
    } else {
        tool_name
    }
}

/// Build a tool activity summary string.
pub fn tool_summary(tool_name: &str, input: &serde_json::Value) -> String {
    let verb = tool_verb(tool_name);

    let target = input
        .get("file_path")
        .or_else(|| input.get("filePath"))
        .or_else(|| input.get("pattern"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| {
            input
                .get("command")
                .and_then(|v| v.as_str())
                .map(|s| s.chars().take(60).collect())
        })
        .or_else(|| {
            input
                .get("url")
                .or_else(|| input.get("query"))
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .unwrap_or_default();

    if target.is_empty() {
        verb.to_string()
    } else {
        format!("{verb} {target}")
    }
}

// ---------------------------------------------------------------------------
// IDE Bridge Server
// ---------------------------------------------------------------------------

/// State for a connected IDE client.
struct IdeClient {
    /// Sender for writing messages to this client.
    tx: mpsc::Sender<IdeBridgeMessage>,
    /// IDE type identifier.
    ide_type: IdeType,
}

/// Supported IDE types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdeType {
    VsCode,
    JetBrains,
    Unknown,
}

/// IDE bridge server — accepts TCP connections and routes messages
/// between the agent and connected IDEs.
pub struct IdeBridgeServer {
    /// Incoming messages from all connected IDEs.
    incoming_tx: mpsc::Sender<IdeBridgeMessage>,
    incoming_rx: Option<mpsc::Receiver<IdeBridgeMessage>>,
    /// Broadcast channel for outgoing messages to all IDEs.
    outgoing_tx: broadcast::Sender<IdeBridgeMessage>,
    /// Connected clients.
    clients: Arc<Mutex<Vec<IdeClient>>>,
    /// Recent activities per session (bounded ring buffer).
    activities: Arc<Mutex<HashMap<String, Vec<SessionActivity>>>>,
    /// Whether the server is running.
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl IdeBridgeServer {
    /// Create a new IDE bridge server.
    pub fn new() -> Self {
        let (incoming_tx, incoming_rx) = mpsc::channel(256);
        let (outgoing_tx, _) = broadcast::channel(256);
        Self {
            incoming_tx,
            incoming_rx: Some(incoming_rx),
            outgoing_tx,
            clients: Arc::new(Mutex::new(Vec::new())),
            activities: Arc::new(Mutex::new(HashMap::new())),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Start listening for IDE connections on the given address.
    pub async fn listen(&mut self, addr: &str) -> crate::Result<()> {
        let listener = TcpListener::bind(addr).await?;
        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        info!(addr = %addr, "IDE bridge server listening");

        let incoming_tx = self.incoming_tx.clone();
        let outgoing_tx = self.outgoing_tx.clone();
        let clients = Arc::clone(&self.clients);
        let running = Arc::clone(&self.running);

        tokio::spawn(async move {
            while running.load(std::sync::atomic::Ordering::SeqCst) {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        info!(peer = %peer_addr, "IDE client connected");
                        Self::handle_client(
                            stream,
                            incoming_tx.clone(),
                            outgoing_tx.subscribe(),
                            Arc::clone(&clients),
                        )
                        .await;
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to accept IDE connection");
                    }
                }
            }
        });

        Ok(())
    }

    /// Handle a single IDE client connection.
    ///
    /// Spawns two tasks: one for reading messages from the client,
    /// one for writing broadcast messages to the client.
    async fn handle_client(
        stream: TcpStream,
        incoming_tx: mpsc::Sender<IdeBridgeMessage>,
        mut outgoing_rx: broadcast::Receiver<IdeBridgeMessage>,
        clients: Arc<Mutex<Vec<IdeClient>>>,
    ) {
        let (reader, writer) = stream.into_split();
        let (client_tx, mut client_rx) = mpsc::channel::<IdeBridgeMessage>(64);

        // Detect IDE type from the first message or default to Unknown
        let ide_type = IdeType::Unknown;

        // Register client
        {
            let mut clients_guard = clients.lock().await;
            clients_guard.push(IdeClient {
                tx: client_tx,
                ide_type,
            });
        }

        // Reader task: NDJSON lines from client -> incoming channel
        let incoming = incoming_tx;
        tokio::spawn(async move {
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();
            loop {
                line.clear();
                match buf_reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<IdeBridgeMessage>(trimmed) {
                            Ok(msg) => {
                                if incoming.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, line = %trimmed, "failed to parse IDE message");
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "read error from IDE client");
                        break;
                    }
                }
            }
        });

        // Writer task: outgoing broadcast + direct messages -> client
        let mut writer = writer;
        tokio::spawn(async move {
            loop {
                let msg = tokio::select! {
                    msg = outgoing_rx.recv() => match msg {
                        Ok(msg) => msg,
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(lagged = n, "IDE client lagging behind broadcast");
                            continue;
                        }
                    },
                    msg = client_rx.recv() => match msg {
                        Some(msg) => msg,
                        None => break,
                    },
                };

                match serde_json::to_string(&msg) {
                    Ok(json) => {
                        let line = format!("{json}\n");
                        if writer.write_all(line.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to serialize IDE message");
                    }
                }
            }
        });
    }

    /// Take the incoming message receiver (can only be called once).
    pub fn take_incoming_receiver(&mut self) -> Option<mpsc::Receiver<IdeBridgeMessage>> {
        self.incoming_rx.take()
    }

    /// Get a sender for injecting incoming messages (for testing or NDJSON transport).
    pub fn incoming_sender(&self) -> mpsc::Sender<IdeBridgeMessage> {
        self.incoming_tx.clone()
    }

    /// Subscribe to outgoing messages.
    pub fn subscribe_outgoing(&self) -> broadcast::Receiver<IdeBridgeMessage> {
        self.outgoing_tx.subscribe()
    }

    /// Send a message to all connected IDEs.
    pub fn broadcast(&self, msg: IdeBridgeMessage) -> crate::Result<()> {
        self.outgoing_tx
            .send(msg)
            .map_err(|_| crate::BridgeError::NoSubscribers)?;
        Ok(())
    }

    /// Send a file-open request to connected IDEs.
    pub fn open_file(
        &self,
        path: &str,
        line: Option<i32>,
        column: Option<i32>,
    ) -> crate::Result<()> {
        self.broadcast(IdeBridgeMessage::FileOpen {
            path: path.to_string(),
            line,
            column,
        })
    }

    /// Send a status update to connected IDEs.
    pub fn send_status(
        &self,
        session_id: &str,
        state: SessionState,
        model: Option<&str>,
        activity: Option<SessionActivity>,
    ) -> crate::Result<()> {
        self.broadcast(IdeBridgeMessage::StatusUpdate {
            session_id: session_id.to_string(),
            state,
            model: model.map(String::from),
            activity,
        })
    }

    /// Record a session activity and send it to connected IDEs.
    ///
    /// Bounded to `MAX_ACTIVITIES` entries per session.
    pub async fn record_activity(
        &self,
        session_id: &str,
        activity: SessionActivity,
    ) -> crate::Result<()> {
        // Record in bounded buffer
        {
            let mut activities = self.activities.lock().await;
            let entries = activities
                .entry(session_id.to_string())
                .or_insert_with(Vec::new);
            entries.push(activity.clone());
            if entries.len() > MAX_ACTIVITIES {
                entries.remove(0);
            }
        }

        // Send status update
        self.send_status(session_id, SessionState::Running, None, Some(activity))
    }

    /// Get recent activities for a session.
    pub async fn recent_activities(&self, session_id: &str) -> Vec<SessionActivity> {
        let activities = self.activities.lock().await;
        activities
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Whether the server is running.
    pub fn is_running(&self) -> bool {
        self.running
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Stop the server.
    pub fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        info!("IDE bridge server stopped");
    }

    /// Number of connected clients.
    pub async fn client_count(&self) -> usize {
        self.clients.lock().await.len()
    }
}

impl Default for IdeBridgeServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Encode a bridge message as NDJSON.
pub fn encode_ide_ndjson(msg: &IdeBridgeMessage) -> crate::Result<String> {
    let json = serde_json::to_string(msg)?;
    Ok(format!("{json}\n"))
}

/// Decode an NDJSON line into a bridge message.
pub fn decode_ide_ndjson(line: &str) -> crate::Result<IdeBridgeMessage> {
    let msg: IdeBridgeMessage = serde_json::from_str(line.trim())?;
    Ok(msg)
}

#[cfg(test)]
#[path = "ide.test.rs"]
mod tests;
