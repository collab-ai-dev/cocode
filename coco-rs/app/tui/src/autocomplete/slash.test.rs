use super::is_tight_subsequence;
use super::rank;
use crate::state::SlashCommandInfo;
use coco_types::CommandSource;
use coco_types::CommandTypeTag;

fn cmd(name: &str, aliases: &[&str], desc: Option<&str>, hint: Option<&str>) -> SlashCommandInfo {
    SlashCommandInfo {
        name: name.to_string(),
        description: desc.map(ToString::to_string),
        aliases: aliases.iter().map(ToString::to_string).collect(),
        argument_hint: hint.map(ToString::to_string),
        ..SlashCommandInfo::default()
    }
}

fn builtin(name: &str, desc: Option<&str>) -> SlashCommandInfo {
    // type=local commands always sit in the Builtin bucket of the
    // empty-query layout regardless of `source`.
    SlashCommandInfo {
        name: name.to_string(),
        description: desc.map(ToString::to_string),
        source: Some(CommandSource::Builtin),
        kind: CommandTypeTag::Local,
        ..SlashCommandInfo::default()
    }
}

fn prompt(name: &str, source: CommandSource) -> SlashCommandInfo {
    SlashCommandInfo {
        name: name.to_string(),
        description: Some(format!("{name} prompt body")),
        source: Some(source),
        kind: CommandTypeTag::Prompt,
        ..SlashCommandInfo::default()
    }
}

fn labels(items: &[crate::widgets::suggestion_popup::SuggestionItem]) -> Vec<&str> {
    items.iter().map(|i| i.label.as_str()).collect()
}

#[test]
fn empty_query_alpha_sorts_within_builtin_bucket() {
    // The new empty-query layout drops registry order in favor of source
    // bucketing. When every command shares one bucket the result is alphabetical.
    let cmds = vec![cmd("help", &[], None, None), cmd("clear", &[], None, None)];
    let items = rank("", &cmds);
    assert_eq!(labels(&items), vec!["/clear", "/help"]);
}

#[test]
fn empty_query_groups_by_source_bucket_order() {
    // Concatenation order: builtin → user → project → policy → other.
    // Mixed source commands should land in this exact sequence regardless
    // of input order.
    let cmds = vec![
        prompt("z-other", CommandSource::Plugin { name: "foo".into() }),
        prompt("a-project", CommandSource::Project),
        prompt("m-user", CommandSource::User),
        prompt("k-policy", CommandSource::Managed),
        builtin("help", Some("Show help")),
    ];
    let items = rank("", &cmds);
    assert_eq!(
        labels(&items),
        vec!["/help", "/m-user", "/a-project", "/k-policy", "/z-other"]
    );
}

#[test]
fn exact_name_beats_prefix_alias() {
    // `c` should rank `/c` (exact name) above `/clear` (prefix on name).
    let cmds = vec![
        cmd("clear", &["cls"], None, None),
        cmd("c", &[], None, None),
    ];
    let items = rank("c", &cmds);
    assert_eq!(labels(&items)[0], "/c");
}

#[test]
fn alias_match_finds_command() {
    // Typing the alias still surfaces the command — without alias
    // matching the popup would silently miss `/clear` for `/cls`.
    let cmds = vec![
        cmd("help", &[], None, None),
        cmd("clear", &["cls"], None, None),
    ];
    let items = rank("cls", &cmds);
    assert_eq!(labels(&items), vec!["/clear"]);
}

#[test]
fn prefix_beats_contains() {
    // `/help` (prefix) outranks `/quick-help` (contains).
    let cmds = vec![
        cmd("quick-help", &[], None, None),
        cmd("help", &[], None, None),
    ];
    let items = rank("help", &cmds);
    assert_eq!(labels(&items), vec!["/help", "/quick-help"]);
}

#[test]
fn shorter_name_wins_within_bucket() {
    // Both prefix-name matches — `/h` is shorter than `/help` so it
    // sorts first.
    let cmds = vec![cmd("help", &[], None, None), cmd("h", &[], None, None)];
    let items = rank("h", &cmds);
    assert_eq!(labels(&items), vec!["/h", "/help"]);
}

