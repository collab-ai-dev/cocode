//! `coco-goal-runtime` — the host orchestration layer over the pure
//! [`coco_goals`] domain.
//!
//! This crate turns reducer decisions into durable, observable session state. It
//! owns:
//!
//! * [`GoalRuntimeHandle`] — the session-local transaction boundary that
//!   serializes mutations, commits durably before publishing, and returns typed
//!   effects (§10.2);
//! * [`GoalStore`] — the narrow persistence seam (concrete session-backed impl
//!   lives in the session runtime, which holds the write lease).
//!
//! Later phases add the completion coordinator/gate, context materializer, and
//! supervisor on top of this boundary.

mod admission;
mod coordinator;
mod error;
mod evidence;
mod gate;
mod handle;
mod materializer;
mod port;
mod store;
mod supervisor;
mod verifier;

#[cfg(test)]
mod test_support;

pub use admission::{AdmissionPermit, AutonomousAdmission};
pub use coordinator::{CoordinatorOutcome, GoalCompletionCoordinator, GoalTurnResult};
pub use error::{GoalRuntimeError, Result};
pub use evidence::{EvidenceStore, InMemoryEvidenceStore};
pub use gate::GoalCompletionGate;
pub use handle::{AppliedGoalDecision, GoalRuntimeHandle};
pub use materializer::{
    GoalBudgetView, GoalContextMaterializer, GoalPlanView, GoalTurnContext, NoPlanSource,
    PlanSource,
};
pub use port::{
    GoalTurnCompletion, GoalTurnHandle, GoalTurnOutcome, GoalTurnRequest, ProviderErrorKind,
    SessionTurnPort,
};
pub use store::{GoalStore, InMemoryGoalStore};
pub use supervisor::{AdvanceOutcome, GoalSupervisor};
pub use verifier::{
    AlwaysRejected, AlwaysUnavailable, AlwaysVerified, CompletionVerifier, VerificationRequest,
};

// Re-export the domain surface most host callers need, so they depend on one
// crate at the goal seam.
pub use coco_goals;
