//! `cli-subcommands` block — every subcommand, its execution mode, and how much
//! of it is actually wired up.
//!
//! The subcommand set and their paths come from the clap schema; the Mode column
//! comes from `Commands::execution_mode`, the same match `classify_execution_plan`
//! dispatches on. The Status column is not derivable from either — "this prints a
//! hardcoded version" is a finding about the implementation — so it is
//! hand-maintained below, together with the argument display and the description.
//!
//! Both directions are gated: a clap subcommand with no entry, or an entry for a
//! subcommand clap no longer has, fails the run.

use anyhow::Result;
use anyhow::bail;
use clap::Command;
use clap::CommandFactory;
use coco_cli::Cli;
use coco_cli::Commands;
use coco_cli::execution_plan::ExecutionMode;

/// Hand-maintained half of a subcommand row, keyed by the clap path.
struct SubcommandDoc {
    /// Space-separated clap path, e.g. `"config set"`.
    path: &'static str,
    /// Argument display appended to the path, e.g. `"<KEY> <VALUE>"`.
    args: Option<&'static str>,
    status: &'static str,
    description: &'static str,
}

pub fn render() -> Result<String> {
    let mut command = Cli::command();
    command.build();

    let paths = leaf_paths(&command);
    check_coverage(&paths)?;

    let mut out = String::from(
        "| Subcommand | Mode | Status | Description |\n|------------|------|--------|-------------|",
    );
    for path in &paths {
        let Some(doc) = SUBCOMMAND_DOCS.iter().find(|d| d.path == path.as_str()) else {
            bail!(
                "the clap schema has `coco {path}` but xtask has no entry for it. Add one to \
                 `SUBCOMMAND_DOCS` in xtask/src/blocks/cli_subcommands.rs so \
                 `docs/cli-reference.md` documents the new subcommand."
            );
        };
        let signature = match doc.args {
            Some(args) => format!("{path} {args}"),
            None => path.clone(),
        };
        let mode = mode_label(execution_mode(path)?);
        let status = doc.status;
        let description = doc.description;
        out.push_str(&format!(
            "\n| `{signature}` | {mode} | {status} | {description} |"
        ));
    }
    Ok(out)
}

/// Every runnable subcommand path, in clap declaration order. A subcommand that
/// only groups children (`config`, `mcp`, …) contributes its children, not itself.
fn leaf_paths(command: &Command) -> Vec<String> {
    let mut out = Vec::new();
    for sub in command.get_subcommands() {
        collect_leaves(sub, String::new(), &mut out);
    }
    out
}

fn collect_leaves(command: &Command, prefix: String, out: &mut Vec<String>) {
    if command.is_hide_set() {
        return;
    }
    let name = command.get_name();
    // `help` is synthesized by clap under every group that has subcommands. It
    // is not part of the CLI anyone designed, and documenting `coco config help
    // get` alongside real commands would be noise.
    if name == "help" {
        return;
    }
    let path = if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix} {name}")
    };
    let mut children = command.get_subcommands().peekable();
    if children.peek().is_none() {
        out.push(path);
        return;
    }
    for child in children {
        collect_leaves(child, path.clone(), out);
    }
}

/// Execution mode for a path, resolved through the same match the dispatcher
/// uses. Keyed by the top-level segment — nested actions inherit their parent's
/// mode, which is how `classify_execution_plan` treats them.
fn execution_mode(path: &str) -> Result<ExecutionMode> {
    let top = path.split(' ').next().unwrap_or(path);
    let Some(command) = sample_command(top) else {
        bail!(
            "no sample `Commands` value for `coco {top}` — add one to `sample_command` in \
             xtask/src/blocks/cli_subcommands.rs so the Mode column can be derived."
        );
    };
    Ok(command.execution_mode().0)
}

