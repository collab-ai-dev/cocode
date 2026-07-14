use crate::session_runtime::SessionHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMemoryRefresh {
    Dream,
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMemoryRefreshResult {
    Ran,
    NoRuntime,
    NoMemoryRuntime,
}

pub async fn refresh_memory(
    runtime: Option<SessionHandle>,
    refresh: SessionMemoryRefresh,
) -> SessionMemoryRefreshResult {
    let Some(runtime) = runtime else {
        return SessionMemoryRefreshResult::NoRuntime;
    };
    let Some(memory_runtime) = runtime.memory_runtime().cloned() else {
        return SessionMemoryRefreshResult::NoMemoryRuntime;
    };
    match refresh {
        SessionMemoryRefresh::Dream => {
            let transcript_dir = memory_runtime
                .transcript_dir()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let now_ms = coco_memory::service::dream::DreamService::now_ms();
            let _ = memory_runtime
                .dream
                .force(&transcript_dir, Vec::new, now_ms)
                .await;
        }
        SessionMemoryRefresh::Summary => {
            let history = runtime.history_messages().await;
            let tokens = coco_messages::estimate_tokens_for_messages(history.as_slice());
            let last_msg_id = history
                .last()
                .and_then(|message| message.uuid())
                .map(uuid::Uuid::to_string);
            let had_tool_calls =
                coco_messages::count_tool_calls_in_last_assistant_turn(history.as_slice()) > 0;
            let _ = memory_runtime
                .session_memory
                .force(tokens, last_msg_id, had_tool_calls)
                .await;
        }
    }
    SessionMemoryRefreshResult::Ran
}
