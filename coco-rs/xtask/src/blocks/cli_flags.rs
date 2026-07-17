//! `cli-flags` block — every global flag on the top-level `coco` command.
//!
//! The flag set, short forms, and whether a flag takes a value all come from
//! the clap schema (`coco_cli::Cli`). The placeholder and the description are
//! hand-maintained below, because the doc cells carry cross-document markdown
//! links that cannot live in `--help` output.
//!
//! Both directions are gated: a clap flag with no entry, an entry naming a flag
//! clap no longer has, or an entry disagreeing with clap about whether the flag
//! takes a value, all fail the run.

use anyhow::Result;
use anyhow::bail;
use clap::Command;
use clap::CommandFactory;
use coco_cli::Cli;

/// Hand-maintained half of a flag row, keyed by the clap long name.
struct FlagDoc {
    long: &'static str,
    /// Value placeholder as rendered between backticks, or `None` for a switch.
    /// Must agree with clap on whether the flag takes a value.
    value: Option<&'static str>,
    description: &'static str,
}

pub fn render() -> Result<String> {
    let mut command = Cli::command();
    // `build` materializes the implicit `--help` / `--version` args.
    command.build();

    let args = flag_args(&command);
    check_coverage(&args)?;

    let mut out = String::from("| Flag | Value | Description |\n|------|-------|-------------|");
    for arg in &args {
        let long = long_name(arg)?;
        let Some(doc) = FLAG_DOCS.iter().find(|d| d.long == long) else {
            bail!("{}", missing_entry_error(long));
        };
        check_value_shape(arg, doc)?;

        let flag = match arg.get_short() {
            Some(short) => format!("`-{short}`, `--{long}`"),
            None => format!("`--{long}`"),
        };
        let value = match doc.value {
            Some(value) => format!("`{value}`"),
            None => String::new(),
        };
        let description = doc.description;
        out.push_str(&format!("\n| {flag} | {value} | {description} |"));
    }
    Ok(out)
}

/// Non-hidden option/switch args, in clap declaration order.
fn flag_args(command: &Command) -> Vec<&clap::Arg> {
    command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set() && arg.get_long().is_some())
        .collect()
}

fn long_name(arg: &clap::Arg) -> Result<&str> {
    match arg.get_long() {
        Some(long) => Ok(long),
        None => bail!("argument `{}` has no long form", arg.get_id()),
    }
}

fn takes_value(arg: &clap::Arg) -> bool {
    arg.get_num_args().is_some_and(|range| range.takes_values())
}

fn check_value_shape(arg: &clap::Arg, doc: &FlagDoc) -> Result<()> {
    let clap_takes_value = takes_value(arg);
    if clap_takes_value != doc.value.is_some() {
        let long = doc.long;
        let (clap_says, doc_says) = if clap_takes_value {
            ("takes a value", "documents it as a switch")
        } else {
            ("is a switch", "documents a value placeholder")
        };
        bail!(
            "`--{long}` {clap_says} in the clap schema but xtask {doc_says}. \
             Update `FLAG_DOCS` in xtask/src/blocks/cli_flags.rs."
        );
    }
    Ok(())
}

fn check_coverage(args: &[&clap::Arg]) -> Result<()> {
    for doc in FLAG_DOCS {
        if !args.iter().any(|arg| arg.get_long() == Some(doc.long)) {
            bail!(
                "xtask documents `--{}`, which the clap schema no longer has (or now hides). \
                 Remove the entry from `FLAG_DOCS` in xtask/src/blocks/cli_flags.rs.",
                doc.long
            );
        }
    }
    Ok(())
}

fn missing_entry_error(long: &str) -> String {
    format!(
        "the clap schema has `--{long}` but xtask has no entry for it. Add one to `FLAG_DOCS` in \
         xtask/src/blocks/cli_flags.rs so `docs/cli-reference.md` documents the new flag."
    )
}