/// One representative `Commands` value per top-level subcommand. Field values
/// are irrelevant: `execution_mode` matches on the variant alone.
fn sample_command(name: &str) -> Option<Commands> {
    use coco_cli::ConfigAction;
    use coco_cli::McpAction;
    use coco_cli::MoaAction;
    use coco_cli::PluginAction;

    let command = match name {
        "chat" => Commands::Chat { prompt: None },
        "config" => Commands::Config {
            action: ConfigAction::List,
        },
        "resume" => Commands::Resume { session_id: None },
        "sessions" => Commands::Sessions,
        "status" => Commands::Status,
        "doctor" => Commands::Doctor,
        "login" => Commands::Login {
            provider: None,
            no_browser: false,
            import: None,
        },
        "logout" => Commands::Logout { provider: None },
        "init" => Commands::Init,
        "review" => Commands::Review { target: None },
        "mcp" => Commands::Mcp {
            action: McpAction::List,
        },
        "plugin" => Commands::Plugin {
            action: PluginAction::List,
        },
        "moa" => Commands::Moa {
            action: MoaAction::List,
        },
        "agents" => Commands::Agents,
        "auto-mode" => Commands::AutoMode { subcmd: None },
        "exec-server" => Commands::ExecServer {
            listen: String::new(),
        },
        "ps" => Commands::Ps {
            json: false,
            all: false,
        },
        "release-notes" => Commands::ReleaseNotes,
        "sdk" => Commands::Sdk,
        _ => return None,
    };
    Some(command)
}

fn mode_label(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Skip => "Skip",
        ExecutionMode::Tui => "TUI",
        ExecutionMode::Headless => "Headless",
        ExecutionMode::Sdk => "SDK",
    }
}

fn check_coverage(paths: &[String]) -> Result<()> {
    for doc in SUBCOMMAND_DOCS {
        if !paths.iter().any(|path| path == doc.path) {
            bail!(
                "xtask documents `coco {}`, which the clap schema no longer has (or now hides). \
                 Remove the entry from `SUBCOMMAND_DOCS` in \
                 xtask/src/blocks/cli_subcommands.rs.",
                doc.path
            );
        }
    }
    Ok(())
}

