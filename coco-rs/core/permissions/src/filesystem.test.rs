use super::*;

// ── contains_path_traversal ──

#[test]
fn test_traversal_simple() {
    assert!(contains_path_traversal("../etc/passwd"));
    assert!(contains_path_traversal("foo/../bar"));
    assert!(contains_path_traversal("foo/.."));
}

#[test]
fn test_traversal_windows() {
    assert!(contains_path_traversal("foo\\..\\bar"));
}

#[test]
fn test_no_traversal() {
    assert!(!contains_path_traversal("foo/bar/baz"));
    assert!(!contains_path_traversal("foo..bar"));
    assert!(!contains_path_traversal("..foo"));
    assert!(!contains_path_traversal("/absolute/path"));
}

// ── is_dangerous_file_path ──

#[test]
fn test_dangerous_dotfiles() {
    assert!(is_dangerous_file_path("/home/user/.bashrc"));
    assert!(is_dangerous_file_path("/home/user/.gitconfig"));
    assert!(is_dangerous_file_path("project/.mcp.json"));
}

#[test]
fn test_dangerous_directories() {
    assert!(is_dangerous_file_path("/project/.git/config"));
    assert!(is_dangerous_file_path("/project/.Git/hooks/pre-commit"));
    assert!(is_dangerous_file_path("/project/.claude/settings.json"));
    assert!(is_dangerous_file_path("/project/.vscode/settings.json"));
}

#[test]
fn test_coco_worktrees_allowed() {
    assert!(!is_dangerous_file_path(
        "/project/.coco/worktrees/feature/src/main.rs"
    ));
    assert!(!is_dangerous_file_path(
        "/project/.coco/worktrees/feat/lib/util.rs"
    ));
}

#[test]
fn test_nested_coco_under_worktree_still_dangerous() {
    // P12b: the `.coco/worktrees` exemption is component-anchored, so a
    // SECOND nested `.coco` (a settings file inside the worktree) is still
    // blocked even though an earlier `.coco/worktrees/` segment exists.
    assert!(is_dangerous_file_path(
        "/project/.coco/worktrees/feature/.coco/settings.json"
    ));
}

#[test]
fn test_worktrees_not_under_coco_not_exempt() {
    // `.coco` followed by `config` (not `worktrees`) blocks.
    assert!(is_dangerous_file_path(
        "/project/.coco/config/worktrees/x.rs"
    ));
    // Plain `.coco` segment still blocks.
    assert!(is_dangerous_file_path("/project/.coco/settings.json"));
}

#[test]
fn test_safe_paths() {
    assert!(!is_dangerous_file_path("/project/src/main.rs"));
    assert!(!is_dangerous_file_path("/project/Cargo.toml"));
    assert!(!is_dangerous_file_path("/project/README.md"));
}

#[test]
fn test_unc_paths_blocked() {
    assert!(is_dangerous_file_path("\\\\server\\share\\file"));
    assert!(is_dangerous_file_path("//server/share/file"));
}

// ── is_coco_config_path ──

#[test]
fn test_coco_config_detected() {
    assert!(is_coco_config_path("/project/.coco/settings.json"));
    assert!(is_coco_config_path("/project/.coco/settings.local.json"));
    assert!(is_coco_config_path("/project/.coco/skills/foo/SKILL.md"));
    assert!(is_coco_config_path("/project/.coco/agents/reviewer.md"));
    assert!(is_coco_config_path("/project/.coco/commands/foo.md"));
    assert!(is_coco_config_path("/project/.Coco/Settings.json"));
}

#[test]
fn test_legacy_claude_paths_are_not_coco_config() {
    // coco serves all config from `.coco/`; legacy `.claude/` paths are NOT
    // coco config paths. (Writes under `.claude/` are still gated by the
    // dangerous-directory check, just not by the config-edit gate.)
    assert!(!is_coco_config_path("/project/.claude/settings.json"));
    assert!(!is_coco_config_path("/project/.claude/agents/reviewer.md"));
    assert!(!is_coco_config_path("/project/.claude/skills/foo/SKILL.md"));
}

#[test]
fn test_coco_commands_is_config_path() {
    // `.coco/commands/` counts as a config path requiring approval — commands,
    // agents, skills, and settings under `.coco/` are all treated alike.
    assert!(is_coco_config_path("/project/.coco/commands/foo.md"));
    assert!(is_coco_config_path("/project/.coco/agents/reviewer.md"));
    assert!(is_coco_config_path("/project/.coco/settings.json"));
    assert!(is_coco_config_path("/project/.coco/skills/foo/SKILL.md"));
    assert!(!is_coco_config_path("/project/src/config.json"));
}

#[test]
fn test_check_path_safety_blocks_coco_commands() {
    assert!(matches!(
        check_path_safety_for_auto_edit("/project/.coco/commands/run.md"),
        PathSafetyResult::Blocked {
            classifier_approvable: true,
            ..
        }
    ));
}

