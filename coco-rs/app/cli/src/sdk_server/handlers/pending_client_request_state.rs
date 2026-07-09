use coco_types::ApprovalResolveParams;
use coco_types::ElicitationResolveParams;
use coco_types::UserInputResolveParams;
use tokio::sync::oneshot;

use crate::sdk_server::pending_map::PendingMap;
use crate::sdk_server::pending_map::ResolveOutcome;

#[derive(Default)]
pub(super) struct PendingClientRequestState {
    approvals: PendingMap<ApprovalResolveParams>,
    user_inputs: PendingMap<UserInputResolveParams>,
    elicitations: PendingMap<ElicitationResolveParams>,
}

impl PendingClientRequestState {
    pub(super) async fn register_approval(
        &self,
        request_id: String,
    ) -> oneshot::Receiver<ApprovalResolveParams> {
        self.approvals.register(request_id).await
    }

    pub(super) async fn register_user_input(
        &self,
        request_id: String,
    ) -> oneshot::Receiver<UserInputResolveParams> {
        self.user_inputs.register(request_id).await
    }

    pub(super) async fn register_elicitation(
        &self,
        request_id: String,
    ) -> oneshot::Receiver<ElicitationResolveParams> {
        self.elicitations.register(request_id).await
    }

    pub(super) async fn resolve_approval(
        &self,
        request_id: &str,
        params: ApprovalResolveParams,
    ) -> ResolveOutcome {
        self.approvals.resolve(request_id, params).await
    }

    pub(super) async fn resolve_user_input(
        &self,
        request_id: &str,
        params: UserInputResolveParams,
    ) -> ResolveOutcome {
        self.user_inputs.resolve(request_id, params).await
    }

    pub(super) async fn resolve_elicitation(
        &self,
        request_id: &str,
        params: ElicitationResolveParams,
    ) -> ResolveOutcome {
        self.elicitations.resolve(request_id, params).await
    }

    pub(super) async fn cancel(&self, request_id: &str) -> Option<&'static str> {
        if self.approvals.remove(request_id).await {
            Some("approval")
        } else if self.user_inputs.remove(request_id).await {
            Some("user_input")
        } else if self.elicitations.remove(request_id).await {
            Some("elicitation")
        } else {
            None
        }
    }
}