const SUBCOMMAND_DOCS: &[SubcommandDoc] = &[
    SubcommandDoc {
        path: "chat",
        args: Some("[PROMPT]"),
        status: "Working",
        description: "Run one non-interactive turn. Defaults to the prompt `Hello!` when none is given.",
    },
    SubcommandDoc {
        path: "config get",
        args: Some("<KEY>"),
        status: "Working",
        description: "Print one top-level merged settings value. Lists available keys when the key is absent.",
    },
    SubcommandDoc {
        path: "config set",
        args: Some("<KEY> <VALUE>"),
        status: "Works",
        description: "Writes the key to your user `settings.json`, preserving comments and formatting. Accepts dotted keys (`models.main`). The value is read as JSON when it parses (`true` becomes a boolean), otherwise as a string. Takes effect in the next session.",
    },
    SubcommandDoc {
        path: "config list",
        args: None,
        status: "Working",
        description: "Print the fully merged settings as JSON.",
    },
    SubcommandDoc {
        path: "config reset",
        args: None,
        status: "Working",
        description: "Delete the user settings file.",
    },
    SubcommandDoc {
        path: "resume",
        args: Some("[SESSION_ID]"),
        status: "Working",
        description: "Resume a session in the interactive UI. With no ID, continues the most recent conversation.",
    },
    SubcommandDoc {
        path: "sessions",
        args: None,
        status: "Working",
        description: "List saved sessions with ID, model, creation time, and working directory.",
    },
    SubcommandDoc {
        path: "status",
        args: None,
        status: "**Partly hardcoded**",
        description: "Prints a hardcoded `coco-rs v0.0.0` as the version. The model, provider, and auth lines below it are real; the version is not. Use `coco --version` for the real build.",
    },
    SubcommandDoc {
        path: "doctor",
        args: None,
        status: "**Partly hardcoded**",
        description: "The shell and config lines are printed unconditionally without probing anything. Only the model line and the auth status are resolved for real.",
    },
    SubcommandDoc {
        path: "login",
        args: Some("[PROVIDER]"),
        status: "Working",
        description: "Log in to a provider subscription via OAuth. Defaults to `openai`. `--no-browser` prints the authorization URL instead of opening a browser; `--import <PATH>` imports a credential from another tool's auth file.",
    },
    SubcommandDoc {
        path: "logout",
        args: Some("[PROVIDER]"),
        status: "Working",
        description: "Clear stored provider credentials. Defaults to `openai`.",
    },
    SubcommandDoc {
        path: "init",
        args: None,
        status: "Working",
        description: "Create a `.cocode/` directory in the current directory with an empty `settings.json`.",
    },
    SubcommandDoc {
        path: "review",
        args: Some("[TARGET]"),
        status: "Working",
        description: "Ask the agent to review changes. Defaults to `HEAD`.",
    },
    SubcommandDoc {
        path: "mcp list",
        args: None,
        status: "**Stub**",
        description: "Always prints `MCP servers: (none connected)` regardless of configuration.",
    },
    SubcommandDoc {
        path: "mcp add",
        args: Some("<NAME> [CONFIG]"),
        status: "**Stub**",
        description: "Echoes its arguments. Nothing is registered.",
    },
    SubcommandDoc {
        path: "mcp remove",
        args: Some("<NAME>"),
        status: "**Stub**",
        description: "Echoes its argument. Nothing is removed.",
    },
    SubcommandDoc {
        path: "mcp login",
        args: Some("<NAME>"),
        status: "Working",
        description: "Run the OAuth flow for an MCP server. `--no-browser` prints the URL instead.",
    },
    SubcommandDoc {
        path: "mcp logout",
        args: Some("<NAME>"),
        status: "Working",
        description: "Clear stored OAuth credentials for an MCP server.",
    },
    SubcommandDoc {
        path: "plugin list",
        args: None,
        status: "Working",
        description: "List installed and enabled plugins.",
    },
    SubcommandDoc {
        path: "plugin install",
        args: Some("<NAME>"),
        status: "Working",
        description: "Install from a local directory containing `PLUGIN.toml`, or from a registered marketplace via `<name>[@<marketplace>]`.",
    },
    SubcommandDoc {
        path: "plugin uninstall",
        args: Some("<NAME>"),
        status: "Working",
        description: "Uninstall a plugin by name.",
    },
    SubcommandDoc {
        path: "plugin validate",
        args: Some("<PATH>"),
        status: "Working",
        description: "Validate a `PLUGIN.toml` manifest.",
    },
    SubcommandDoc {
        path: "moa list",
        args: None,
        status: "Working",
        description: "List configured Mixture-of-Agents presets.",
    },
    SubcommandDoc {
        path: "moa configure",
        args: Some("<NAME>"),
        status: "Working",
        description: "Create or replace a MoA preset. See [models and MoA](models-and-moa.md).",
    },
    SubcommandDoc {
        path: "moa delete",
        args: Some("<NAME>"),
        status: "Working",
        description: "Delete a MoA preset.",
    },
    SubcommandDoc {
        path: "agents",
        args: None,
        status: "Working",
        description: "List discovered agent definitions from the built-in catalog and the user and project agent directories.",
    },
    SubcommandDoc {
        path: "auto-mode",
        args: Some("[defaults]"),
        status: "**Stub**",
        description: "Prints a fixed one-line pointer to `/permissions`. It shows no actual defaults.",
    },
    SubcommandDoc {
        path: "exec-server",
        args: Some("[--listen URL]"),
        status: "Working",
        description: "Run a local exec-server over WebSocket or stdio.",
    },
    SubcommandDoc {
        path: "ps",
        args: Some("[--json] [--all]"),
        status: "Working",
        description: "List running background sessions. `--json` emits a JSON array for scripting; `--all` includes completed and failed entries.",
    },
    SubcommandDoc {
        path: "release-notes",
        args: None,
        status: "**Wrong target**",
        description: "Prints the current version and then links to the `anthropics/claude-code` releases page, which is a different project. Ignore the link.",
    },
    SubcommandDoc {
        path: "sdk",
        args: None,
        status: "Working",
        description: "Run in SDK mode: NDJSON over stdio with the JSON-RPC control protocol. Intended to be spawned as a subprocess by the Python or TypeScript SDK.",
    },
];