#[test]
fn test_non_coco_config() {
    assert!(!is_coco_config_path("/project/src/config.json"));
}

// ── has_suspicious_windows_pattern ──

#[test]
fn test_ads_colon_is_windows_only() {
    // Colons are valid filename characters on Linux/macOS, so the NTFS-ADS
    // colon check is Windows-only (#74). On Unix a legitimate colon path must
    // NOT be flagged; on Windows the ADS pattern still is.
    if cfg!(target_os = "windows") {
        assert!(has_suspicious_windows_pattern("C:\\file.txt::$DATA"));
        assert!(has_suspicious_windows_pattern("C:\\settings.json:stream"));
    } else {
        assert!(!has_suspicious_windows_pattern("/tmp/log:2026-06-01.txt"));
        assert!(!has_suspicious_windows_pattern("/tmp/settings.json:stream"));
    }
}

#[test]
fn test_short_name_detected() {
    assert!(has_suspicious_windows_pattern("C:\\GIT~1\\config"));
    assert!(has_suspicious_windows_pattern("CLAUDE~1"));
}

#[test]
fn test_long_prefix_detected() {
    assert!(has_suspicious_windows_pattern("\\\\?\\C:\\long\\path"));
    assert!(has_suspicious_windows_pattern("//?/C:/path"));
}

#[test]
fn test_trailing_dots() {
    assert!(has_suspicious_windows_pattern(".git."));
    assert!(has_suspicious_windows_pattern("settings.json "));
}

#[test]
fn test_dos_device_names() {
    assert!(has_suspicious_windows_pattern("file.CON"));
    assert!(has_suspicious_windows_pattern("data.PRN"));
    assert!(has_suspicious_windows_pattern("test.COM1"));
}

#[test]
fn test_triple_dots() {
    assert!(has_suspicious_windows_pattern(".../file.txt"));
    assert!(has_suspicious_windows_pattern("path/.../file"));
}

#[test]
fn test_normal_path_not_suspicious() {
    assert!(!has_suspicious_windows_pattern("src/main.rs"));
    assert!(!has_suspicious_windows_pattern("/usr/local/bin/test"));
}

// ── check_path_safety_for_auto_edit ──

#[test]
fn test_safe_path_for_auto_edit() {
    assert!(matches!(
        check_path_safety_for_auto_edit("src/main.rs"),
        PathSafetyResult::Safe
    ));
}

#[test]
fn test_traversal_blocked_not_approvable() {
    assert!(matches!(
        check_path_safety_for_auto_edit("../../../etc/passwd"),
        PathSafetyResult::Blocked {
            classifier_approvable: false,
            ..
        }
    ));
}

#[test]
fn test_dangerous_file_classifier_approvable() {
    assert!(matches!(
        check_path_safety_for_auto_edit("/home/user/.bashrc"),
        PathSafetyResult::Blocked {
            classifier_approvable: true,
            ..
        }
    ));
}

#[test]
fn test_coco_config_classifier_approvable() {
    assert!(matches!(
        check_path_safety_for_auto_edit("/project/.coco/settings.json"),
        PathSafetyResult::Blocked {
            classifier_approvable: true,
            ..
        }
    ));
}

// ── path_in_working_path ──

#[test]
fn test_path_in_working_path_relative() {
    assert!(path_in_working_path("src/main.rs", "/project"));
}

#[test]
fn test_path_in_working_path_absolute() {
    assert!(path_in_working_path("/project/src/main.rs", "/project"));
}

#[test]
fn test_path_in_working_path_outside() {
    assert!(!path_in_working_path("/etc/passwd", "/project"));
}

#[test]
fn test_path_in_working_path_exact() {
    assert!(path_in_working_path("/project", "/project"));
}

#[test]
fn test_path_in_working_path_macos_private() {
    assert!(path_in_working_path("/private/tmp/test", "/tmp"));
}

// ── is_path_within_allowed_dirs ──

#[test]
fn test_path_within_cwd() {
    assert!(is_path_within_allowed_dirs(
        "src/main.rs",
        "/home/user/project",
        &[]
    ));
}

#[test]
fn test_bare_tmp_not_allowed() {
    // P12a: a bare /tmp path is NO LONGER blanket-allowed — it must fall to the
    // cwd/additional-dir gate (the /tmp/claude-* project-temp exemption lives in
    // is_readable_internal_path / is_editable_internal_path instead).
    assert!(!is_path_within_allowed_dirs(
        "/tmp/test.txt",
        "/home/user/project",
        &[]
    ));
    // But /tmp passed explicitly as an additional working dir IS allowed.
    assert!(is_path_within_allowed_dirs(
        "/tmp/work/f",
        "/home/user/project",
        &["/tmp/work".to_string()]
    ));
}

