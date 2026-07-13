pub(super) async fn emit_provider_statuses_refresh(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let statuses = coco_agent_host::session_dialogs::build_provider_status_payload(session);
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::ProviderStatusesRefreshed {
            statuses,
        }))
        .await;
}

pub(super) async fn dispatch_provider_login(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) -> SlashOutcome {
    let provider = slash_provider_arg(args);
    let tx = event_tx.clone();
    let url_sink: std::sync::Arc<dyn Fn(String) + Send + Sync> = std::sync::Arc::new(move |url| {
        let _ = tx.try_send(CoreEvent::Tui(TuiOnlyEvent::SlashCommandResult {
            name: "login".to_string(),
            args: String::new(),
            text: format!("Opening your browser to sign in. If it doesn't open, visit:\n{url}"),
        }));
    });
    match coco_agent_host::provider_login::run_login_for_session(session, provider, url_sink).await
    {
        Ok(msg) => {
            emit_slash_text(event_tx, "login", args, &msg).await;
            emit_provider_statuses_refresh(session, event_tx).await;
            // Best-effort: discover the provider's live model list so
            // subscription-only models surface in `/model` without a restart.
            let instance =
                coco_agent_host::provider_login::instance_name(slash_provider_arg(args).as_deref());
            let base = coco_agent_host::session_dialogs::build_model_catalog_payload(session);
            coco_agent_host::openai_model_refresh::spawn_after_login(
                session.clone(),
                instance,
                event_tx.clone(),
                base,
            );
        }
        Err(e) => {
            emit_slash_status(
                event_tx,
                "login",
                args,
                SlashCommandStatusKind::Failed {
                    error: e.to_string(),
                },
            )
            .await;
        }
    }
    SlashOutcome::Handled
}

/// `/logout [provider]` — clears the subscription credential on the shared
/// `AuthService` (best-effort server-side revocation included).
pub(super) async fn dispatch_provider_logout(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) -> SlashOutcome {
    let provider = slash_provider_arg(args);
    match coco_agent_host::provider_login::run_logout_session(provider).await {
        Ok(msg) => {
            emit_slash_text(event_tx, "logout", args, &msg).await;
            emit_provider_statuses_refresh(session, event_tx).await;
        }
        Err(e) => {
            emit_slash_status(
                event_tx,
                "logout",
                args,
                SlashCommandStatusKind::Failed {
                    error: e.to_string(),
                },
            )
            .await;
        }
    }
    SlashOutcome::Handled
}

/// Emit a `TuiOnlyEvent::SlashCommandStatus` so the TUI renders a
/// localized dispatcher breadcrumb (handler missing, handler error,
/// empty Prompt body, dialog wiring pending).
pub(super) async fn emit_slash_status(
    event_tx: &mpsc::Sender<CoreEvent>,
    name: &str,
    args: &str,
    kind: SlashCommandStatusKind,
) {
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::SlashCommandStatus {
            name: name.to_string(),
            args: args.to_string(),
            kind,
        }))
        .await;
}

/// One-shot, fire-and-forget title generation. Returns immediately
/// without spawning if any precondition (auto-title disabled, already
/// attempted for this session id, no Fast spec, plan not exited,
use coco_query::CoreEvent;
use coco_types::SlashCommandStatusKind;
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc;

use super::SlashOutcome;
use super::emit_slash_text;
use super::slash_provider_arg;
