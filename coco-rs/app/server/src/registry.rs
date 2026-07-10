use std::collections::HashMap;
use std::sync::RwLock;

use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use coco_types::SessionId;
use snafu::Snafu;

type LoadResult<H> = Option<Result<H, RegistryError>>;

/// Registry for root session lifecycle slots.
///
/// The registry owns only slot state and completion signals. Runtime
/// construction, close cascade, and owner-task spawning are wired by AppServer.
pub struct LiveSessionRegistry<H> {
    pub(crate) sessions: RwLock<HashMap<SessionId, SessionSlot<H>>>,
    max_sessions: usize,
}

impl<H: Clone> LiveSessionRegistry<H> {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            max_sessions,
        }
    }

    pub fn max_sessions(&self) -> usize {
        self.max_sessions
    }

    pub fn slot_count(&self) -> usize {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    pub fn live_count(&self) -> usize {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|slot| matches!(slot, SessionSlot::Live(_)))
            .count()
    }

    pub fn get(&self, session_id: &SessionId) -> Option<H> {
        match self
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
        {
            Some(SessionSlot::Live(handle)) => Some(handle.clone()),
            _ => None,
        }
    }

    pub fn replace_live_handle(
        &self,
        session_id: &SessionId,
        handle: H,
    ) -> Result<H, RegistryError> {
        let mut sessions = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(slot @ SessionSlot::Live(_)) = sessions.get_mut(session_id) else {
            return SlotConflictSnafu {
                session_id: session_id.clone(),
                expected: "Live",
            }
            .fail();
        };
        let SessionSlot::Live(previous) = std::mem::replace(slot, SessionSlot::Live(handle)) else {
            unreachable!("slot was matched as Live above");
        };
        Ok(previous)
    }

    pub fn list_live(&self) -> Vec<SessionId> {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|&(_, slot)| matches!(slot, SessionSlot::Live(_)))
            .map(|(session_id, _)| session_id.clone())
            .collect()
    }

    pub fn list_closable(&self) -> Vec<SessionId> {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    pub fn begin_load(&self, session_id: SessionId) -> Result<LoadStart<H>, RegistryError> {
        let mut sessions = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match sessions.get(&session_id) {
            Some(SessionSlot::Loading(load)) => {
                return Ok(LoadStart::Loading(load.completion()));
            }
            Some(SessionSlot::Live(handle)) => {
                return Ok(LoadStart::Live(handle.clone()));
            }
            Some(SessionSlot::Closing(closing)) => {
                return Ok(LoadStart::Closing(closing.close.completion()));
            }
            None => {}
        }
        if sessions.len() >= self.max_sessions {
            return ResourceExhaustedSnafu.fail();
        }

        sessions.insert(session_id, SessionSlot::Loading(LoadState::new()));
        Ok(LoadStart::Reserved)
    }

    pub fn complete_load_success(
        &self,
        session_id: &SessionId,
        handle: H,
    ) -> Result<(), RegistryError> {
        let mut sessions = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(SessionSlot::Loading(mut load)) = sessions.remove(session_id) else {
            return SlotConflictSnafu {
                session_id: session_id.clone(),
                expected: "Loading",
            }
            .fail();
        };
        let _ = load.sender.send(Some(Ok(handle.clone())));
        let next_slot = match load.close_after_load.take() {
            Some(close) => SessionSlot::Closing(ClosingState { handle, close }),
            None => SessionSlot::Live(handle),
        };
        sessions.insert(session_id.clone(), next_slot);
        Ok(())
    }

    pub fn complete_load_failure(
        &self,
        session_id: &SessionId,
        error: RegistryError,
    ) -> Result<(), RegistryError> {
        let mut sessions = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(SessionSlot::Loading(load)) = sessions.remove(session_id) else {
            return SlotConflictSnafu {
                session_id: session_id.clone(),
                expected: "Loading",
            }
            .fail();
        };
        let _ = load.sender.send(Some(Err(error)));
        if let Some(close) = load.close_after_load {
            let _ = close.sender.send(true);
        }
        Ok(())
    }

    pub fn begin_close(&self, session_id: &SessionId) -> Result<CloseStart<H>, RegistryError> {
        let mut sessions = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match sessions.get_mut(session_id) {
            Some(SessionSlot::Loading(load)) => {
                let load_completion = load.completion();
                let should_spawn = load.close_after_load.is_none();
                let close_completion = load
                    .close_after_load
                    .get_or_insert_with(CloseState::new)
                    .completion();
                Ok(CloseStart::Loading {
                    load_completion,
                    close_completion,
                    should_spawn,
                })
            }
            Some(SessionSlot::Closing(closing)) => Ok(CloseStart::Closing {
                handle: closing.handle.clone(),
                completion: closing.close.completion(),
            }),
            Some(SessionSlot::Live(handle)) => {
                let handle = handle.clone();
                let close = CloseState::new();
                let completion = close.completion();
                sessions.insert(
                    session_id.clone(),
                    SessionSlot::Closing(ClosingState {
                        handle: handle.clone(),
                        close,
                    }),
                );
                Ok(CloseStart::Started { handle, completion })
            }
            None => NotFoundSnafu {
                session_id: session_id.clone(),
            }
            .fail(),
        }
    }

    pub fn begin_replace(
        &self,
        old_session_id: &SessionId,
        new_session_id: SessionId,
    ) -> Result<ReplaceStart<H>, RegistryError> {
        let mut sessions = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(SessionSlot::Live(old_handle)) = sessions.get(old_session_id) else {
            return OldNotReadySnafu {
                session_id: old_session_id.clone(),
            }
            .fail();
        };
        if sessions.contains_key(&new_session_id) {
            return NewSlotOccupiedSnafu {
                session_id: new_session_id,
            }
            .fail();
        }

        let new_load = LoadState::new();
        let new_completion = new_load.completion();
        let old_handle = old_handle.clone();
        sessions.insert(new_session_id, SessionSlot::Loading(new_load));
        Ok(ReplaceStart::Reserved {
            old_handle,
            new_completion,
        })
    }

    pub fn complete_replace_success(
        &self,
        old_session_id: &SessionId,
        new_session_id: &SessionId,
        new_handle: H,
    ) -> Result<ReplaceCommit<H>, RegistryError> {
        let mut sessions = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old_handle = match sessions.get(old_session_id) {
            Some(SessionSlot::Live(handle)) => handle.clone(),
            _ => {
                return OldNotReadySnafu {
                    session_id: old_session_id.clone(),
                }
                .fail();
            }
        };
        let new_load_sender = match sessions.get(new_session_id) {
            Some(SessionSlot::Loading(load)) => load.sender.clone(),
            _ => {
                return SlotConflictSnafu {
                    session_id: new_session_id.clone(),
                    expected: "Loading",
                }
                .fail();
            }
        };

        let old_close = CloseState::new();
        let old_close_completion = old_close.completion();
        let _ = new_load_sender.send(Some(Ok(new_handle.clone())));
        sessions.insert(new_session_id.clone(), SessionSlot::Live(new_handle));
        sessions.insert(
            old_session_id.clone(),
            SessionSlot::Closing(ClosingState {
                handle: old_handle.clone(),
                close: old_close,
            }),
        );
        Ok(ReplaceCommit {
            old_handle,
            old_close_completion,
        })
    }

    pub fn complete_replace_failure(
        &self,
        new_session_id: &SessionId,
        error: RegistryError,
    ) -> Result<(), RegistryError> {
        self.complete_load_failure(new_session_id, error)
    }

    pub fn complete_close(&self, session_id: &SessionId) -> Result<(), RegistryError> {
        let mut sessions = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(SessionSlot::Closing(closing)) = sessions.get(session_id) else {
            return SlotConflictSnafu {
                session_id: session_id.clone(),
                expected: "Closing",
            }
            .fail();
        };
        let _ = closing.close.sender.send(true);
        sessions.remove(session_id);
        Ok(())
    }
}

