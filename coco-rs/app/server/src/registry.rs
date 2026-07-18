use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use coco_types::SessionId;
use snafu::Snafu;

use crate::registration_policy::{SessionRegistrationPolicy, SessionTopology};

type LoadResult<H> = Option<Result<H, RegistryError>>;
type CloseResult = Option<Result<(), RegistryError>>;

/// Registry storage guarded by a single lock: the lifecycle slots plus their
/// immutable registration policies and the parent→child sidechat index. Keeping
/// all three under one lock lets child reservation and its "one child per
/// parent" guarantee commit atomically with slot transitions.
pub(crate) struct RegistryInner<H> {
    pub(crate) slots: HashMap<SessionId, SessionSlot<H>>,
    pub(crate) policies: HashMap<SessionId, SessionRegistrationPolicy>,
    /// parent session id → its single live/loading/closing child.
    pub(crate) children: HashMap<SessionId, SessionId>,
    /// Parents whose close/replace transaction has begun. Child admission is
    /// closed until the parent is removed or a failed replace rolls back.
    pub(crate) blocked_parents: HashSet<SessionId>,
}

impl<H> RegistryInner<H> {
    fn new() -> Self {
        Self {
            slots: HashMap::new(),
            policies: HashMap::new(),
            children: HashMap::new(),
            blocked_parents: HashSet::new(),
        }
    }

    /// Retire a slot key entirely: drop the slot, its policy, and (if it was a
    /// child) its parent→child index entry. Returns the removed slot, if any.
    /// Only for terminal removal — transitions that re-insert the same key
    /// (`promote`) must keep the policy and index intact.
    pub(crate) fn forget(&mut self, session_id: &SessionId) -> Option<SessionSlot<H>> {
        let removed = self.slots.remove(session_id);
        self.blocked_parents.remove(session_id);
        if let Some(policy) = self.policies.remove(session_id)
            && let SessionTopology::Child { parent } = &policy.topology
            && self.children.get(parent) == Some(session_id)
        {
            self.children.remove(parent);
        }
        removed
    }
}

/// Registry for session lifecycle slots (root plus at most one sidechat child
/// per parent).
///
/// The registry owns only slot state, registration policy, the parent→child
/// index, and completion signals. Runtime construction, close cascade, and
/// owner-task spawning are wired by AppServer.
pub struct LiveSessionRegistry<H> {
    pub(crate) sessions: RwLock<RegistryInner<H>>,
    max_sessions: usize,
}

