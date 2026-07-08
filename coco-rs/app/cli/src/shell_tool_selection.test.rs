use coco_config::ShellToolSelection;
use coco_types::ActiveShellTool;
use coco_types::ModelShellToolType;

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
