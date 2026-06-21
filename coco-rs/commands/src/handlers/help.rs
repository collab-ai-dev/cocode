//! `/help` — list all available commands grouped by category.
//!
//! With no arguments, shows all commands organized by category.
//! With a command name, shows detailed help for that command.

use std::pin::Pin;

/// A category of related commands.
struct Category {
    name: &'static str,
    commands: &'static [CommandEntry],
}

/// Static metadata for a single command entry in the help listing.
struct CommandEntry {
    name: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    usage: &'static str,
}

const CATEGORIES: &[Category] = &[
    Category {
        name: "Core",
        commands: &[
            CommandEntry {
                name: "help",
                aliases: &["h", "?"],
                description: "Show available commands and help",
                usage: "/help [command]",
            },
            CommandEntry {
                name: "clear",
                aliases: &["reset", "new"],
                description: "Clear conversation history and start fresh",
                usage: "/clear",
            },
            CommandEntry {
                name: "compact",
                aliases: &[],
                description: "Compact conversation to reduce context usage",
                usage: "/compact [instructions]",
            },
            CommandEntry {
                name: "status",
                aliases: &[],
                description: "Show current session status and model info",
                usage: "/status",
            },
            CommandEntry {
                name: "exit",
                aliases: &["quit"],
                description: "Exit the REPL",
                usage: "/exit",
            },
            CommandEntry {
                name: "version",
                aliases: &[],
                description: "Show version info",
                usage: "/version",
            },
        ],
    },
    Category {
        name: "Configuration",
        commands: &[
            CommandEntry {
                name: "config",
                aliases: &["settings"],
                description: "Show or modify configuration",
                usage: "/config [key] [value]",
            },
            CommandEntry {
                name: "model",
                aliases: &[],
                description: "Switch the current model",
                usage: "/model [model]",
            },
            CommandEntry {
                name: "effort",
                aliases: &[],
                description: "Set reasoning effort level",
                usage: "/effort [low|medium|high]",
            },
            CommandEntry {
                name: "permissions",
                aliases: &["perms", "allowed-tools"],
                description: "Manage allow & deny tool permission rules",
                usage: "/permissions [allow|deny] [tool]",
            },
            CommandEntry {
                name: "theme",
                aliases: &[],
                description: "Change the theme",
                usage: "/theme",
            },
            CommandEntry {
                name: "color",
                aliases: &[],
                description: "Set the prompt bar color for this session",
                usage: "/color <color|default>",
            },
            CommandEntry {
                name: "vim",
                aliases: &[],
                description: "Toggle between Vim and Normal editing modes",
                usage: "/vim [on|off|toggle]",
            },
            CommandEntry {
                // This command is hidden; registry-side visibility filters
                // drop it from typeahead/picker.
                // `/help` is hardcoded in coco-rs (see `commands/CLAUDE.md`)
                // so the entry stays here for `/help output-style` lookups,
                // matching the registry description verbatim.
                name: "output-style",
                aliases: &[],
                description: "Deprecated: use /config to change output style",
                usage: "/output-style",
            },
            CommandEntry {
                name: "keybindings",
                aliases: &[],
                description: "Open keybindings configuration",
                usage: "/keybindings",
            },
            CommandEntry {
                name: "sandbox",
                aliases: &[],
                description: "Configure sandbox mode",
                usage: "/sandbox [none|readonly|strict]",
            },
        ],
    },
    Category {
        name: "Session",
        commands: &[
            CommandEntry {
                name: "cost",
                aliases: &[],
                description: "Show total cost and duration of this session",
                usage: "/cost",
            },
            CommandEntry {
                name: "context",
                aliases: &[],
                description: "Show context window usage breakdown",
                usage: "/context",
            },
            CommandEntry {
                name: "session",
                aliases: &["remote"],
                description: "Manage sessions (list, resume, delete)",
                usage: "/session [list|delete|info] [id]",
            },
            CommandEntry {
                name: "resume",
                // Canonical name only — implementations.rs registers
                // /resume with no /continue alias.
                aliases: &[],
                description: "Resume a previous conversation",
                usage: "/resume [session-id]",
            },
            CommandEntry {
                name: "rename",
                aliases: &[],
                description: "Rename the current conversation \
                              (bare `/rename` auto-generates a name)",
                usage: "/rename [name]",
            },
            CommandEntry {
                name: "branch",
                aliases: &["fork"],
                description: "Branch the current conversation",
                usage: "/branch [name]",
            },
            CommandEntry {
                name: "export",
                aliases: &[],
                description: "Export conversation to a file or clipboard",
                usage: "/export [filename]",
            },
            CommandEntry {
                name: "copy",
                aliases: &[],
                description: "Copy last assistant response to clipboard",
                usage: "/copy",
            },
            CommandEntry {
                name: "rewind",
                aliases: &[],
                description: "Rewind the code and/or conversation to a previous point",
                usage: "/rewind",
            },
            CommandEntry {
                name: "stats",
                aliases: &[],
                description: "Show usage statistics and activity",
                usage: "/stats",
            },
        ],
    },
    Category {
        name: "Development",
        commands: &[
            CommandEntry {
                name: "diff",
                aliases: &[],
                description: "Show git diff of current changes",
                usage: "/diff",
            },
            CommandEntry {
                name: "commit",
                aliases: &[],
                description: "Create a git commit with staged changes",
                usage: "/commit [message]",
            },
            CommandEntry {
                name: "review",
                aliases: &[],
                description: "Review a pull request",
                usage: "/review [PR number]",
            },
            CommandEntry {
                name: "init",
                aliases: &[],
                description: "Initialize project with CLAUDE.md",
                usage: "/init",
            },
        ],
    },
    Category {
        name: "Tools & Extensions",
        commands: &[
            CommandEntry {
                name: "mcp",
                aliases: &[],
                description: "Manage MCP server connections",
                usage: "/mcp [list|add|remove|enable|disable] [name]",
            },
            CommandEntry {
                name: "plugin",
                aliases: &["plugins", "marketplace"],
                description: "Manage installed plugins",
                usage: "/plugin [list|install|uninstall] [name]",
            },
            CommandEntry {
                name: "agents",
                aliases: &[],
                description: "List and manage agent definitions",
                usage: "/agents",
            },
            CommandEntry {
                name: "tasks",
                aliases: &["bashes"],
                description: "List and manage active tasks",
                usage: "/tasks",
            },
            CommandEntry {
                name: "skills",
                aliases: &[],
                description: "List available skills",
                usage: "/skills",
            },
            CommandEntry {
                name: "hooks",
                aliases: &[],
                description: "View hook configurations for tool events",
                usage: "/hooks",
            },
            CommandEntry {
                name: "files",
                aliases: &[],
                description: "List files currently tracked in context",
                usage: "/files",
            },
            CommandEntry {
                name: "memory",
                aliases: &[],
                description: "View and manage memory files (CLAUDE.md)",
                usage: "/memory [edit|refresh]",
            },
            CommandEntry {
                name: "plan",
                aliases: &["planning"],
                description: "Toggle plan mode or view current plan",
                usage: "/plan [open|<description>]",
            },
        ],
    },
    Category {
        name: "System",
        commands: &[
            CommandEntry {
                name: "doctor",
                aliases: &[],
                description: "Diagnose and verify installation and settings",
                usage: "/doctor",
            },
            CommandEntry {
                name: "upgrade",
                aliases: &[],
                description: "Check for updates",
                usage: "/upgrade",
            },
            CommandEntry {
                name: "usage",
                aliases: &[],
                description: "Show plan usage limits",
                usage: "/usage",
            },
        ],
    },
];

