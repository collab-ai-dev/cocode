use super::apply_async_result;
use super::apply_async_result_for_key;
use super::detect;
use super::refresh_suggestions;
use crate::state::ActiveSuggestions;
use crate::state::AppState;
use crate::state::SavedSession;
use crate::state::SlashCommandInfo;
use crate::state::SuggestionKind;
use crate::widgets::suggestion_popup::SuggestionItem;
use crate::widgets::suggestion_popup::SuggestionMeta;
use coco_types::CommandArgumentKind;

fn slash(name: &str, description: Option<&str>) -> SlashCommandInfo {
    SlashCommandInfo {
        name: name.to_string(),
        description: description.map(ToString::to_string),
        ..SlashCommandInfo::default()
    }
}

fn temp_completion_dir(test_name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "coco-tui-completion-{test_name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp completion dir");
    dir
}

#[test]
fn test_slash_command_trigger() {
    let t = detect("/con", 4).expect("slash command detected");
    assert_eq!(t.kind, SuggestionKind::SlashCommand);
    assert_eq!(t.pos, 0);
    assert_eq!(t.query, "con");
}

#[test]
fn test_slash_with_space_is_not_trigger() {
    // Once the user has typed a space after the slash, we're in argument
    // territory and the suggestion popup should dismiss.
    assert!(detect("/help foo", 9).is_none());
}

#[test]
fn test_at_trigger_unified() {
    // After the @-trigger unification, `@src` produces a single `At`
    // kind — the popup decides per-row whether each suggestion is a
    // file path, an agent, or an MCP resource.
    let t = detect("hello @src", 10).expect("at trigger detected");
    assert_eq!(t.kind, SuggestionKind::At);
    assert_eq!(t.pos, 6);
    assert_eq!(t.query, "src");
}

#[test]
fn test_at_bare_word_no_agent_subprefix() {
    // The legacy `@agent-<name>` sub-prefix is gone. `@agent-plan`
    // becomes a unified `@` query whose body happens to start with
    // `agent-` — agents fall out of `seed_agent_items` by fuzzy match
    // on the bare name, not by prefix.
    let t = detect("@agent-plan", 11).expect("at trigger detected");
    assert_eq!(t.kind, SuggestionKind::At);
    assert_eq!(t.query, "agent-plan");
}

#[test]
fn test_at_symbol_trigger() {
    let t = detect("@#foo", 5).expect("symbol trigger detected");
    assert_eq!(t.kind, SuggestionKind::Symbol);
    assert_eq!(t.query, "foo");
}

#[test]
fn test_email_does_not_trigger() {
    // `@` after non-whitespace must not start a suggestion — protects
    // `user@example.com` from being treated as a mention.
    assert!(detect("mylinyuzhi@gmail.com", 20).is_none());
}

#[test]
fn test_no_trigger_plain_text() {
    assert!(detect("hello world", 11).is_none());
}

#[test]
fn test_cjk_at_trigger_uses_byte_offset() {
    // "你好" is 6 bytes (3 per CJK char). A trigger at byte 6 puts `@`
    // immediately after the 2-char prefix; the splice site must be the
    // byte offset, not the char index.
    let text = "你好 @src";
    // After "你好 " is 7 bytes, then "@src" starts at byte 7.
    let cursor = text.len(); // end of input
    let t = detect(text, cursor).expect("trigger detected after CJK prefix");
    assert_eq!(t.kind, SuggestionKind::At);
    assert_eq!(t.pos, 7, "trigger pos must be byte 7 (not char 3)");
    assert_eq!(t.query, "src");
}

#[test]
fn test_refresh_installs_slash_suggestions() {
    let mut state = AppState::new();
    state.session.available_commands = vec![
        slash("help", Some("Show help")),
        slash("clear", None),
        slash("config", Some("Edit settings")),
    ];
    state.ui.input.textarea.set_text("/c");
    state.ui.input.textarea.set_cursor(2);

    refresh_suggestions(&mut state);

    let sug = state.ui.completion.active.expect("popup installed");
    assert_eq!(sug.kind, SuggestionKind::SlashCommand);
    assert_eq!(sug.query, "c");
    let labels: Vec<&str> = sug.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, vec!["/clear", "/config"]);
}

