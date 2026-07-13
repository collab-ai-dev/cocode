use crate::session_runtime::SessionHandle;

pub(crate) fn build_session_result(
    session: &SessionHandle,
    default_stop_reason: &str,
) -> coco_types::SessionResultParams {
    let crate::session_runtime::SessionAccounting { started_at, stats } =
        session.session_accounting_snapshot();
    coco_types::SessionResultParams {
        session_id: session.session_id().clone(),
        total_turns: stats.total_turns,
        duration_ms: started_at.elapsed().as_millis() as i64,
        duration_api_ms: stats.total_duration_api_ms,
        is_error: stats.had_error,
        stop_reason: stats
            .last_stop_reason
            .clone()
            .unwrap_or_else(|| default_stop_reason.into()),
        total_cost_usd: stats.total_cost_usd,
        usage: stats.usage,
        model_usage: stats.model_usage.clone(),
        permission_denials: stats.permission_denials.clone(),
        result: stats.last_result_text.clone(),
        errors: stats.errors.clone(),
        structured_output: if stats.had_error {
            None
        } else {
            stats.structured_output.clone()
        },
        fast_mode_state: None,
        num_api_calls: if stats.num_api_calls > 0 {
            Some(stats.num_api_calls)
        } else {
            None
        },
    }
}