pub(crate) enum SessionSlot<H> {
    Loading(LoadState<H>),
    Live(H),
    Closing(ClosingState<H>),
}

pub(crate) struct ClosingState<H> {
    pub(crate) handle: H,
    pub(crate) close: CloseState,
}

pub(crate) struct LoadState<H> {
    pub(crate) sender: tokio::sync::watch::Sender<LoadResult<H>>,
    receiver: tokio::sync::watch::Receiver<LoadResult<H>>,
    close_after_load: Option<CloseState>,
}

impl<H: Clone> LoadState<H> {
    fn new() -> Self {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        Self {
            sender,
            receiver,
            close_after_load: None,
        }
    }

    fn completion(&self) -> LoadCompletion<H> {
        LoadCompletion {
            receiver: self.receiver.clone(),
        }
    }
}

pub(crate) struct CloseState {
    pub(crate) sender: tokio::sync::watch::Sender<bool>,
    receiver: tokio::sync::watch::Receiver<bool>,
}

impl CloseState {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = tokio::sync::watch::channel(false);
        Self { sender, receiver }
    }

    pub(crate) fn completion(&self) -> CloseCompletion {
        CloseCompletion {
            receiver: self.receiver.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum LoadStart<H> {
    Reserved,
    Live(H),
    Loading(LoadCompletion<H>),
    Closing(CloseCompletion),
}

#[derive(Debug, Clone)]
pub enum CloseStart<H> {
    Started {
        handle: H,
        completion: CloseCompletion,
    },
    Loading {
        load_completion: LoadCompletion<H>,
        close_completion: CloseCompletion,
        should_spawn: bool,
    },
    Closing {
        handle: H,
        completion: CloseCompletion,
    },
}

#[derive(Debug, Clone)]
pub enum ReplaceStart<H> {
    Reserved {
        old_handle: H,
        new_completion: LoadCompletion<H>,
    },
}

#[derive(Debug, Clone)]
pub struct ReplaceCommit<H> {
    pub old_handle: H,
    pub old_close_completion: CloseCompletion,
}

#[derive(Debug, Clone)]
pub struct LoadCompletion<H> {
    receiver: tokio::sync::watch::Receiver<LoadResult<H>>,
}

impl<H: Clone> LoadCompletion<H> {
    pub fn ready(&self) -> Option<Result<H, RegistryError>> {
        self.receiver.borrow().clone()
    }

    pub async fn wait(&mut self) -> Result<H, RegistryError> {
        loop {
            if let Some(result) = self.ready() {
                return result;
            }
            self.receiver
                .changed()
                .await
                .map_err(|_| SignalDroppedSnafu.build())?;
        }
    }
}

#[derive(Debug, Clone)]
pub struct CloseCompletion {
    receiver: tokio::sync::watch::Receiver<bool>,
}

impl CloseCompletion {
    pub fn is_complete(&self) -> bool {
        *self.receiver.borrow()
    }

    pub async fn wait(&mut self) -> Result<(), RegistryError> {
        loop {
            if self.is_complete() {
                return Ok(());
            }
            self.receiver
                .changed()
                .await
                .map_err(|_| SignalDroppedSnafu.build())?;
        }
    }
}

#[stack_trace_debug]
#[derive(Snafu, Clone)]
#[snafu(visibility(pub(crate)))]
pub enum RegistryError {
    #[snafu(display("session not found: {session_id}"))]
    NotFound {
        session_id: SessionId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("max_sessions limit reached"))]
    ResourceExhausted {
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("old session is not live: {session_id}"))]
    OldNotReady {
        session_id: SessionId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("new session slot is already occupied: {session_id}"))]
    NewSlotOccupied {
        session_id: SessionId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("session {session_id} was not in expected {expected} slot"))]
    SlotConflict {
        session_id: SessionId,
        expected: &'static str,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("session load failed: {message}"))]
    LoadFailed {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("registry completion signal dropped"))]
    SignalDropped {
        #[snafu(implicit)]
        location: Location,
    },
}

impl RegistryError {
    pub fn load_failed(message: impl Into<String>) -> Self {
        LoadFailedSnafu {
            message: message.into(),
        }
        .build()
    }
}

impl ErrorExt for RegistryError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::NotFound { .. } => StatusCode::FileNotFound,
            Self::ResourceExhausted { .. } => StatusCode::ResourcesExhausted,
            Self::OldNotReady { .. } => StatusCode::InvalidArguments,
            Self::NewSlotOccupied { .. } => StatusCode::InvalidArguments,
            Self::SlotConflict { .. } => StatusCode::InvalidArguments,
            Self::LoadFailed { .. } => StatusCode::Internal,
            Self::SignalDropped { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
#[path = "registry.test.rs"]
mod tests;