#[test]
fn case_insensitive_match() {
    let cmds = vec![cmd("Help", &[], None, None)];
    let items = rank("HELP", &cmds);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "/Help");
}

#[test]
fn no_match_returns_empty() {
    let cmds = vec![cmd("help", &[], None, None)];
    assert!(rank("zzz", &cmds).is_empty());
}

#[test]
fn subsequence_fallback_finds_typo() {
    // `clr` is a subsequence of `clear` (c→l→r). Prefix + contains both
    // miss, but the subsequence bucket catches it.
    let cmds = vec![cmd("clear", &[], None, None), cmd("help", &[], None, None)];
    let items = rank("clr", &cmds);
    assert_eq!(labels(&items), vec!["/clear"]);
}

#[test]
fn subsequence_rejects_wide_spread_match() {
    // For needle `ame` (len 3) the span cap is 6.
    // `add-marker` greedy-matches a@0, m@4, e@8 → span 9 > 6 → rejected.
    // `amber-easy` matches a@0, m@1, e@3 → span 4 ≤ 6 → kept.
    // (Neither contains `ame` contiguously nor starts with it, so both
    // bypass Prefix/Contains and only the Subsequence bucket can match.)
    let cmds = vec![
        cmd("add-marker", &[], None, None),
        cmd("amber-easy", &[], None, None),
    ];
    let items = rank("ame", &cmds);
    assert_eq!(labels(&items), vec!["/amber-easy"]);
}

#[test]
fn empty_query_alpha_orders_unsourced_commands() {
    // Commands without a `source` (e.g. tests that don't bother) land
    // in the builtin bucket because their `kind` defaults to Local —
    // the bucket then alpha-sorts.
    let cmds = vec![cmd("clear", &[], None, None), cmd("help", &[], None, None)];
    let items = rank("", &cmds);
    assert_eq!(labels(&items), vec!["/clear", "/help"]);
}

#[test]
fn prefix_outranks_subsequence() {
    // Both `clear` and `compact` prefix-match `c`; shorter name wins
    // (clear=5 < compact=7).
    let cmds = vec![
        cmd("clear", &[], None, None),
        cmd("compact", &[], None, None),
    ];
    let items = rank("c", &cmds);
    let ls = labels(&items);
    assert_eq!(ls[0], "/clear");
    assert_eq!(ls[1], "/compact");
}

#[test]
fn argument_hint_prepended_to_description() {
    let cmds = vec![cmd(
        "add-dir",
        &[],
        Some("Mount an extra working directory"),
        Some("<path>"),
    )];
    let items = rank("add", &cmds);
    assert_eq!(
        items[0].description.as_deref(),
        Some("<path>  Mount an extra working directory")
    );
}

#[test]
fn argument_hint_alone_when_no_description() {
    let cmds = vec![cmd("tag", &[], None, Some("<name>"))];
    let items = rank("tag", &cmds);
    assert_eq!(items[0].description.as_deref(), Some("<name>"));
}

#[test]
fn tied_results_are_alphabetical_within_bucket() {
    // Verifies the final tiebreak — without it, equal-priority equal-len
    // matches would pick up whatever order the input slice happened to
    // be in (which traces back to `HashMap::values()` in `snapshot_for_ui`).
    let cmds = vec![
        cmd("zebra", &[], None, None),
        cmd("zen", &[], None, None),
        cmd("zap", &[], None, None),
    ];
    let items = rank("z", &cmds);
    let ls = labels(&items);
    // All three prefix-match. `zap` and `zen` share len 3 — alphabetical
    // tiebreak orders them ahead of `zebra` (len 5).
    assert_eq!(ls, vec!["/zap", "/zen", "/zebra"]);
}

#[test]
fn description_appends_user_source_suffix() {
    // User-installed skills get `text (user)`.
    let cmds = vec![prompt("my-skill", CommandSource::User)];
    let items = rank("my-skill", &cmds);
    assert_eq!(
        items[0].description.as_deref(),
        Some("my-skill prompt body (user)")
    );
}