#[test]
fn test_refresh_installs_mid_input_slash_ghost() {
    let mut state = AppState::new();
    state.session.available_commands = vec![slash("clear", None)];
    state.ui.input.textarea.set_text("then /cl");
    state.ui.input.textarea.set_cursor("then /cl".len());

    refresh_suggestions(&mut state);

    assert!(state.ui.completion.active.is_none());
    let ghost = state
        .ui
        .input
        .active_inline_ghost()
        .expect("mid-input slash ghost");
    assert_eq!(ghost.text, "ear");
    assert_eq!(ghost.replacement, "ear");
}

#[test]
fn test_refresh_leading_slash_uses_popup_not_ghost() {
    let mut state = AppState::new();
    state.session.available_commands = vec![slash("clear", None)];
    state.ui.input.textarea.set_text("/cl");
    state.ui.input.textarea.set_cursor(3);

    refresh_suggestions(&mut state);

    assert!(state.ui.completion.active.is_some());
    assert!(state.ui.input.active_inline_ghost().is_none());
}

#[test]
fn at_popup_retains_prior_items_across_keystroke_until_async_lands() {
    // Flicker regression: typing another character of the same `@` token must
    // NOT blank the popup while the debounced file search is still pending.
    // The previously-shown rows are retained until apply_async_result swaps in
    // the new results, eliminating the per-keystroke empty frame.
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@src/fo");
    state.ui.input.textarea.set_cursor("@src/fo".len());
    refresh_suggestions(&mut state);

    // Simulate the FileSearchManager result for the first query landing.
    {
        let active = state
            .ui
            .completion
            .active
            .as_mut()
            .expect("async @ popup installed");
        active.items = vec![SuggestionItem {
            label: "src/foo.rs".to_string(),
            description: None,
            metadata: Some(SuggestionMeta::Path {
                is_directory: false,
            }),
        }];
    }

    // User types one more character of the SAME @-token.
    state.ui.input.textarea.set_text("@src/foo");
    state.ui.input.textarea.set_cursor("@src/foo".len());
    refresh_suggestions(&mut state);

    let active = state
        .ui
        .completion
        .active
        .as_ref()
        .expect("popup must stay mounted, not blank");
    assert_eq!(
        active
            .items
            .iter()
            .map(|i| i.label.as_str())
            .collect::<Vec<_>>(),
        vec!["src/foo.rs"],
        "prior file rows must be retained across the keystroke, not cleared to empty"
    );
}

#[test]
fn test_refresh_installs_shell_history_ghost() {
    let mut state = AppState::new();
    state.ui.input.add_to_history("!cargo test".into());
    state.ui.input.textarea.set_text("!cargo");
    state.ui.input.textarea.set_cursor("!cargo".len());

    refresh_suggestions(&mut state);

    let ghost = state
        .ui
        .input
        .active_inline_ghost()
        .expect("shell history ghost");
    assert_eq!(ghost.text, " test");
}

#[test]
fn test_refresh_dismisses_when_space_typed() {
    let mut state = AppState::new();
    state.session.available_commands = vec![slash("help", None)];
    state.ui.input.textarea.set_text("/help");
    state.ui.input.textarea.set_cursor(5);
    refresh_suggestions(&mut state);
    assert!(state.ui.completion.active.is_some());

    state.ui.input.textarea.set_text("/help ");
    state.ui.input.textarea.set_cursor(6);
    refresh_suggestions(&mut state);
    assert!(state.ui.completion.active.is_none());
}

#[test]
fn test_refresh_seeds_at_trigger_with_agents() {
    // The unified `@` trigger seeds the popup with agent matches
    // synchronously; async file results merge in later via
    // `apply_async_result`. The empty-query case shows every agent.
    let mut state = AppState::new();
    state.session.available_agents = vec![
        crate::autocomplete::AgentInfo {
            name: "plan".into(),
            description: Some("Planning agent".into()),
            color: None,
        },
        crate::autocomplete::AgentInfo {
            name: "review".into(),
            description: None,
            color: None,
        },
    ];
    state.ui.input.textarea.set_text("@pl");
    state.ui.input.textarea.set_cursor(3);
    refresh_suggestions(&mut state);

    let sug = state.ui.completion.active.expect("popup installed");
    assert_eq!(sug.kind, SuggestionKind::At);
    assert_eq!(sug.query, "pl");
    let labels: Vec<&str> = sug.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, vec!["plan (agent)"]);
}

