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
        self.sessions.read().expect("registry lock poisoned").len()
    }

    pub fn live_count(&self) -> usize {
        self.sessions
            .read()
            .expect("registry lock poisoned")
            .values()
            .filter(|slot| matches!(slot, SessionSlot::Live(_)))
            .count()
    }

    pub fn get(&self, session_id: &SessionId) -> Option<H> {
        match self
            .sessions
            .read()
            .expect("registry lock poisoned")
            .get(session_id)
        {
            Some(SessionSlot::Live(handle)) => Some(handle.clone()),
            _ => None,
        }
    }

    pub fn list_live(&self) -> Vec<SessionId> {
        self.sessions
            .read()
            .expect("registry lock poisoned")
            .iter()
            .filter_map(|(session_id, slot)| {
                matches!(slot, SessionSlot::Live(_)).then(|| session_id.clone())
            })
            .collect()
    }

    pub fn begin_load(&self, session_id: SessionId) -> Result<LoadStart<H>, RegistryError> {
        let mut sessions = self.sessions.write().expect("registry lock poisoned");
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
        let mut sessions = self.sessions.write().expect("registry lock poisoned");
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
        let mut sessions = self.sessions.write().expect("registry lock poisoned");
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
        let mut sessions = self.sessions.write().expect("registry lock poisoned");
        match sessions.get_mut(session_id) {
            Some(SessionSlot::Loading(load)) => {
                let load_completion = load.completion();
                let mut should_spawn = false;
                if load.close_after_load.is_none() {
                    load.close_after_load = Some(CloseState::new());
                    should_spawn = true;
                }
                let close_completion = load
                    .close_after_load
                    .as_ref()
                    .expect("close-after-load state must exist")
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
        let mut sessions = self.sessions.write().expect("registry lock poisoned");
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
        let mut sessions = self.sessions.write().expect("registry lock poisoned");
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
        let mut sessions = self.sessions.write().expect("registry lock poisoned");
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
    #[snafu(display("registry completion signal dropped"))]
    SignalDropped {
        #[snafu(implicit)]
        location: Location,
    },
}

impl ErrorExt for RegistryError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::NotFound { .. } => StatusCode::FileNotFound,
            Self::ResourceExhausted { .. } => StatusCode::ResourcesExhausted,
            Self::OldNotReady { .. } => StatusCode::InvalidArguments,
            Self::NewSlotOccupied { .. } => StatusCode::InvalidArguments,
            Self::SlotConflict { .. } => StatusCode::InvalidArguments,
            Self::SignalDropped { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session_id(value: &str) -> SessionId {
        SessionId::try_new(value).expect("valid test session id")
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestHandle(&'static str);

    #[test]
    fn load_slot_promotes_to_live_and_unblocks_waiters() {
        let registry = LiveSessionRegistry::new(4);
        let session_id = test_session_id("sess-1");
        assert!(matches!(
            registry
                .begin_load(session_id.clone())
                .expect("reserve load"),
            LoadStart::Reserved
        ));
        let LoadStart::Loading(waiter) = registry
            .begin_load(session_id.clone())
            .expect("observe loading")
        else {
            panic!("expected loading");
        };

        registry
            .complete_load_success(&session_id, TestHandle("h1"))
            .expect("complete load");

        assert_eq!(
            waiter.ready().expect("load ready").expect("load ok"),
            TestHandle("h1")
        );
        assert_eq!(registry.get(&session_id), Some(TestHandle("h1")));
        assert_eq!(registry.live_count(), 1);
    }

    #[test]
    fn load_failure_removes_slot_and_unblocks_waiters() {
        let registry = LiveSessionRegistry::<TestHandle>::new(4);
        let session_id = test_session_id("sess-1");
        registry
            .begin_load(session_id.clone())
            .expect("reserve load");
        let LoadStart::Loading(waiter) = registry
            .begin_load(session_id.clone())
            .expect("observe loading")
        else {
            panic!("expected loading");
        };
        let error = NotFoundSnafu {
            session_id: session_id.clone(),
        }
        .build();

        registry
            .complete_load_failure(&session_id, error.clone())
            .expect("complete failure");

        let Err(ready_error) = waiter.ready().expect("load ready") else {
            panic!("expected load error");
        };
        assert!(matches!(ready_error, RegistryError::NotFound { .. }));
        assert_eq!(ready_error.status_code(), error.status_code());
        assert_eq!(registry.slot_count(), 0);
    }

    #[test]
    fn max_sessions_counts_loading_live_and_closing_slots() {
        let registry = LiveSessionRegistry::new(2);
        let loading = test_session_id("sess-loading");
        let live = test_session_id("sess-live");
        let blocked = test_session_id("sess-blocked");
        registry.begin_load(loading).expect("reserve loading slot");
        registry
            .begin_load(live.clone())
            .expect("reserve live slot");
        registry
            .complete_load_success(&live, TestHandle("live"))
            .expect("complete live");

        let err = registry
            .begin_load(blocked)
            .expect_err("registry should be full");

        assert!(matches!(err, RegistryError::ResourceExhausted { .. }));
        assert_eq!(err.status_code(), StatusCode::ResourcesExhausted);
        assert_eq!(registry.slot_count(), 2);
    }

    #[test]
    fn begin_close_moves_live_slot_to_closing_until_completion() {
        let registry = LiveSessionRegistry::new(4);
        let session_id = test_session_id("sess-1");
        registry
            .begin_load(session_id.clone())
            .expect("reserve load");
        registry
            .complete_load_success(&session_id, TestHandle("h1"))
            .expect("complete load");

        let CloseStart::Started { handle, completion } =
            registry.begin_close(&session_id).expect("begin close")
        else {
            panic!("expected started close");
        };

        assert_eq!(handle, TestHandle("h1"));
        assert!(!completion.is_complete());
        assert_eq!(registry.get(&session_id), None);
        assert_eq!(registry.slot_count(), 1);
        let LoadStart::Closing(other_completion) = registry
            .begin_load(session_id.clone())
            .expect("observe closing")
        else {
            panic!("expected closing");
        };
        assert!(!other_completion.is_complete());
        let CloseStart::Closing {
            handle: closing_handle,
            completion: repeated_close,
        } = registry.begin_close(&session_id).expect("observe close")
        else {
            panic!("expected repeated closing");
        };
        assert_eq!(closing_handle, TestHandle("h1"));
        assert!(!repeated_close.is_complete());

        registry
            .complete_close(&session_id)
            .expect("complete close");

        assert!(completion.is_complete());
        assert_eq!(registry.slot_count(), 0);
    }

    #[test]
    fn close_on_loading_reuses_close_signal_and_transitions_to_closing_after_load() {
        let registry = LiveSessionRegistry::new(4);
        let session_id = test_session_id("sess-1");
        registry
            .begin_load(session_id.clone())
            .expect("reserve load");

        let CloseStart::Loading {
            load_completion,
            close_completion,
            should_spawn,
        } = registry.begin_close(&session_id).expect("close loading")
        else {
            panic!("expected loading close");
        };
        assert!(load_completion.ready().is_none());
        assert!(!close_completion.is_complete());
        assert!(should_spawn);

        let CloseStart::Loading {
            close_completion: repeated_close,
            should_spawn: repeated_should_spawn,
            ..
        } = registry
            .begin_close(&session_id)
            .expect("repeat close loading")
        else {
            panic!("expected repeated loading close");
        };
        assert!(!repeated_close.is_complete());
        assert!(!repeated_should_spawn);

        registry
            .complete_load_success(&session_id, TestHandle("h1"))
            .expect("complete load");

        assert_eq!(registry.get(&session_id), None);
        let CloseStart::Closing { handle, completion } =
            registry.begin_close(&session_id).expect("observe closing")
        else {
            panic!("expected closing after load");
        };
        assert_eq!(handle, TestHandle("h1"));
        assert!(!completion.is_complete());
        assert!(!close_completion.is_complete());
        registry
            .complete_close(&session_id)
            .expect("complete close");
        assert!(close_completion.is_complete());
        assert!(repeated_close.is_complete());
    }

    #[test]
    fn close_on_loading_completes_when_load_fails() {
        let registry = LiveSessionRegistry::<TestHandle>::new(4);
        let session_id = test_session_id("sess-1");
        registry
            .begin_load(session_id.clone())
            .expect("reserve load");
        let CloseStart::Loading {
            close_completion, ..
        } = registry.begin_close(&session_id).expect("close loading")
        else {
            panic!("expected loading close");
        };
        let error = NotFoundSnafu {
            session_id: session_id.clone(),
        }
        .build();

        registry
            .complete_load_failure(&session_id, error)
            .expect("complete load failure");

        assert!(close_completion.is_complete());
        assert_eq!(registry.slot_count(), 0);
    }

    #[test]
    fn list_live_returns_only_live_session_ids() {
        let registry = LiveSessionRegistry::new(4);
        let loading = test_session_id("sess-loading");
        let live = test_session_id("sess-live");
        registry.begin_load(loading).expect("reserve loading slot");
        registry
            .begin_load(live.clone())
            .expect("reserve live slot");
        registry
            .complete_load_success(&live, TestHandle("live"))
            .expect("complete live");

        let live_sessions = registry.list_live();

        assert_eq!(live_sessions, vec![live]);
    }

    #[test]
    fn replace_reserves_new_loading_slot_bypassing_max_sessions_by_one() {
        let registry = LiveSessionRegistry::new(1);
        let old_session_id = test_session_id("sess-old");
        let new_session_id = test_session_id("sess-new");
        let blocked_session_id = test_session_id("sess-blocked");
        registry
            .begin_load(old_session_id.clone())
            .expect("reserve old");
        registry
            .complete_load_success(&old_session_id, TestHandle("old"))
            .expect("old live");

        let ReplaceStart::Reserved {
            old_handle,
            new_completion,
        } = registry
            .begin_replace(&old_session_id, new_session_id.clone())
            .expect("begin replace");

        assert_eq!(old_handle, TestHandle("old"));
        assert!(new_completion.ready().is_none());
        assert_eq!(registry.slot_count(), 2);
        assert_eq!(registry.get(&old_session_id), Some(TestHandle("old")));
        let LoadStart::Loading(observed_new) = registry
            .begin_load(new_session_id)
            .expect("observe new loading")
        else {
            panic!("expected new loading");
        };
        assert!(observed_new.ready().is_none());
        assert!(matches!(
            registry.begin_load(blocked_session_id),
            Err(RegistryError::ResourceExhausted { .. })
        ));
    }

    #[test]
    fn replace_construct_failure_removes_new_and_keeps_old_live() {
        let registry = LiveSessionRegistry::new(1);
        let old_session_id = test_session_id("sess-old");
        let new_session_id = test_session_id("sess-new");
        registry
            .begin_load(old_session_id.clone())
            .expect("reserve old");
        registry
            .complete_load_success(&old_session_id, TestHandle("old"))
            .expect("old live");
        let ReplaceStart::Reserved { new_completion, .. } = registry
            .begin_replace(&old_session_id, new_session_id.clone())
            .expect("begin replace");
        let error = NotFoundSnafu {
            session_id: new_session_id.clone(),
        }
        .build();

        registry
            .complete_replace_failure(&new_session_id, error)
            .expect("replace failure");

        let Err(ready_error) = new_completion.ready().expect("new completion ready") else {
            panic!("expected construction error");
        };
        assert!(matches!(ready_error, RegistryError::NotFound { .. }));
        assert_eq!(registry.get(&old_session_id), Some(TestHandle("old")));
        assert_eq!(registry.slot_count(), 1);
    }

    #[test]
    fn replace_commit_promotes_new_and_moves_old_to_closing() {
        let registry = LiveSessionRegistry::new(1);
        let old_session_id = test_session_id("sess-old");
        let new_session_id = test_session_id("sess-new");
        registry
            .begin_load(old_session_id.clone())
            .expect("reserve old");
        registry
            .complete_load_success(&old_session_id, TestHandle("old"))
            .expect("old live");
        let ReplaceStart::Reserved { new_completion, .. } = registry
            .begin_replace(&old_session_id, new_session_id.clone())
            .expect("begin replace");

        let commit = registry
            .complete_replace_success(&old_session_id, &new_session_id, TestHandle("new"))
            .expect("commit replace");

        assert_eq!(commit.old_handle, TestHandle("old"));
        assert!(!commit.old_close_completion.is_complete());
        assert_eq!(
            new_completion
                .ready()
                .expect("new completion ready")
                .expect("new construction ok"),
            TestHandle("new")
        );
        assert_eq!(registry.get(&old_session_id), None);
        assert_eq!(registry.get(&new_session_id), Some(TestHandle("new")));
        assert_eq!(registry.slot_count(), 2);
        let LoadStart::Closing(old_close_completion) = registry
            .begin_load(old_session_id.clone())
            .expect("observe old closing")
        else {
            panic!("expected old closing");
        };
        assert!(!old_close_completion.is_complete());

        registry
            .complete_close(&old_session_id)
            .expect("finish old close");

        assert!(commit.old_close_completion.is_complete());
        assert_eq!(registry.slot_count(), 1);
        assert_eq!(registry.get(&new_session_id), Some(TestHandle("new")));
    }

    #[test]
    fn replace_requires_live_old_and_unused_new_slot() {
        let registry = LiveSessionRegistry::new(4);
        let old_session_id = test_session_id("sess-old");
        let new_session_id = test_session_id("sess-new");
        registry
            .begin_load(old_session_id.clone())
            .expect("old loading");

        let err = registry
            .begin_replace(&old_session_id, new_session_id.clone())
            .expect_err("old must be live");
        assert!(matches!(err, RegistryError::OldNotReady { .. }));

        registry
            .complete_load_success(&old_session_id, TestHandle("old"))
            .expect("old live");
        registry
            .begin_load(new_session_id.clone())
            .expect("new occupied");
        let err = registry
            .begin_replace(&old_session_id, new_session_id)
            .expect_err("new slot must be unused");
        assert!(matches!(err, RegistryError::NewSlotOccupied { .. }));
    }
}