#[test]
fn description_appends_project_source_suffix() {
    let cmds = vec![prompt("scoped", CommandSource::Project)];
    let items = rank("scoped", &cmds);
    assert_eq!(
        items[0].description.as_deref(),
        Some("scoped prompt body (project)")
    );
}

#[test]
fn description_appends_policy_for_managed_source() {
    // The "policy" group; coco-rs internal name is Managed.
    let cmds = vec![prompt("enterprise", CommandSource::Managed)];
    let items = rank("enterprise", &cmds);
    assert_eq!(
        items[0].description.as_deref(),
        Some("enterprise prompt body (policy)")
    );
}

#[test]
fn description_appends_bundled_suffix() {
    let cmds = vec![prompt("ship-it", CommandSource::Bundled)];
    let items = rank("ship-it", &cmds);
    assert_eq!(
        items[0].description.as_deref(),
        Some("ship-it prompt body (bundled)")
    );
}

#[test]
fn description_prefixes_plugin_name_when_known() {
    // The plugin name is shown in parentheses BEFORE the description when known.
    let cmd = SlashCommandInfo {
        name: "from-plugin".into(),
        description: Some("Plugin-backed prompt".into()),
        source: Some(CommandSource::Plugin {
            name: "my-plugin".into(),
        }),
        kind: CommandTypeTag::Prompt,
        ..SlashCommandInfo::default()
    };
    let items = rank("from-plugin", &[cmd]);
    assert_eq!(
        items[0].description.as_deref(),
        Some("(my-plugin) Plugin-backed prompt")
    );
}

#[test]
fn description_falls_back_to_plugin_suffix_when_name_empty() {
    // An empty plugin name shouldn't crash or render `() text` — fall
    // back to the generic `(plugin)` suffix so the row still tells
    // users where the command came from. (Plugin manifests with empty
    // names are invalid and should be caught upstream, but the
    // formatter must stay defensive.)
    let cmd = SlashCommandInfo {
        name: "anon".into(),
        description: Some("Anon plugin command".into()),
        source: Some(CommandSource::Plugin {
            name: String::new(),
        }),
        kind: CommandTypeTag::Prompt,
        ..SlashCommandInfo::default()
    };
    let items = rank("anon", &[cmd]);
    assert_eq!(
        items[0].description.as_deref(),
        Some("Anon plugin command (plugin)")
    );
}

#[test]
fn description_unchanged_for_builtin_and_mcp() {
    // Builtin and MCP commands carry no annotation suffix.
    let builtin = SlashCommandInfo {
        name: "help".into(),
        description: Some("Show help".into()),
        source: Some(CommandSource::Builtin),
        kind: CommandTypeTag::Local,
        ..SlashCommandInfo::default()
    };
    let mcp = SlashCommandInfo {
        name: "weather:forecast".into(),
        description: Some("Today's forecast".into()),
        source: Some(CommandSource::Mcp {
            server_name: "weather".into(),
        }),
        kind: CommandTypeTag::Prompt,
        ..SlashCommandInfo::default()
    };
    let cmds = vec![builtin, mcp];
    let h = rank("help", &cmds);
    assert_eq!(h[0].description.as_deref(), Some("Show help"));
    let m = rank("weather", &cmds);
    assert_eq!(m[0].description.as_deref(), Some("Today's forecast"));
}

#[test]
fn is_tight_subsequence_unit_cases() {
    // Direct unit checks for the matcher — easier to debug than going
    // through `rank` when the span cap behaves unexpectedly.
    assert!(is_tight_subsequence("clr", "clear")); // span 5, cap 6
    assert!(is_tight_subsequence("ab", "abc")); // span 2, cap 4
    assert!(is_tight_subsequence("", "anything")); // empty needle always matches
    assert!(!is_tight_subsequence("ame", "add-marker")); // span 9 > cap 6
    assert!(!is_tight_subsequence("abc", "axxxxxbc")); // span 8 > cap 6
    assert!(!is_tight_subsequence("xyz", "abcdef")); // not a subsequence at all
}