#[test]
fn test_apply_async_result_merges_files_after_agents() {
    let mut state = AppState::new();
    state.session.available_agents = vec![crate::autocomplete::AgentInfo {
        name: "src".into(),
        description: Some("Source explorer".into()),
        color: None,
    }];
    state.ui.input.textarea.set_text("@src");
    state.ui.input.textarea.set_cursor(4);
    refresh_suggestions(&mut state);
    // Popup seeded with 1 agent already.
    assert_eq!(state.ui.completion.active.as_ref().unwrap().items.len(), 1);

    let file_results = vec![
        SuggestionItem {
            label: "src/lib.rs".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path {
                is_directory: false,
            }),
        },
        SuggestionItem {
            label: "src/main.rs".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path {
                is_directory: false,
            }),
        },
    ];
    let adopted = apply_async_result(&mut state, SuggestionKind::At, "src", file_results);
    assert!(adopted);

    let sug = state.ui.completion.active.as_ref().unwrap();
    // Agent first, then both files (merge preserves order, cap 15).
    assert_eq!(sug.items.len(), 3);
    assert!(matches!(
        sug.items[0].metadata,
        Some(SuggestionMeta::Agent { .. })
    ));
    assert!(matches!(
        sug.items[1].metadata,
        Some(SuggestionMeta::Path { .. })
    ));
}

#[test]
fn test_apply_async_result_merges_directory_rows_after_agents() {
    let mut state = AppState::new();
    state.session.available_agents = vec![crate::autocomplete::AgentInfo {
        name: "src".into(),
        description: Some("Source explorer".into()),
        color: None,
    }];
    state.ui.input.textarea.set_text("@src");
    state.ui.input.textarea.set_cursor(4);
    refresh_suggestions(&mut state);

    let file_results = vec![
        SuggestionItem {
            label: "src/".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path { is_directory: true }),
        },
        SuggestionItem {
            label: "src/main.rs".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path {
                is_directory: false,
            }),
        },
    ];
    let adopted = apply_async_result(&mut state, SuggestionKind::At, "src", file_results);
    assert!(adopted);

    let sug = state.ui.completion.active.as_ref().unwrap();
    assert_eq!(
        sug.items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["src (agent)", "src/", "src/main.rs"]
    );
    assert!(matches!(
        sug.items[1].metadata,
        Some(SuggestionMeta::Path { is_directory: true })
    ));
}

