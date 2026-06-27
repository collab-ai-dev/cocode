use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use coco_config::RuntimeConfig;
use coco_config::ShellToolSelection;
use coco_types::ActiveShellTool;
use coco_types::ModelShellToolType;

pub(crate) fn active_shell_tool_from_runtime(
    runtime_config: &RuntimeConfig,
) -> Result<ActiveShellTool> {
    let model_shell_tool_types = model_shell_tool_types_from_runtime(runtime_config);

    select_active_shell_tool(
        model_shell_tool_types,
        runtime_config.shell.tool,
        cfg!(windows),
        |shell_type| coco_shell::get_shell(shell_type, None).is_some(),
    )
}

pub(crate) fn select_active_shell_tool(
    model_shell_tool_types: impl IntoIterator<Item = ModelShellToolType>,
    setting: ShellToolSelection,
    is_windows: bool,
    shell_available: impl Fn(coco_shell::ShellType) -> bool,
) -> Result<ActiveShellTool> {
    let model_shell_tool_types = model_shell_tool_types.into_iter().collect::<Vec<_>>();
    if model_shell_tool_types.contains(&ModelShellToolType::UnifiedExec) {
        // The model-facing unified_exec protocol is not implemented in
        // coco-rs yet. Fail at session bootstrap instead of silently
        // exposing a mismatched shell tool schema.
        bail!("model shell_tool_type=unified_exec is not implemented yet")
    }
    if model_shell_tool_types.contains(&ModelShellToolType::Disabled) {
        return Ok(ActiveShellTool::Disabled);
    }

    let active = match setting {
        ShellToolSelection::Auto => {
            if is_windows {
                ActiveShellTool::PowerShell
            } else {
                ActiveShellTool::Bash
            }
        }
        ShellToolSelection::Bash => ActiveShellTool::Bash,
        ShellToolSelection::PowerShell => ActiveShellTool::PowerShell,
        ShellToolSelection::Disabled => ActiveShellTool::Disabled,
    };

    match active {
        ActiveShellTool::Bash => {
            if !shell_available(coco_shell::ShellType::Bash) {
                bail!("settings.shell.tool requested bash, but `bash` was not found")
            }
        }
        ActiveShellTool::PowerShell => {
            if !shell_available(coco_shell::ShellType::PowerShell) {
                bail!(
                    "settings.shell.tool requested powershell, but neither `pwsh` nor `powershell` was found"
                )
            }
        }
        ActiveShellTool::Disabled => {}
    }

    Ok(active)
}

fn model_shell_tool_types_from_runtime(runtime_config: &RuntimeConfig) -> Vec<ModelShellToolType> {
    let mut types = Vec::new();
    for slots in runtime_config.model_roles.roles.values() {
        for spec in std::iter::once(&slots.primary).chain(slots.fallbacks.iter()) {
            let shell_tool_type = runtime_config
                .model_registry
                .resolve(&spec.provider, &spec.model_id)
                .map(|resolved| resolved.info.shell_tool_type)
                .unwrap_or_default();
            types.push(shell_tool_type);
        }
    }
    if types.is_empty() {
        types.push(ModelShellToolType::default());
    }
    types
}

pub(crate) fn require_shell(
    shell_type: coco_shell::ShellType,
    message: &'static str,
) -> Result<coco_shell::Shell> {
    coco_shell::get_shell(shell_type, None).with_context(|| message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available(shell: coco_shell::ShellType) -> bool {
        matches!(
            shell,
            coco_shell::ShellType::Bash | coco_shell::ShellType::PowerShell
        )
    }

    #[test]
    fn auto_selects_platform_default() {
        assert_eq!(
            select_active_shell_tool(
                [ModelShellToolType::ShellCommand],
                ShellToolSelection::Auto,
                false,
                available,
            )
            .unwrap(),
            ActiveShellTool::Bash
        );
        assert_eq!(
            select_active_shell_tool(
                [ModelShellToolType::ShellCommand],
                ShellToolSelection::Auto,
                true,
                available,
            )
            .unwrap(),
            ActiveShellTool::PowerShell
        );
    }

    #[test]
    fn explicit_selection_overrides_platform_default() {
        assert_eq!(
            select_active_shell_tool(
                [ModelShellToolType::ShellCommand],
                ShellToolSelection::PowerShell,
                false,
                available,
            )
            .unwrap(),
            ActiveShellTool::PowerShell
        );
        assert_eq!(
            select_active_shell_tool(
                [ModelShellToolType::ShellCommand],
                ShellToolSelection::Bash,
                true,
                available,
            )
            .unwrap(),
            ActiveShellTool::Bash
        );
    }

    #[test]
    fn unavailable_shell_fails_fast() {
        let err = select_active_shell_tool(
            [ModelShellToolType::ShellCommand],
            ShellToolSelection::PowerShell,
            false,
            |_| false,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("powershell"));
    }

    #[test]
    fn model_disabled_wins_over_shell_setting_and_availability() {
        assert_eq!(
            select_active_shell_tool(
                [ModelShellToolType::Disabled],
                ShellToolSelection::PowerShell,
                false,
                |_| false,
            )
            .unwrap(),
            ActiveShellTool::Disabled
        );
    }

    #[test]
    fn unified_exec_is_explicitly_unimplemented() {
        let err = select_active_shell_tool(
            [ModelShellToolType::UnifiedExec],
            ShellToolSelection::Auto,
            false,
            available,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unified_exec"));
        assert!(err.contains("not implemented"));
    }

    #[test]
    fn disabled_any_role_disables_shell_tools() {
        assert_eq!(
            select_active_shell_tool(
                [
                    ModelShellToolType::ShellCommand,
                    ModelShellToolType::Disabled,
                ],
                ShellToolSelection::Bash,
                false,
                available,
            )
            .unwrap(),
            ActiveShellTool::Disabled
        );
    }

    #[test]
    fn unified_exec_any_role_fails_before_disabled_wins() {
        let err = select_active_shell_tool(
            [
                ModelShellToolType::Disabled,
                ModelShellToolType::UnifiedExec,
            ],
            ShellToolSelection::Auto,
            false,
            available,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unified_exec"));
    }
}
