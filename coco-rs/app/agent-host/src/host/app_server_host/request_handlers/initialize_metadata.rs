use crate::session_runtime::SessionHandle;

pub(super) struct RuntimeInitializeMetadata {
    pub(super) commands: Vec<crate::session_runtime::SessionInitializeCommand>,
    pub(super) agents: Vec<crate::session_runtime::SessionInitializeAgent>,
    pub(super) output_style: String,
    pub(super) available_output_styles: Vec<String>,
}

pub(super) async fn runtime_initialize_metadata(
    runtime: &SessionHandle,
) -> RuntimeInitializeMetadata {
    let snapshot = runtime.initialize_metadata_snapshot().await;
    RuntimeInitializeMetadata {
        commands: snapshot.commands,
        agents: snapshot.agents,
        output_style: snapshot.output_style,
        available_output_styles: snapshot.available_output_styles,
    }
}

pub(super) async fn runtime_fast_mode_state(runtime: &SessionHandle) -> coco_types::FastModeState {
    runtime.fast_mode_state().await
}