#[test]
fn test_apply_async_result_drops_stale_query() {
    // User moved on to a new query before the search returned. Slow
    // result must not clobber the current trigger state.
    let mut state = AppState::new();
    state.ui.completion.set_active(
        ActiveSuggestions {
            kind: SuggestionKind::At,
            items: Vec::new(),
            selected: 0,
            query: "docs".into(),
            trigger_pos: 0,
        },
        0..5,
        "@docs".into(),
    );

    let adopted = apply_async_result(
        &mut state,
        SuggestionKind::At,
        "src", // stale — user has moved to "docs"
        vec![SuggestionItem {
            label: "src/lib.rs".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path {
                is_directory: false,
            }),
        }],
    );
    assert!(!adopted);
    assert!(
        state
            .ui
            .completion
            .active
            .as_ref()
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn test_apply_async_result_drops_when_dismissed() {
    // User dismissed the popup (Esc) before result came back.
    let mut state = AppState::new();
    state.ui.completion.active = None;

    let adopted = apply_async_result(
        &mut state,
        SuggestionKind::Symbol,
        "foo",
        vec![SuggestionItem {
            label: "foo".into(),
            description: None,
            metadata: None,
        }],
    );
    assert!(!adopted);
    assert!(state.ui.completion.active.is_none());
}

#[test]
fn test_explicit_at_path_uses_path_kind_and_drops_fuzzy_result() {
    let dir = temp_completion_dir("explicit-at-path");
    std::fs::write(dir.join("alpha.txt"), "").expect("write path suggestion file");
    let input = format!("@{}/a", dir.display());

    let mut state = AppState::new();
    state.ui.input.textarea.set_text(&input);
    state.ui.input.textarea.set_cursor(input.len());
    refresh_suggestions(&mut state);

    let sug = state.ui.completion.active.as_ref().expect("path request");
    assert_eq!(sug.kind, SuggestionKind::Path);
    assert!(sug.items.is_empty(), "path provider runs asynchronously");
    let query = sug.query.clone();

    let adopted = apply_async_result(
        &mut state,
        SuggestionKind::At,
        &query,
        vec![SuggestionItem {
            label: "fuzzy-result.rs".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path {
                is_directory: false,
            }),
        }],
    );
    assert!(!adopted);
    assert!(
        state
            .ui
            .completion
            .active
            .as_ref()
            .unwrap()
            .items
            .is_empty()
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_same_query_different_token_range_gets_fresh_request_key() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@src then @src");
    state.ui.input.textarea.set_cursor("@src".len());
    refresh_suggestions(&mut state);
    let first = state
        .ui
        .completion
        .active_key
        .clone()
        .expect("first request key");

    state.ui.input.textarea.set_cursor("@src then @src".len());
    refresh_suggestions(&mut state);
    let second = state
        .ui
        .completion
        .active_key
        .clone()
        .expect("second request key");

    assert_eq!(first.kind, second.kind);
    assert_eq!(first.query, second.query);
    assert_ne!(first.token_range, second.token_range);
    assert_ne!(first.generation, second.generation);
}

#[test]
fn test_keyed_async_result_drops_stale_same_query_different_range() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@src then @src");
    state.ui.input.textarea.set_cursor("@src".len());
    refresh_suggestions(&mut state);
    let first = state
        .ui
        .completion
        .active_key
        .clone()
        .expect("first request key");

    state.ui.input.textarea.set_cursor("@src then @src".len());
    refresh_suggestions(&mut state);
    let second = state
        .ui
        .completion
        .active_key
        .clone()
        .expect("second request key");

    let stale = vec![SuggestionItem {
        label: "first/src.rs".into(),
        description: None,
        metadata: Some(SuggestionMeta::Path {
            is_directory: false,
        }),
    }];
    assert!(!apply_async_result_for_key(&mut state, &first, stale));

    let fresh = vec![SuggestionItem {
        label: "second/src.rs".into(),
        description: None,
        metadata: Some(SuggestionMeta::Path {
            is_directory: false,
        }),
    }];
    assert!(apply_async_result_for_key(&mut state, &second, fresh));
    let labels = state
        .ui
        .completion
        .active
        .as_ref()
        .expect("active suggestions")
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(labels, vec!["second/src.rs"]);
}

#[test]
fn test_explicit_at_path_prefixes_use_path_kind() {
    for input in ["@~/", "@./", "@../", "@/"] {
        let mut state = AppState::new();
        state.ui.input.textarea.set_text(input);
        state.ui.input.textarea.set_cursor(input.len());

        refresh_suggestions(&mut state);

        let sug = state
            .ui
            .completion
            .active
            .as_ref()
            .expect("path completion");
        assert_eq!(sug.kind, SuggestionKind::Path, "input: {input}");
    }
}

#[test]
fn test_quoted_at_path_keeps_spaces_inside_token() {
    let input = "@\"/tmp/my project/s";
    let t = detect(input, input.len()).expect("quoted path trigger");

    assert_eq!(t.kind, SuggestionKind::At);
    assert_eq!(t.pos, 0);
    assert_eq!(t.query, "\"/tmp/my project/s");
}

