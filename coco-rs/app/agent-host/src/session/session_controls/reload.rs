use crate::session_runtime::SessionHandle;

use super::SessionControlError;

pub async fn reload_plugins(runtime: Option<SessionHandle>) -> coco_types::PluginReloadResult {
    let Some(runtime) = runtime else {
        return coco_types::PluginReloadResult {
            plugins: Vec::new(),
            commands: Vec::new(),
            agents: Vec::new(),
            error_count: 0,
        };
    };
    let report = runtime.reload_plugin_environment().await;
    coco_types::PluginReloadResult {
        plugins: report.plugins,
        commands: report.commands,
        agents: report.agents,
        error_count: i32::try_from(report.hook_error_count).unwrap_or(i32::MAX),
    }
}

pub async fn reload_hooks(
    runtime: Option<SessionHandle>,
) -> Result<coco_types::HookReloadResult, SessionControlError> {
    let Some(runtime) = runtime else {
        return Ok(coco_types::HookReloadResult { hook_count: 0 });
    };
    runtime
        .reload_hooks()
        .await
        .map(|hook_count| coco_types::HookReloadResult {
            hook_count: i64::try_from(hook_count).unwrap_or(i64::MAX),
        })
        .map_err(|error| SessionControlError::HookReload(error.to_string()))
}
