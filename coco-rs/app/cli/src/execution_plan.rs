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

impl Commands {
    /// The mode this subcommand selects, independent of IO capabilities and
    /// flags. The single source of truth for the subcommand→mode mapping —
    /// [`classify_execution_plan`] and `xtask docs-gen` (the `cli-subcommands`
    /// table in `docs/cli-reference.md`) both read it, so the docs cannot
    /// drift from the dispatch. The exhaustive match forces a new variant to
    /// pick a mode here.
    pub fn execution_mode(&self) -> (ExecutionMode, ExecutionReason) {
        match self {
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
        }
    }
}

pub fn classify_execution_plan(cli: &Cli, io: IoCapabilities) -> ExecutionPlan {
    if let Some(command) = &cli.command {
        let (mode, reason) = command.execution_mode();
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
#[path = "execution_plan.test.rs"]
mod tests;