#[test]
fn test_arbitrary_tmp_write_gated() {
    // P12a end-to-end: an arbitrary /tmp write outside cwd/additional dirs now
    // reports as outside allowed directories rather than silently passing.
    let r = validate_write_path("/tmp/evil.sh", "/home/user/project", &[]);
    assert!(r.is_some());
    assert!(r.unwrap().contains("outside allowed directories"));
}

#[test]
fn test_path_outside_cwd() {
    assert!(!is_path_within_allowed_dirs(
        "/etc/passwd",
        "/home/user/project",
        &[]
    ));
}

#[test]
fn test_path_with_additional_dir() {
    assert!(is_path_within_allowed_dirs(
        "/opt/data/file.txt",
        "/home/user",
        &["/opt/data".to_string()]
    ));
}

// ── validate_write_path ──

#[test]
fn test_validate_write_system_blocked() {
    let result = validate_write_path("/etc/shadow", "/home/user", &[]);
    assert!(result.is_some());
    assert!(result.unwrap().contains("system directory"));
}

#[test]
fn test_validate_write_safe() {
    assert!(validate_write_path("src/lib.rs", "/home/user/project", &[]).is_none());
}

#[test]
fn test_validate_write_traversal_blocked() {
    let result = validate_write_path("../../etc/passwd", "/home/user", &[]);
    assert!(result.is_some());
    assert!(result.unwrap().contains("traversal"));
}

// ── is_scratchpad_dir ──

#[test]
fn test_is_scratchpad() {
    assert!(is_scratchpad_dir("/tmp/work"));
    assert!(is_scratchpad_dir("/home/user/.cache/data"));
    assert!(!is_scratchpad_dir("/home/user/project"));
}

// ── has_dangerous_tilde ──

#[test]
fn test_safe_tilde() {
    assert!(!has_dangerous_tilde("~/Documents/file.txt"));
    assert!(!has_dangerous_tilde("~"));
    assert!(!has_dangerous_tilde("/no/tilde/here"));
}

#[test]
fn test_dangerous_tilde_user() {
    // ~user expands to that user's home — TOCTOU risk
    assert!(has_dangerous_tilde("~root/.bashrc"));
    assert!(has_dangerous_tilde("~admin/secrets"));
}

#[test]
fn test_dangerous_tilde_special() {
    // ~+ = $PWD, ~- = $OLDPWD in bash
    assert!(has_dangerous_tilde("~+/file"));
    assert!(has_dangerous_tilde("~-/file"));
    assert!(has_dangerous_tilde("~1/file")); // directory stack
}

// ── has_shell_expansion ──

#[test]
fn test_shell_expansion_dollar() {
    assert!(has_shell_expansion("$HOME/file.txt"));
    assert!(has_shell_expansion("${HOME}/file.txt"));
    assert!(has_shell_expansion("$(whoami)/file.txt"));
    assert!(has_shell_expansion("/path/$USER/data"));
}

#[test]
fn test_shell_expansion_backtick() {
    assert!(has_shell_expansion("`pwd`/file.txt"));
}

#[test]
fn test_shell_expansion_windows_percent() {
    assert!(has_shell_expansion("%USERPROFILE%\\Desktop"));
    assert!(has_shell_expansion("%HOME%/file"));
}

#[test]
fn test_shell_expansion_zsh_equals() {
    // Zsh: =rg expands to /usr/bin/rg
    assert!(has_shell_expansion("=rg"));
    assert!(has_shell_expansion("=python"));
    // Not expansion: = alone or =123
    assert!(!has_shell_expansion("="));
    assert!(!has_shell_expansion("=123"));
}

#[test]
fn test_no_shell_expansion() {
    assert!(!has_shell_expansion("/home/user/file.txt"));
    assert!(!has_shell_expansion("src/main.rs"));
    assert!(!has_shell_expansion("~/Documents"));
    // Single % is not expansion
    assert!(!has_shell_expansion("100%done"));
}

// ── check_path_safety_for_auto_edit with new checks ──

#[test]
fn test_safety_blocks_shell_expansion() {
    assert!(matches!(
        check_path_safety_for_auto_edit("$HOME/.bashrc"),
        PathSafetyResult::Blocked {
            classifier_approvable: false,
            ..
        }
    ));
}

#[test]
fn test_safety_blocks_dangerous_tilde() {
    assert!(matches!(
        check_path_safety_for_auto_edit("~root/.ssh/id_rsa"),
        PathSafetyResult::Blocked {
            classifier_approvable: false,
            ..
        }
    ));
}

// ── get_paths_for_permission_check ──

#[test]
fn test_paths_for_permission_check_simple() {
    // Non-symlink path should return at least the original
    let paths = get_paths_for_permission_check("src/main.rs", "/project");
    assert!(paths.contains(&"/project/src/main.rs".to_string()));
}