impl<H: Clone> LiveSessionRegistry<H> {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            sessions: RwLock::new(RegistryInner::new()),
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
            .slots
            .len()
    }

    pub fn live_count(&self) -> usize {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .slots
            .values()
            .filter(|slot| matches!(slot, SessionSlot::Live(_)))
            .count()
    }

    pub fn get(&self, session_id: &SessionId) -> Option<H> {
        match self
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .slots
            .get(session_id)
        {
            Some(SessionSlot::Live(handle)) => Some(handle.clone()),
            _ => None,
        }
    }

    /// The registration policy for a slot in any lifecycle state, if present.
    pub fn policy(&self, session_id: &SessionId) -> Option<SessionRegistrationPolicy> {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .policies
            .get(session_id)
            .cloned()
    }

    /// The child sidechat currently associated with `parent`, if one is
    /// loading, live, or closing.
    pub fn child_of(&self, parent: &SessionId) -> Option<SessionId> {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .children
            .get(parent)
            .cloned()
    }

    /// True when public/remote session-data APIs may observe the slot. Absent
    /// slots and slots whose policy is `Internal` are not public. Slots without
    /// a recorded policy default to public (legacy root behavior).
    pub fn is_public(&self, session_id: &SessionId) -> bool {
        let inner = self
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.slots.contains_key(session_id)
            && inner
                .policies
                .get(session_id)
                .is_none_or(|policy| !policy.is_internal())
    }

    pub fn replace_live_handle(
        &self,
        session_id: &SessionId,
        handle: H,
    ) -> Result<H, RegistryError> {
        let mut inner = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(slot @ SessionSlot::Live(_)) = inner.slots.get_mut(session_id) else {
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

    /// Session ids that must stay announced to process egress: Live plus
    /// retiring Closing slots whose egress is durable. A Closing session's final
    /// `SessionResult` may still be in flight, so it must remain in Hub
    /// membership until its slot is removed by the completed close cascade
    /// (CS-4 / R17). Loading slots are not yet announced. `LocalOnly` slots
    /// (sidechat children) never announce — their events never reach the Hub.
    pub fn list_announced(&self) -> Vec<SessionId> {
        let inner = self
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .slots
            .iter()
            .filter(|&(_, slot)| matches!(slot, SessionSlot::Live(_) | SessionSlot::Closing(_)))
            .filter(|&(id, _)| {
                inner
                    .policies
                    .get(id)
                    .is_none_or(|policy| !policy.is_local_only())
            })
            .map(|(session_id, _)| session_id.clone())
            .collect()
    }

    pub fn list_live(&self) -> Vec<SessionId> {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .slots
            .iter()
            .filter(|&(_, slot)| matches!(slot, SessionSlot::Live(_)))
            .map(|(session_id, _)| session_id.clone())
            .collect()
    }

    /// Live session ids that are publicly visible — the projection behind
    /// `session/list`. Excludes `Internal` slots (sidechat children) so they
    /// never appear in a public catalog.
    pub fn list_public_live(&self) -> Vec<SessionId> {
        let inner = self
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .slots
            .iter()
            .filter(|&(_, slot)| matches!(slot, SessionSlot::Live(_)))
            .filter(|&(id, _)| {
                inner
                    .policies
                    .get(id)
                    .is_none_or(|policy| !policy.is_internal())
            })
            .map(|(session_id, _)| session_id.clone())
            .collect()
    }

    pub fn list_closable(&self) -> Vec<SessionId> {
        let inner = self
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .slots
            .keys()
            .filter(|session_id| {
                inner
                    .policies
                    .get(*session_id)
                    .and_then(SessionRegistrationPolicy::parent)
                    .is_none_or(|parent| !inner.slots.contains_key(parent))
            })
            .cloned()
            .collect()
    }

    pub fn begin_load(&self, session_id: SessionId) -> Result<LoadStart<H>, RegistryError> {
        let mut inner = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match inner.slots.get(&session_id) {
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
        if inner.slots.len() >= self.max_sessions {
            return ResourceExhaustedSnafu.fail();
        }

        inner
            .slots
            .insert(session_id.clone(), SessionSlot::Loading(LoadState::new()));
        inner
            .policies
            .insert(session_id, SessionRegistrationPolicy::root());
        Ok(LoadStart::Reserved)
    }

    /// Reserve a `Loading` slot for a sidechat `child` of a live `parent`, under
    /// one locked transaction. Enforces the "at most one child per parent"
    /// invariant (I-2) and stamps the child's `Child/Internal/LocalOnly` policy
    /// and the parent→child index atomically with the slot insert.
    pub fn begin_child_load(
        &self,
        parent: &SessionId,
        child: SessionId,
    ) -> Result<LoadStart<H>, RegistryError> {
        let mut inner = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The parent must be live to own a child.
        if !matches!(inner.slots.get(parent), Some(SessionSlot::Live(_))) {
            return OldNotReadySnafu {
                session_id: parent.clone(),
            }
            .fail();
        }
        // At most one loading/live/closing child per parent.
        if inner.blocked_parents.contains(parent) {
            return OldNotReadySnafu {
                session_id: parent.clone(),
            }
            .fail();
        }
        if inner.children.contains_key(parent) {
            return ChildExistsSnafu {
                session_id: parent.clone(),
            }
            .fail();
        }
        // The child id must be unused.
        if inner.slots.contains_key(&child) {
            return NewSlotOccupiedSnafu { session_id: child }.fail();
        }
        if inner.slots.len() >= self.max_sessions {
            return ResourceExhaustedSnafu.fail();
        }

        inner
            .slots
            .insert(child.clone(), SessionSlot::Loading(LoadState::new()));
        inner.policies.insert(
            child.clone(),
            SessionRegistrationPolicy::side_chat_child(parent.clone()),
        );
        inner.children.insert(parent.clone(), child);
        Ok(LoadStart::Reserved)
    }

    pub fn complete_load_success(
        &self,
        session_id: &SessionId,
        handle: H,
    ) -> Result<(), RegistryError> {
        let mut inner = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(parent) = inner
            .policies
            .get(session_id)
            .and_then(SessionRegistrationPolicy::parent)
            .cloned()
        {
            let parent_accepts_child =
                matches!(inner.slots.get(&parent), Some(SessionSlot::Live(_)))
                    && !inner.blocked_parents.contains(&parent);
            let close_was_recorded = matches!(
                inner.slots.get(session_id),
                Some(SessionSlot::Loading(LoadState {
                    close_after_load: Some(_),
                    ..
                }))
            );
            if !parent_accepts_child && !close_was_recorded {
                return OldNotReadySnafu { session_id: parent }.fail();
            }
        }
        // Same key is re-inserted by `promote`; policy/index stay intact.
        let Some(SessionSlot::Loading(load)) = inner.slots.remove(session_id) else {
            return SlotConflictSnafu {
                session_id: session_id.clone(),
                expected: "Loading",
            }
            .fail();
        };
        inner.slots.insert(session_id.clone(), load.promote(handle));
        Ok(())
    }

    pub fn complete_load_failure(
        &self,
        session_id: &SessionId,
        error: RegistryError,
    ) -> Result<(), RegistryError> {
        let mut inner = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(inner.slots.get(session_id), Some(SessionSlot::Loading(_))) {
            return SlotConflictSnafu {
                session_id: session_id.clone(),
                expected: "Loading",
            }
            .fail();
        }
        let Some(SessionSlot::Loading(load)) = inner.forget(session_id) else {
            unreachable!("slot was matched as Loading above");
        };
        let _ = load.sender.send(Some(Err(error)));
        if let Some(close) = load.close_after_load {
            let _ = close.sender.send(Some(Ok(())));
        }
        Ok(())
    }

    pub fn begin_close(&self, session_id: &SessionId) -> Result<CloseStart<H>, RegistryError> {
        let mut inner = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        begin_close_locked(&mut inner, session_id)
    }

    /// Atomically close child admission and transition the owned child before
    /// transitioning `session_id` itself.
    pub fn begin_close_cascade(
        &self,
        session_id: &SessionId,
    ) -> Result<CloseCascadeStart<H>, RegistryError> {
        let mut inner = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inner.slots.contains_key(session_id) {
            return NotFoundSnafu {
                session_id: session_id.clone(),
            }
            .fail();
        }
        let child = inner
            .children
            .get(session_id)
            .cloned()
            .map(|child_id| {
                begin_close_locked(&mut inner, &child_id).map(|start| (child_id, start))
            })
            .transpose()?;
        let parent = begin_close_locked(&mut inner, session_id)?;
        inner.blocked_parents.insert(session_id.clone());
        Ok(CloseCascadeStart { child, parent })
    }

    pub fn unblock_child_admission(&self, session_id: &SessionId) {
        self.sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .blocked_parents
            .remove(session_id);
    }

    /// Block new child admission and transition the current child, if any,
    /// before a parent replacement that does not reserve a new slot.
    pub fn begin_parent_transition(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<(SessionId, CloseStart<H>)>, RegistryError> {
        let mut inner = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(inner.slots.get(session_id), Some(SessionSlot::Live(_))) {
            return OldNotReadySnafu {
                session_id: session_id.clone(),
            }
            .fail();
        }
        let child = inner
            .children
            .get(session_id)
            .cloned()
            .map(|child_id| {
                begin_close_locked(&mut inner, &child_id).map(|start| (child_id, start))
            })
            .transpose()?;
        inner.blocked_parents.insert(session_id.clone());
        Ok(child)
    }

    pub fn begin_replace(
        &self,
        old_session_id: &SessionId,
        new_session_id: SessionId,
    ) -> Result<ReplaceStart<H>, RegistryError> {
        let mut inner = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old_handle = match inner.slots.get(old_session_id) {
            Some(SessionSlot::Live(old_handle)) => old_handle.clone(),
            _ => {
                return OldNotReadySnafu {
                    session_id: old_session_id.clone(),
                }
                .fail();
            }
        };
        if inner.slots.contains_key(&new_session_id) {
            return NewSlotOccupiedSnafu {
                session_id: new_session_id,
            }
            .fail();
        }

        let child = inner
            .children
            .get(old_session_id)
            .cloned()
            .map(|child_id| {
                begin_close_locked(&mut inner, &child_id).map(|start| (child_id, start))
            })
            .transpose()?;
        inner.blocked_parents.insert(old_session_id.clone());

        let new_load = LoadState::new();
        let new_completion = new_load.completion();
        let inherited = inner
            .policies
            .get(old_session_id)
            .cloned()
            .unwrap_or_else(SessionRegistrationPolicy::root);
        inner
            .slots
            .insert(new_session_id.clone(), SessionSlot::Loading(new_load));
        inner.policies.insert(new_session_id, inherited);
        Ok(ReplaceStart::Reserved {
            old_handle,
            new_completion,
            child,
        })
    }

    pub fn complete_replace_success(
        &self,
        old_session_id: &SessionId,
        new_session_id: &SessionId,
        new_handle: H,
    ) -> Result<ReplaceCommit<H>, RegistryError> {
        let mut inner = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Validate both slots before mutating either (no await between).
        let old_handle = match inner.slots.get(old_session_id) {
            Some(SessionSlot::Live(handle)) => handle.clone(),
            _ => {
                return OldNotReadySnafu {
                    session_id: old_session_id.clone(),
                }
                .fail();
            }
        };
        if !matches!(
            inner.slots.get(new_session_id),
            Some(SessionSlot::Loading(_))
        ) {
            return SlotConflictSnafu {
                session_id: new_session_id.clone(),
                expected: "Loading",
            }
            .fail();
        }

        let old_close = CloseState::new();
        let old_close_completion = old_close.completion();
        // Consume the new reservation so `promote` can honor a close-after-load
        // recorded on it instead of a blind `Live` insert. Same keys are
        // re-inserted, so policy/index bookkeeping is unchanged.
        let Some(SessionSlot::Loading(new_load)) = inner.slots.remove(new_session_id) else {
            unreachable!("new slot was matched as Loading above");
        };
        inner
            .slots
            .insert(new_session_id.clone(), new_load.promote(new_handle));
        inner.slots.insert(
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
        old_session_id: &SessionId,
        new_session_id: &SessionId,
        error: RegistryError,
    ) -> Result<(), RegistryError> {
        self.complete_load_failure(new_session_id, error)?;
        self.unblock_child_admission(old_session_id);
        Ok(())
    }

    pub fn complete_close(&self, session_id: &SessionId) -> Result<(), RegistryError> {
        self.complete_close_with_result(session_id, Ok(()))
    }

    pub(crate) fn complete_close_with_result(
        &self,
        session_id: &SessionId,
        close_result: Result<(), RegistryError>,
    ) -> Result<(), RegistryError> {
        let mut inner = self
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(SessionSlot::Closing(closing)) = inner.slots.get(session_id) else {
            return SlotConflictSnafu {
                session_id: session_id.clone(),
                expected: "Closing",
            }
            .fail();
        };
        let _ = closing.close.sender.send(Some(close_result));
        inner.forget(session_id);
        Ok(())
    }
}

pub(crate) fn begin_close_locked<H: Clone>(
    inner: &mut RegistryInner<H>,
    session_id: &SessionId,
) -> Result<CloseStart<H>, RegistryError> {
    match inner.slots.get_mut(session_id) {
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
            inner.slots.insert(
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

    /// Fire the load-completion signal with `handle` and produce the next
    /// slot. A `close_after_load` recorded while loading (process shutdown, or a
    /// close racing a replace reservation) is honored here, so the slot
    /// moves straight to `Closing` and the waiting close owner task finds it,
    /// instead of a blind `Live` insert dropping the close request.
    pub(crate) fn promote(mut self, handle: H) -> SessionSlot<H> {
        let _ = self.sender.send(Some(Ok(handle.clone())));
        match self.close_after_load.take() {
            Some(close) => SessionSlot::Closing(ClosingState { handle, close }),
            None => SessionSlot::Live(handle),
        }
    }
}

pub(crate) struct CloseState {
    pub(crate) sender: tokio::sync::watch::Sender<CloseResult>,
    receiver: tokio::sync::watch::Receiver<CloseResult>,
}

impl CloseState {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = tokio::sync::watch::channel(None);
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
pub struct CloseCascadeStart<H> {
    pub child: Option<(SessionId, CloseStart<H>)>,
    pub parent: CloseStart<H>,
}

#[derive(Debug, Clone)]
pub enum ReplaceStart<H> {
    Reserved {
        old_handle: H,
        new_completion: LoadCompletion<H>,
        child: Option<(SessionId, CloseStart<H>)>,
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
    receiver: tokio::sync::watch::Receiver<CloseResult>,
}

impl CloseCompletion {
    pub fn is_complete(&self) -> bool {
        self.receiver.borrow().is_some()
    }

    pub fn ready(&self) -> Option<Result<(), RegistryError>> {
        self.receiver.borrow().clone()
    }

    pub async fn wait(&mut self) -> Result<(), RegistryError> {
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
    #[snafu(display("session {session_id} already has a child sidechat"))]
    ChildExists {
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
    #[snafu(display("session close failed: {message}"))]
    CloseFailed {
        message: String,
        data: Option<serde_json::Value>,
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

    pub fn close_failed(message: impl Into<String>) -> Self {
        CloseFailedSnafu {
            message: message.into(),
            data: None,
        }
        .build()
    }

    pub fn close_failed_with_data(
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Self {
        CloseFailedSnafu {
            message: message.into(),
            data,
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
            Self::ChildExists { .. } => StatusCode::InvalidArguments,
            Self::SlotConflict { .. } => StatusCode::InvalidArguments,
            Self::LoadFailed { .. } => StatusCode::Internal,
            Self::CloseFailed { .. } => StatusCode::Internal,
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
