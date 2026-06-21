//! Shell execution, bash security, destructive warnings, sandbox decisions,
//! and shell environment snapshot support.
//!
//! Two layers:
//!
//! - **Provider layer** ([`provider::ShellProvider`]) — knows the shell's
//!   quirks: snapshot sourcing, session-env injection, extglob disabling,
//!   `pwd -P` tracking, base64-encoded PowerShell, sandbox `TMPDIR`
//!   overrides, `COCO_SHELL_PREFIX` wrapping. Two impls: [`BashProvider`]
//!   for bash/zsh/sh, [`PowerShellProvider`] for pwsh.
//!
//! - **Executor layer** ([`ShellExecutor`]) — spawn / wait / cancel /
//!   timeout / sandbox-wrap loop on top of the provider's output.
//!

pub mod ast;
pub mod bash_permissions;
pub mod destructive;
pub mod executor;
pub mod heredoc;
pub mod mode_validation;
pub mod path_validation;
pub mod pipe_rearrange;
pub mod provider;
pub mod read_only;
pub mod result;
pub mod safety;
pub mod security;
pub mod sed_parser;
pub mod semantics;
pub mod session_env;
pub mod shell_prefix;
pub mod shell_quoting;
pub mod shell_types;
pub mod shell_utils;
pub mod snapshot;
pub mod tokenizer;
pub mod windows_paths;

pub use ast::BashNode;
pub use ast::SimpleCommand;
pub use ast::extract_simple_commands;
pub use ast::parse_command;
pub use bash_permissions::CompoundCommandAnalysis;
pub use bash_permissions::analyze_compound_command;
pub use bash_permissions::extract_output_redirect_targets;
pub use bash_permissions::get_command_prefix;
pub use bash_permissions::get_first_word_prefix;
pub use bash_permissions::has_process_substitution;
pub use bash_permissions::heredoc_command_prefix;
pub use bash_permissions::is_dangerous_bare_prefix;
pub use bash_permissions::split_compound_command;
pub use bash_permissions::strip_all_env_vars;
pub use bash_permissions::strip_output_redirections;
pub use bash_permissions::strip_safe_wrappers;
pub use executor::ShellExecutor;
pub use executor::ShellProgress;
pub use heredoc::HeredocContent;
pub use heredoc::extract_heredocs;
pub use mode_validation::is_auto_allowed_in_accept_edits;
pub use path_validation::check_dangerous_removal;
pub use path_validation::check_git_escape;
pub use path_validation::check_multiple_cwd_changes;
pub use path_validation::extract_write_path_targets;
pub use path_validation::has_git_escape_pattern;
pub use path_validation::has_git_escape_pattern_in_cwd;
pub use provider::BashProvider;
pub use provider::BuildExecOpts;
pub use provider::BuiltCommand;
pub use provider::PowerShellProvider;
pub use provider::ShellProvider;
pub use result::CommandResult;
pub use result::ExecOptions;
pub use safety::SafetyResult;
pub use safety::SecurityCheckId;
pub use security::SecurityCheck;
pub use security::SecuritySeverity;
pub use security::check_security;
pub use sed_parser::SedEditInfo;
pub use sed_parser::has_dangerous_sed;
pub use sed_parser::is_sed_in_place_edit;
pub use sed_parser::parse_sed_edit_command;
pub use semantics::CommandResultInterpretation;
pub use semantics::interpret_command_result;
pub use session_env::SessionEnvReader;
pub use session_env::SessionEnvVars;
pub use session_env::session_env_dir;
pub use shell_types::Shell;
pub use shell_types::ShellType;
pub use shell_types::default_user_shell;
pub use shell_types::detect_shell_type;
pub use shell_types::get_shell;
pub use shell_types::shell_from_config;
pub use snapshot::ShellSnapshot;
pub use snapshot::SnapshotConfig;
pub use snapshot::cleanup_stale_snapshots;