#[test]
fn test_directory_command_detection_uses_typed_argument_kind() {
    let dir = temp_completion_dir("typed-directory");
    std::fs::create_dir_all(dir.join("src")).expect("create child directory");
    let input = format!("/scope {}/s", dir.display());

    let mut state = AppState::new();
    state.session.available_commands = vec![SlashCommandInfo {
        name: "scope".into(),
        argument_kind: CommandArgumentKind::DirectoryPath,
        ..SlashCommandInfo::default()
    }];
    state.ui.input.textarea.set_text(&input);
    state.ui.input.textarea.set_cursor(input.len());
    refresh_suggestions(&mut state);

    let sug = state
        .ui
        .completion
        .active
        .as_ref()
        .expect("directory request");
    assert_eq!(sug.kind, SuggestionKind::Directory);
    assert!(sug.items.is_empty(), "path provider runs asynchronously");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_command_detection_uses_files_and_directories_path_kind() {
    let dir = temp_completion_dir("typed-file");
    std::fs::write(dir.join("src.txt"), "").expect("create child file");
    let input = format!("/open {}/s", dir.display());

    let mut state = AppState::new();
    state.session.available_commands = vec![SlashCommandInfo {
        name: "open".into(),
        argument_kind: CommandArgumentKind::FilePath,
        ..SlashCommandInfo::default()
    }];
    state.ui.input.textarea.set_text(&input);
    state.ui.input.textarea.set_cursor(input.len());
    refresh_suggestions(&mut state);

    let sug = state
        .ui
        .completion
        .active
        .as_ref()
        .expect("file path request");
    assert_eq!(sug.kind, SuggestionKind::Path);
    assert!(sug.items.is_empty(), "path provider runs asynchronously");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_mcp_resources_are_seeded_with_server_metadata() {
    let mut state = AppState::new();
    state.session.available_mcp_resources = vec![crate::state::McpResourceCompletion {
        server: "docs-server".into(),
        uri: "file://guide".into(),
        name: "Guide".into(),
        description: Some("Project guide".into()),
    }];
    state.ui.input.textarea.set_text("@guide");
    state.ui.input.textarea.set_cursor("@guide".len());

    refresh_suggestions(&mut state);

    let sug = state.ui.completion.active.as_ref().expect("mcp popup");
    assert_eq!(sug.items[0].label, "Guide");
    assert!(matches!(
        sug.items[0].metadata.as_ref(),
        Some(SuggestionMeta::McpResource { server, uri })
            if server == "docs-server" && uri == "file://guide"
    ));
}

#[test]
fn test_slack_channel_source_defaults_to_no_rows() {
    let state = AppState::new();
    assert!(state.session.available_slack_channels.is_empty());
}

#[test]
fn test_resume_command_detection_uses_session_id_argument_kind() {
    let mut state = AppState::new();
    state.session.available_commands = vec![SlashCommandInfo {
        name: "resume".into(),
        argument_kind: CommandArgumentKind::SessionId,
        ..SlashCommandInfo::default()
    }];
    state.session.saved_sessions = vec![SavedSession {
        id: "session-123".into(),
        label: "Auth refactor".into(),
        message_count: 5,
        created_at: "today".into(),
        model: None,
    }];
    state.ui.input.textarea.set_text("/resume auth");
    state.ui.input.textarea.set_cursor("/resume auth".len());

    refresh_suggestions(&mut state);

    let sug = state.ui.completion.active.as_ref().expect("resume popup");
    assert_eq!(sug.kind, SuggestionKind::CustomTitle);
    assert_eq!(sug.items[0].label, "session-123");
    assert_eq!(sug.items[0].description.as_deref(), Some("Auth refactor"));
}

#[test]
fn test_directory_command_detection_ignores_hint_without_typed_kind() {
    let dir = temp_completion_dir("hint-no-directory");
    let input = format!("/add-dir {}/", dir.display());

    let mut state = AppState::new();
    state.session.available_commands = vec![SlashCommandInfo {
        name: "add-dir".into(),
        argument_hint: Some("<path>".into()),
        argument_kind: CommandArgumentKind::FreeText,
        ..SlashCommandInfo::default()
    }];
    state.ui.input.textarea.set_text(&input);
    state.ui.input.textarea.set_cursor(input.len());
    refresh_suggestions(&mut state);

    assert!(state.ui.completion.active.is_none());

    let _ = std::fs::remove_dir_all(dir);
}
