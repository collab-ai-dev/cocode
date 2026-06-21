use std::sync::Arc;
use std::sync::OnceLock;

use coco_types::ToolAbortReasonPayload;
use coco_types::TurnAbortReason;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct TurnAbortController {
    token: CancellationToken,
    reason: Arc<OnceLock<TurnAbortReason>>,
}

impl TurnAbortController {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            reason: Arc::new(OnceLock::new()),
        }
    }

    pub fn signal(&self) -> TurnAbortSignal {
        TurnAbortSignal {
            token: self.token.clone(),
            reason: self.reason.clone(),
        }
    }

    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn abort(&self, reason: TurnAbortReason) {
        let _ = self.reason.set(reason);
        self.token.cancel();
    }

    pub fn reason(&self) -> Option<TurnAbortReason> {
        self.reason.get().copied()
    }
}

impl Default for TurnAbortController {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct TurnAbortSignal {
    token: CancellationToken,
    reason: Arc<OnceLock<TurnAbortReason>>,
}

impl TurnAbortSignal {
    pub fn new_aborted(reason: TurnAbortReason) -> Self {
        let controller = TurnAbortController::new();
        controller.abort(reason);
        controller.signal()
    }

    pub fn from_token(token: CancellationToken) -> Self {
        Self {
            token,
            reason: Arc::new(OnceLock::new()),
        }
    }

    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn is_aborted(&self) -> bool {
        self.token.is_cancelled()
    }

    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }

    pub fn reason(&self) -> Option<TurnAbortReason> {
        self.reason.get().copied()
    }
}

#[derive(Debug, Clone)]
pub struct ToolAbortController {
    token: CancellationToken,
    reason: Arc<OnceLock<ToolAbortReasonPayload>>,
}

impl ToolAbortController {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            reason: Arc::new(OnceLock::new()),
        }
    }

    pub fn signal(&self) -> ToolAbortSignalPart {
        ToolAbortSignalPart {
            token: self.token.clone(),
            reason: self.reason.clone(),
        }
    }

    pub fn abort(&self, reason: ToolAbortReasonPayload) {
        let _ = self.reason.set(reason);
        self.token.cancel();
    }
}

impl Default for ToolAbortController {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ToolAbortSignalPart {
    token: CancellationToken,
    reason: Arc<OnceLock<ToolAbortReasonPayload>>,
}

impl ToolAbortSignalPart {
    fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    fn reason(&self) -> Option<ToolAbortReasonPayload> {
        self.reason.get().cloned()
    }
}

#[derive(Debug, Clone)]
pub struct ToolAbortSignal {
    token: CancellationToken,
    turn: TurnAbortSignal,
    self_abort: ToolAbortSignalPart,
    sibling_abort: Option<ToolAbortSignalPart>,
}

impl ToolAbortSignal {
    pub fn new(
        turn: TurnAbortSignal,
        self_abort: ToolAbortSignalPart,
        sibling_abort: Option<ToolAbortSignalPart>,
    ) -> Self {
        let token = turn.token().child_token();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let token_for_task = token.clone();
            let turn_token = turn.token();
            let self_token = self_abort.token();
            let sibling_token = sibling_abort.as_ref().map(ToolAbortSignalPart::token);
            handle.spawn(async move {
                match sibling_token {
                    Some(sibling_token) => {
                        tokio::select! {
                            () = turn_token.cancelled() => {}
                            () = self_token.cancelled() => {}
                            () = sibling_token.cancelled() => {}
                        }
                    }
                    None => {
                        tokio::select! {
                            () = turn_token.cancelled() => {}
                            () = self_token.cancelled() => {}
                        }
                    }
                }
                token_for_task.cancel();
            });
        }

        Self {
            token,
            turn,
            self_abort,
            sibling_abort,
        }
    }

    pub fn from_turn(turn: TurnAbortSignal) -> Self {
        Self {
            token: turn.token(),
            turn,
            self_abort: ToolAbortController::new().signal(),
            sibling_abort: None,
        }
    }

    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn turn_signal(&self) -> TurnAbortSignal {
        self.turn.clone()
    }

    pub fn is_aborted(&self) -> bool {
        self.token.is_cancelled()
    }

    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }

    pub fn reason(&self) -> Option<ToolAbortReasonPayload> {
        if let Some(reason) = self.self_abort.reason() {
            return Some(reason);
        }
        if let Some(reason) = self
            .sibling_abort
            .as_ref()
            .and_then(ToolAbortSignalPart::reason)
        {
            return Some(reason);
        }
        self.turn
            .reason()
            .map(|reason| ToolAbortReasonPayload::Turn { reason })
    }
}