#[test]
fn test_paths_for_permission_check_unc_blocked() {
    let paths = get_paths_for_permission_check("//server/share", "/project");
    assert_eq!(paths.len(), 1, "UNC should return only original");
}

// ── is_dangerous_removal_path ──

#[test]
fn test_dangerous_removal_root() {
    assert!(is_dangerous_removal_path("/"));
    assert!(is_dangerous_removal_path("~"));
    assert!(is_dangerous_removal_path("~/"));
}

#[test]
fn test_dangerous_removal_wildcard() {
    assert!(is_dangerous_removal_path("*"));
    assert!(is_dangerous_removal_path("/*"));
    assert!(is_dangerous_removal_path("/tmp/*"));
}

#[test]
fn test_dangerous_removal_root_children() {
    assert!(is_dangerous_removal_path("/usr"));
    assert!(is_dangerous_removal_path("/etc"));
    assert!(is_dangerous_removal_path("/home"));
}

#[test]
fn test_safe_removal_paths() {
    assert!(!is_dangerous_removal_path("/tmp/test/file.txt"));
    assert!(!is_dangerous_removal_path("/home/user/project/build"));
    assert!(!is_dangerous_removal_path("src/old.rs"));
}

#[test]
fn test_dangerous_removal_windows_drive() {
    assert!(is_dangerous_removal_path("C:\\"));
    assert!(is_dangerous_removal_path("C:"));
    assert!(is_dangerous_removal_path("C:\\Windows"));
}

// ── is_editable_internal_path ──

#[test]
fn test_editable_plan_files() {
    // Plan-file writes are keyed on the resolved session plan file (cocohome
    // by default), NOT a `.coco/plans` substring. The session file and its
    // `-agent-*` variant are allowed; a different slug and a non-`.md` are not.
    let plan = std::path::PathBuf::from("/home/u/.coco/plans/typed-conjuring-fox.md");
    let ctx = InternalPathContext {
        cwd: "/project",
        session_plan_file: Some(&plan),
    };
    assert!(is_editable_internal_path(
        "/home/u/.coco/plans/typed-conjuring-fox.md",
        &ctx
    ));
    assert!(is_editable_internal_path(
        "/home/u/.coco/plans/typed-conjuring-fox-agent-7.md",
        &ctx
    ));
    assert!(!is_editable_internal_path(
        "/home/u/.coco/plans/some-other-slug.md",
        &ctx
    ));
    assert!(!is_editable_internal_path(
        "/home/u/.coco/plans/typed-conjuring-fox.txt",
        &ctx
    ));

    // Without a resolved session plan file there is no plan carve-out.
    let none_ctx = InternalPathContext {
        cwd: "/project",
        session_plan_file: None,
    };
    assert!(!is_editable_internal_path(
        "/home/u/.coco/plans/typed-conjuring-fox.md",
        &none_ctx
    ));
}

#[test]
fn test_editable_plan_file_traversal_blocked() {
    // A `..` escape that string-prefixes the slug must NOT slip through:
    // resolve_path collapses the traversal before the prefix check.
    let plan = std::path::PathBuf::from("/home/u/.coco/plans/typed-conjuring-fox.md");
    let ctx = InternalPathContext {
        cwd: "/project",
        session_plan_file: Some(&plan),
    };
    assert!(!is_editable_internal_path(
        "/home/u/.coco/plans/typed-conjuring-fox/../../../etc/passwd.md",
        &ctx
    ));
}

// ── is_readable_internal_path ──

#[test]
fn test_readable_plan_files() {
    let plan = std::path::PathBuf::from("/home/u/.coco/plans/design-slug.md");
    let ctx = InternalPathContext {
        cwd: "/project",
        session_plan_file: Some(&plan),
    };
    assert!(is_readable_internal_path(
        "/home/u/.coco/plans/design-slug.md",
        &ctx
    ));
    assert!(!is_readable_internal_path(
        "/home/u/.coco/plans/unrelated.md",
        &ctx
    ));
}

// ── DANGEROUS_DIRECTORIES (agent config homes) ──

#[test]
fn test_agent_config_dirs_are_dangerous() {
    // Coco guards its own config home plus the claude/codex dirs it reads for
    // compat. `.coco/worktrees/` is the one structural exemption.
    assert!(is_dangerous_file_path("/project/.coco/settings.json"));
    assert!(is_dangerous_file_path("/project/.codex/config.toml"));
    assert!(is_dangerous_file_path("/project/.claude/settings.json"));
    assert!(!is_dangerous_file_path(
        "/repo/.coco/worktrees/agent-x/src/main.rs"
    ));
    assert!(!is_dangerous_file_path("/project/src/main.rs"));
}
