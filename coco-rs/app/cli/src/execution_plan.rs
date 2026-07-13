use std::io::IsTerminal;

use crate::Cli;
use crate::Commands;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoCapabilities {
    pub stdin_is_terminal: bool,
    pub stdout_is_terminal: bool,
}

impl IoCapabilities {
    pub fn detect() -> Self {
        Self {
            stdin_is_terminal: std::io::stdin().is_terminal(),
            stdout_is_terminal: std::io::stdout().is_terminal(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Skip,
    Tui,
    Headless,
    Sdk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionReason {
    ShortCommand,
    InteractiveCommand,
    HeadlessCommand,
    SdkCommand,
    PromptFlag,
    NonInteractiveFlag,
    StdinNotTerminal,
    StdoutNotTerminal,
    DefaultInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionPlan {
    pub mode: ExecutionMode,
    pub reason: ExecutionReason,
    pub io: IoCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionPlanError {
    NoSessionPersistenceRequiresHeadless,
    PlanModeInstructionsRequiresHeadless,
}

impl std::fmt::Display for ExecutionPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSessionPersistenceRequiresHeadless => f.write_str(
                "--no-session-persistence can only be used in print mode (-p / --print) or SDK mode",
            ),
            Self::PlanModeInstructionsRequiresHeadless => {
                f.write_str("--plan-mode-instructions can only be used in print mode (-p / --print)")
            }
        }
    }
}

impl std::error::Error for ExecutionPlanError {}

pub fn build_execution_plan(
    cli: &Cli,
    io: IoCapabilities,
) -> Result<ExecutionPlan, ExecutionPlanError> {
    let plan = classify_execution_plan(cli, io);
    validate_execution_plan(cli, plan)?;
    Ok(plan)
}

pub fn classify_execution_plan(cli: &Cli, io: IoCapabilities) -> ExecutionPlan {
    if let Some(command) = &cli.command {
        let (mode, reason) = match command {
            Commands::Chat { .. } | Commands::Review { .. } => {
                (ExecutionMode::Headless, ExecutionReason::HeadlessCommand)
            }
            Commands::Sdk => (ExecutionMode::Sdk, ExecutionReason::SdkCommand),
            Commands::Resume { .. } => (ExecutionMode::Tui, ExecutionReason::InteractiveCommand),
            Commands::Status
            | Commands::Sessions
            | Commands::Config { .. }
            | Commands::Doctor
            | Commands::Login { .. }
            | Commands::Logout { .. }
            | Commands::Init
            | Commands::Mcp { .. }
            | Commands::Plugin { .. }
            | Commands::Moa { .. }
            | Commands::Agents
            | Commands::AutoMode { .. }
            | Commands::ExecServer { .. }
            | Commands::Ps { .. }
            | Commands::ReleaseNotes => (ExecutionMode::Skip, ExecutionReason::ShortCommand),
        };
        return ExecutionPlan { mode, reason, io };
    }

    let (mode, reason) = if cli.non_interactive {
        (ExecutionMode::Headless, ExecutionReason::NonInteractiveFlag)
    } else if cli.prompt.is_some() {
        (ExecutionMode::Headless, ExecutionReason::PromptFlag)
    } else if !io.stdin_is_terminal {
        (ExecutionMode::Headless, ExecutionReason::StdinNotTerminal)
    } else if !io.stdout_is_terminal {
        (ExecutionMode::Headless, ExecutionReason::StdoutNotTerminal)
    } else {
        (ExecutionMode::Tui, ExecutionReason::DefaultInteractive)
    };
    ExecutionPlan { mode, reason, io }
}

fn validate_execution_plan(cli: &Cli, plan: ExecutionPlan) -> Result<(), ExecutionPlanError> {
    if cli.no_session_persistence && !plan.is_headless_like() {
        return Err(ExecutionPlanError::NoSessionPersistenceRequiresHeadless);
    }
    if cli.plan_mode_instructions.is_some() && !matches!(plan.mode, ExecutionMode::Headless) {
        return Err(ExecutionPlanError::PlanModeInstructionsRequiresHeadless);
    }
    Ok(())
}

impl ExecutionPlan {
    pub fn is_headless_like(self) -> bool {
        matches!(self.mode, ExecutionMode::Headless | ExecutionMode::Sdk)
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use pretty_assertions::assert_eq;

    use super::*;

    fn parse(args: &[&str]) -> Cli {
        let mut full = vec!["coco"];
        full.extend_from_slice(args);
        Cli::parse_from(full)
    }

    fn io(stdin_is_terminal: bool, stdout_is_terminal: bool) -> IoCapabilities {
        IoCapabilities {
            stdin_is_terminal,
            stdout_is_terminal,
        }
    }

    #[test]
    fn non_interactive_selects_headless_without_prompt() {
        let cli = parse(&["--non-interactive"]);
        let plan = build_execution_plan(&cli, io(true, true)).expect("valid plan");

        assert_eq!(plan.mode, ExecutionMode::Headless);
        assert_eq!(plan.reason, ExecutionReason::NonInteractiveFlag);
    }

    #[test]
    fn prompt_selects_headless() {
        let cli = parse(&["--prompt", "hi"]);
        let plan = build_execution_plan(&cli, io(true, true)).expect("valid plan");

        assert_eq!(plan.mode, ExecutionMode::Headless);
        assert_eq!(plan.reason, ExecutionReason::PromptFlag);
    }

    #[test]
    fn non_terminal_stdin_selects_headless() {
        let cli = parse(&[]);
        let plan = build_execution_plan(&cli, io(false, true)).expect("valid plan");

        assert_eq!(plan.mode, ExecutionMode::Headless);
        assert_eq!(plan.reason, ExecutionReason::StdinNotTerminal);
    }

    #[test]
    fn non_terminal_stdout_selects_headless() {
        let cli = parse(&[]);
        let plan = build_execution_plan(&cli, io(true, false)).expect("valid plan");

        assert_eq!(plan.mode, ExecutionMode::Headless);
        assert_eq!(plan.reason, ExecutionReason::StdoutNotTerminal);
    }

    #[test]
    fn no_command_tty_defaults_to_tui() {
        let cli = parse(&[]);
        let plan = build_execution_plan(&cli, io(true, true)).expect("valid plan");

        assert_eq!(plan.mode, ExecutionMode::Tui);
        assert_eq!(plan.reason, ExecutionReason::DefaultInteractive);
    }

    #[test]
    fn resume_subcommand_is_interactive() {
        let cli = parse(&["resume", "abc"]);
        let plan = build_execution_plan(&cli, io(true, true)).expect("valid plan");

        assert_eq!(plan.mode, ExecutionMode::Tui);
        assert_eq!(plan.reason, ExecutionReason::InteractiveCommand);
    }

    #[test]
    fn sdk_subcommand_is_sdk() {
        let cli = parse(&["sdk"]);
        let plan = build_execution_plan(&cli, io(true, true)).expect("valid plan");

        assert_eq!(plan.mode, ExecutionMode::Sdk);
        assert_eq!(plan.reason, ExecutionReason::SdkCommand);
    }

    #[test]
    fn no_session_persistence_rejected_for_tui() {
        let cli = parse(&["--no-session-persistence"]);
        let err = build_execution_plan(&cli, io(true, true)).expect_err("TUI should reject flag");

        assert_eq!(
            err,
            ExecutionPlanError::NoSessionPersistenceRequiresHeadless
        );
    }

    #[test]
    fn no_session_persistence_allowed_for_sdk() {
        let cli = parse(&["--no-session-persistence", "sdk"]);
        let plan = build_execution_plan(&cli, io(true, true)).expect("SDK should allow flag");

        assert_eq!(plan.mode, ExecutionMode::Sdk);
    }

    #[test]
    fn plan_mode_instructions_rejected_for_tui() {
        let cli = parse(&["--plan-mode-instructions", "custom"]);
        let err = build_execution_plan(&cli, io(true, true)).expect_err("TUI should reject flag");

        assert_eq!(
            err,
            ExecutionPlanError::PlanModeInstructionsRequiresHeadless
        );
    }

    #[test]
    fn default_invocation_mode_matrix_is_explicit() {
        for (args, stdin_tty, stdout_tty, expected_mode, expected_reason) in [
            (
                &[][..],
                true,
                true,
                ExecutionMode::Tui,
                ExecutionReason::DefaultInteractive,
            ),
            (
                &[][..],
                false,
                true,
                ExecutionMode::Headless,
                ExecutionReason::StdinNotTerminal,
            ),
            (
                &[][..],
                true,
                false,
                ExecutionMode::Headless,
                ExecutionReason::StdoutNotTerminal,
            ),
            (
                &[][..],
                false,
                false,
                ExecutionMode::Headless,
                ExecutionReason::StdinNotTerminal,
            ),
            (
                &["--prompt", "hi"][..],
                true,
                true,
                ExecutionMode::Headless,
                ExecutionReason::PromptFlag,
            ),
            (
                &["--non-interactive"][..],
                true,
                true,
                ExecutionMode::Headless,
                ExecutionReason::NonInteractiveFlag,
            ),
        ] {
            let cli = parse(args);
            let plan = build_execution_plan(&cli, io(stdin_tty, stdout_tty)).expect("valid plan");

            assert_eq!(plan.mode, expected_mode, "args={args:?}");
            assert_eq!(plan.reason, expected_reason, "args={args:?}");
        }
    }
}