const FLAG_DOCS: &[FlagDoc] = &[
    FlagDoc {
        long: "prompt",
        value: Some("<PROMPT>"),
        description: "Prompt to send. Implies non-interactive (headless) mode.",
    },
    FlagDoc {
        long: "models.main",
        value: Some("<PROVIDER/MODEL_ID>"),
        description: "Override the model bound to the Main role for this run.",
    },
    FlagDoc {
        long: "settings",
        value: Some("<PATH>"),
        description: "Load an additional settings file as the Flag layer (see [configuration](configuration.md)).",
    },
    FlagDoc {
        long: "event-hub-url",
        value: Some("<WS_URL>"),
        description: "Send this session's events to an external Event Hub. Must be `ws://` or `wss://`. Conflicts with `--serve-hub`.",
    },
    FlagDoc {
        long: "serve-hub",
        value: None,
        description: "Start an embedded local Event Hub and point this process at it. Requires a binary built with the `serve-hub` cargo feature.",
    },
    FlagDoc {
        long: "hub-port",
        value: Some("<PORT>"),
        description: "Port for the embedded Event Hub. Default `8731`. Requires `--serve-hub`.",
    },
    FlagDoc {
        long: "max-tokens",
        value: Some("<N>"),
        description: "Maximum tokens per model response.",
    },
    FlagDoc {
        long: "max-turns",
        value: Some("<N>"),
        description: "Maximum agent turns before the run stops.",
    },
    FlagDoc {
        long: "permission-mode",
        value: Some("<MODE>"),
        description: "Starting permission mode: `default`, `plan`, `acceptEdits`, `bypassPermissions`, `dontAsk`, or `auto`.",
    },
    FlagDoc {
        long: "cwd",
        value: Some("<DIR>"),
        description: "Working directory for the session. Defaults to the process working directory.",
    },
    FlagDoc {
        long: "resume",
        value: Some("<SESSION_ID>"),
        description: "Resume a specific session by ID or title.",
    },
    FlagDoc {
        long: "continue-session",
        value: None,
        description: "Continue the most recent conversation. Also accepted as `--continue`.",
    },
    FlagDoc {
        long: "system-prompt",
        value: Some("<TEXT>"),
        description: "Replace the built-in system prompt entirely with this text.",
    },
    FlagDoc {
        long: "append-system-prompt",
        value: Some("<TEXT>"),
        description: "Append this text verbatim to the system prompt.",
    },
    FlagDoc {
        long: "append-system-prompt-file",
        value: Some("<PATH>"),
        description: "Read a file and append its contents to the system prompt. Fails if the file is missing.",
    },
    FlagDoc {
        long: "allowed-tools",
        value: Some("<TOOL>..."),
        description: "Allow specific tools. Accepts multiple values after one flag, and the flag may repeat.",
    },
    FlagDoc {
        long: "disallowed-tools",
        value: Some("<TOOL>..."),
        description: "Deny specific tools. Same repeat semantics as `--allowed-tools`.",
    },
    FlagDoc {
        long: "add-dir",
        value: Some("<DIR>..."),
        description: "Grant the session access to additional directories outside the working directory.",
    },
    FlagDoc {
        long: "dangerously-skip-permissions",
        value: None,
        description: "Start in `bypassPermissions` mode and unlock it as a reachable target for Shift+Tab and plan-mode exit.",
    },
    FlagDoc {
        long: "allow-dangerously-skip-permissions",
        value: None,
        description: "Unlock `bypassPermissions` as an option without entering it at startup.",
    },
    FlagDoc {
        long: "non-interactive",
        value: None,
        description: "Print the response and exit. Also accepted as `--print`.",
    },
    FlagDoc {
        long: "fallback-model",
        value: Some("<PROVIDER/MODEL_ID>"),
        description: "Fallback model on capacity errors. Repeat the flag to build a multi-tier chain; tiers are walked in flag order.",
    },
    FlagDoc {
        long: "no-session-persistence",
        value: None,
        description: "Do not persist the session. Only valid in print mode or SDK mode.",
    },
    FlagDoc {
        long: "bare",
        value: None,
        description: "Skip session-start and per-turn background housekeeping (auto-dream, memory extraction, prompt suggestion, stale-directory sweeps). Flag form of `COCO_BARE_MODE=1`.",
    },
    FlagDoc {
        long: "json-schema",
        value: Some("<JSON>"),
        description: "Inline JSON Schema (not a file path) validating the run's structured output. Honored in print mode and SDK mode; ignored in the TUI.",
    },
    FlagDoc {
        long: "include-hook-events",
        value: None,
        description: "Emit hook lifecycle events (`HookStarted`, `HookProgress`, `HookResponse`) in the stream-json output.",
    },
    FlagDoc {
        long: "setting-sources",
        value: Some("<CSV>"),
        description: "Comma-separated settings layers to load: `user`, `project`, `local`. See [configuration](configuration.md).",
    },
    FlagDoc {
        long: "fork-session",
        value: None,
        description: "Copy the history from `--resume <id>` into a fresh session instead of continuing the original.",
    },
    FlagDoc {
        long: "session-id",
        value: Some("<ID>"),
        description: "Use an explicit session ID for this run. For deterministic IDs in automation.",
    },
    FlagDoc {
        long: "log-level",
        value: Some("<DIRECTIVE>"),
        description: "Tracing filter. A bare level (`debug`) expands to `coco=debug,debug`; a full `EnvFilter` directive is passed through. Highest-priority log override.",
    },
    FlagDoc {
        long: "log-format",
        value: Some("<FORMAT>"),
        description: "`pretty`, `compact`, or `json`. Defaults by mode: `json` for SDK, `compact` for TUI and headless.",
    },
    FlagDoc {
        long: "log-file",
        value: Some("<PATH>"),
        description: "Override the default rotating log file path.",
    },
    FlagDoc {
        long: "log-stderr",
        value: None,
        description: "Add a stderr log layer alongside the file sink.",
    },
    FlagDoc {
        long: "log-location",
        value: Some("[<BOOL>]"),
        description: "Show source `file:line` and thread name on each event. Tri-state: bare flag or `=true` forces on, `=false` forces off, omission auto-enables when the resolved filter is `debug` or `trace`.",
    },
    FlagDoc {
        long: "log-timezone",
        value: Some("<TZ>"),
        description: "`local` or `utc` for log timestamps. Default `local`.",
    },
    FlagDoc {
        long: "help",
        value: None,
        description: "Print help.",
    },
    FlagDoc {
        long: "version",
        value: None,
        description: "Print version, commit, and build time.",
    },
];