/// Async handler for `/help [command]`.
///
/// With no arguments, lists all commands grouped by category.
/// With a command name, shows detailed help for that command.
pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let query = args.trim().to_string();

        if query.is_empty() {
            return Ok(list_all_commands());
        }

        // Strip leading slash if provided (e.g. "/help /model" → look up "model")
        let name = query.trim_start_matches('/');
        match find_command(name) {
            Some(entry) => Ok(format_command_detail(entry)),
            None => Ok(format!(
                "No command found: {name}\n\nUse /help to see all available commands."
            )),
        }
    })
}

/// Build the full categorized command listing.
fn list_all_commands() -> String {
    let mut out = String::from("## Available Commands\n\n");
    out.push_str("Use /help <command> for detailed usage.\n\n");

    for category in CATEGORIES {
        out.push_str(&format!("### {}\n\n", category.name));
        for cmd in category.commands {
            let aliases = if cmd.aliases.is_empty() {
                String::new()
            } else {
                format!(" ({})", cmd.aliases.join(", "))
            };
            out.push_str(&format!(
                "  /{:<20} {}{}\n",
                cmd.name, cmd.description, aliases,
            ));
        }
        out.push('\n');
    }

    out
}

/// Format detailed help for a single command entry.
fn format_command_detail(entry: &CommandEntry) -> String {
    let mut out = format!("## /{}\n\n", entry.name);
    out.push_str(&format!("{}\n\n", entry.description));
    out.push_str(&format!("**Usage:** `{}`\n", entry.usage));

    if !entry.aliases.is_empty() {
        let alias_list: Vec<String> = entry.aliases.iter().map(|a| format!("/{a}")).collect();
        out.push_str(&format!("**Aliases:** {}\n", alias_list.join(", ")));
    }

    out
}

/// Find a command entry by name or alias across all categories.
fn find_command(name: &str) -> Option<&'static CommandEntry> {
    for category in CATEGORIES {
        for entry in category.commands {
            if entry.name == name || entry.aliases.contains(&name) {
                return Some(entry);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "help.test.rs"]
mod tests;
