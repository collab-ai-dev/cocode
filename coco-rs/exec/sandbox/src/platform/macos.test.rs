use std::path::PathBuf;

use crate::config::EnforcementLevel;
use crate::config::SandboxConfig;
use crate::config::WritableRoot;

use super::*;

#[test]
fn test_macos_sandbox_available() {
    let sandbox = MacOsSandbox;
    if cfg!(target_os = "macos") {
        assert!(sandbox.available());
    } else {
        assert!(!sandbox.available());
    }
}

// ==========================================================================
// SBPL path escaping
// ==========================================================================

#[test]
fn test_escape_sbpl_path_normal() {
    assert_eq!(escape_sbpl_path("/home/user/project"), "/home/user/project");
}

#[test]
fn test_escape_sbpl_path_quotes() {
    assert_eq!(escape_sbpl_path(r#"/tmp/a"b"#), r#"/tmp/a\"b"#);
}

#[test]
fn test_escape_sbpl_path_backslash() {
    assert_eq!(escape_sbpl_path(r"/tmp/a\b"), r"/tmp/a\\b");
}

#[test]
fn test_escape_sbpl_path_newlines_stripped() {
    assert_eq!(escape_sbpl_path("/tmp/a\nb"), "/tmp/ab");
    assert_eq!(escape_sbpl_path("/tmp/a\r\nb"), "/tmp/ab");
}

#[test]
fn test_escape_sbpl_path_injection_attempt() {
    // Attempt to inject a new SBPL rule via path
    let malicious = "/tmp\"))\n(allow network*)\n(allow file-write* (subpath \"/etc";
    let escaped = escape_sbpl_path(malicious);
    assert!(!escaped.contains('\n'));
    assert!(!escaped.contains('"') || escaped.contains("\\\""));
}

// ==========================================================================
// Profile generation
// ==========================================================================

#[test]
fn test_generate_seatbelt_profile_includes_base_policy() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::ReadOnly,
        writable_roots: vec![],
        denied_paths: vec![],
        allow_network: false,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "echo hello", "_test_SBX", &[]);
    // Version header and deny-default with command tag
    assert!(profile.contains("(version 1)"));
    assert!(profile.contains("(deny default (with message \"CMD64_"));
    assert!(profile.contains("_END_test_SBX"));
    // Base policy includes Chrome-inspired rules
    assert!(profile.contains("(allow process-exec)"));
    assert!(profile.contains("(allow process-fork)"));
    assert!(profile.contains("(allow signal (target same-sandbox))"));
    assert!(profile.contains("(allow process-info* (target same-sandbox))"));
    // PTY allowed by default config
    assert!(profile.contains("(allow pseudo-tty)"));
    assert!(profile.contains("(allow ipc-posix-sem)"));
    // Aligned with Claude Code: mach-lookup entries
    assert!(profile.contains("com.apple.FontObjectsServer"));
    assert!(profile.contains("com.apple.logd"));
    assert!(profile.contains("com.apple.SecurityServer"));
    // POSIX shared memory
    assert!(profile.contains("(allow ipc-posix-shm)"));
    // Granular sysctl whitelist
    assert!(profile.contains("hw.activecpu"));
    assert!(profile.contains("kern.hostname"));
}

#[test]
fn test_generate_seatbelt_profile_readonly_no_writable_roots() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::ReadOnly,
        writable_roots: vec![],
        denied_paths: vec![],
        allow_network: false,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "test cmd", "_test_SBX", &[]);
    assert!(profile.contains("(allow file-read* (subpath \"/usr\"))"));
    assert!(!profile.contains("Writable roots"));
    // Network restricted to loopback
    assert!(profile.contains("localhost"));
    // Should NOT include network policy (no allow network*)
    assert!(!profile.contains("com.apple.trustd.agent"));
}

#[test]
fn test_generate_seatbelt_profile_with_writable_roots() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        writable_roots: vec![WritableRoot::new("/home/user/project")],
        denied_paths: vec![],
        allow_network: true,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "test cmd", "_test_SBX", &[]);
    assert!(profile.contains("(allow file-write* (subpath \"/home/user/project\"))"));
    // Protected subpaths denied for writes
    assert!(profile.contains("(deny file-write* (subpath \"/home/user/project/.git\"))"));
    assert!(profile.contains("(deny file-write* (subpath \"/home/user/project/.coco\"))"));
    assert!(profile.contains("(deny file-write* (subpath \"/home/user/project/.agents\"))"));
    // Network fully allowed + TLS services
    assert!(profile.contains("(allow network*)"));
    assert!(profile.contains("com.apple.trustd.agent"));
}

#[test]
fn test_generate_seatbelt_profile_strict_no_network() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::Strict,
        writable_roots: vec![WritableRoot::new("/tmp/sandbox")],
        denied_paths: vec![PathBuf::from("/tmp/sandbox/secret")],
        allow_network: false,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "test cmd", "_test_SBX", &[]);
    assert!(profile.contains("(allow file-write* (subpath \"/tmp/sandbox\"))"));
    // No network policy appended
    assert!(!profile.contains("com.apple.trustd.agent"));
}

#[test]
fn test_generate_seatbelt_profile_weaker_network_isolation() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        allow_network: false,
        weaker_network_isolation: true,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "test cmd", "_test_SBX", &[]);
    // Should include trustd.agent for Go TLS cert verification
    assert!(profile.contains("com.apple.trustd.agent"));
}

#[test]
fn test_generate_seatbelt_profile_no_trustd_by_default() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        allow_network: false,
        weaker_network_isolation: false,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "test cmd", "_test_SBX", &[]);
    // Should NOT include trustd.agent by default
    assert!(!profile.contains("com.apple.trustd.agent"));
}

#[test]
fn test_generate_seatbelt_profile_escapes_writable_root_paths() {
    let root = WritableRoot::new(r#"/tmp/project "with quotes""#);
    let config = SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        writable_roots: vec![root],
        denied_paths: vec![],
        allow_network: false,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "test cmd", "_test_SBX", &[]);
    // Quotes should be escaped
    assert!(profile.contains(r#"\"with quotes\""#));
    // Should not contain unescaped quotes that break SBPL
    assert!(!profile.contains("\"with quotes\"\""));
}

// ==========================================================================
// Process hardening
// ==========================================================================

#[test]
fn test_wrap_command_removes_dyld_env_vars() {
    let sandbox = MacOsSandbox;
    let config = SandboxConfig {
        enforcement: EnforcementLevel::ReadOnly,
        writable_roots: vec![],
        denied_paths: vec![],
        allow_network: false,
        ..Default::default()
    };
    let mut cmd = tokio::process::Command::new("/bin/echo");
    cmd.arg("test");

    let result = sandbox.wrap_command(&config, "test command", "_test_SBX", &[], &mut cmd);
    assert!(result.is_ok());

    let inner = cmd.as_std();
    let envs: std::collections::HashMap<_, _> = inner
        .get_envs()
        .map(|(k, v)| (k.to_os_string(), v.map(std::ffi::OsStr::to_os_string)))
        .collect();

    // env_remove sets the key with a None value in the env map
    let dyld_insert = std::ffi::OsString::from("DYLD_INSERT_LIBRARIES");
    let dyld_lib = std::ffi::OsString::from("DYLD_LIBRARY_PATH");
    let dyld_fw = std::ffi::OsString::from("DYLD_FRAMEWORK_PATH");

    assert!(
        envs.get(&dyld_insert).is_some_and(Option::is_none),
        "DYLD_INSERT_LIBRARIES should be removed"
    );
    assert!(
        envs.get(&dyld_lib).is_some_and(Option::is_none),
        "DYLD_LIBRARY_PATH should be removed"
    );
    assert!(
        envs.get(&dyld_fw).is_some_and(Option::is_none),
        "DYLD_FRAMEWORK_PATH should be removed"
    );
}

#[test]
fn test_wrap_command_disabled_does_not_remove_env_vars() {
    let sandbox = MacOsSandbox;
    let config = SandboxConfig::default(); // Disabled
    let mut cmd = tokio::process::Command::new("/bin/echo");
    cmd.arg("test");

    let result = sandbox.wrap_command(&config, "test command", "_test_SBX", &[], &mut cmd);
    assert!(result.is_ok());

    let inner = cmd.as_std();
    // When disabled, no env manipulation should occur
    assert!(inner.get_envs().next().is_none());
}

// ==========================================================================
// Command wrapping
// ==========================================================================

#[test]
fn test_wrap_command_disabled_is_noop() {
    let sandbox = MacOsSandbox;
    let config = SandboxConfig::default();
    let mut cmd = tokio::process::Command::new("echo");
    cmd.arg("hello");

    let result = sandbox.wrap_command(&config, "test command", "_test_SBX", &[], &mut cmd);
    assert!(result.is_ok());
    let inner = cmd.as_std();
    assert_eq!(inner.get_program(), "echo");
}

#[test]
fn test_wrap_command_rewrites_to_sandbox_exec() {
    let sandbox = MacOsSandbox;
    let config = SandboxConfig {
        enforcement: EnforcementLevel::ReadOnly,
        writable_roots: vec![],
        denied_paths: vec![],
        allow_network: false,
        ..Default::default()
    };
    let mut cmd = tokio::process::Command::new("/bin/bash");
    cmd.arg("-c").arg("echo hello");

    let result = sandbox.wrap_command(&config, "test command", "_test_SBX", &[], &mut cmd);
    assert!(result.is_ok());

    let inner = cmd.as_std();
    assert_eq!(inner.get_program(), SANDBOX_EXEC_PATH);
    let args: Vec<_> = inner.get_args().collect();
    // Should be: -p <profile> /bin/bash -c "echo hello"
    assert_eq!(args[0], "-p");
    // args[1] is the profile string
    assert_eq!(args[2], "/bin/bash");
    assert_eq!(args[3], "-c");
    assert_eq!(args[4], "echo hello");
}

// ==========================================================================
// PTY disabled
// ==========================================================================

#[test]
fn test_generate_seatbelt_profile_pty_disabled() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::ReadOnly,
        allow_pty: false,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "echo hello", "_test_SBX", &[]);
    assert!(
        profile.contains("(deny pseudo-tty)"),
        "PTY should be denied when allow_pty=false"
    );
    assert!(
        !profile.contains("(allow pseudo-tty)"),
        "PTY allow should not be present when disabled"
    );
    assert!(
        !profile.contains("/dev/ptmx"),
        "ptmx rules should not be present when PTY disabled"
    );
}

// ==========================================================================
// Special write paths
// ==========================================================================

#[test]
fn test_generate_seatbelt_profile_includes_special_write_paths() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::ReadOnly,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "test cmd", "_test_SBX", &[]);
    // Device file writes (from Claude Code kx6)
    assert!(profile.contains("(allow file-write* (literal \"/dev/stdout\"))"));
    assert!(profile.contains("(allow file-write* (literal \"/dev/stderr\"))"));
    // Branded temp dirs
    assert!(profile.contains("(allow file-write* (subpath \"/tmp/coco\"))"));
    assert!(profile.contains("(allow file-write* (subpath \"/private/tmp/coco\"))"));
}

// ==========================================================================
// Command tag truncation
// ==========================================================================

#[test]
fn test_generate_seatbelt_profile_long_command_tag_truncated() {
    let long_command = "x".repeat(500);
    let config = SandboxConfig {
        enforcement: EnforcementLevel::ReadOnly,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, &long_command, "_test_SBX", &[]);
    // Tag should be present but truncated (100 chars → ~136 base64 chars)
    assert!(profile.contains("CMD64_"));
    // 500 chars of 'x' base64-encoded would be ~668 chars; 100 chars → ~136 chars
    // Verify the tag is not excessively long
    let tag_start = profile.find("CMD64_").unwrap();
    let tag_end = profile[tag_start..].find('"').unwrap();
    let tag = &profile[tag_start..tag_start + tag_end];
    // Base64 of 100 bytes = ceil(100/3)*4 = 136 chars + prefix/suffix ≈ 160 chars
    assert!(
        tag.len() < 200,
        "Tag should be truncated, got length {}",
        tag.len()
    );
}

#[test]
fn test_generate_seatbelt_profile_denied_read_paths() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        writable_roots: vec![],
        denied_paths: vec![],
        denied_read_paths: vec![
            std::path::PathBuf::from("/etc/shadow"),
            std::path::PathBuf::from("/private/var/secrets"),
        ],
        allow_network: true,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "ls", "_test_SBX", &[]);
    assert!(
        profile.contains("(deny file-read* (subpath \"/etc/shadow\"))"),
        "Profile should deny reading /etc/shadow"
    );
    assert!(
        profile.contains("(deny file-read* (subpath \"/private/var/secrets\"))"),
        "Profile should deny reading /private/var/secrets"
    );
}

#[test]
fn test_generate_seatbelt_profile_no_denied_read_when_empty() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::ReadOnly,
        writable_roots: vec![],
        denied_paths: vec![],
        denied_read_paths: vec![],
        allow_network: true,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "ls", "_test_SBX", &[]);
    assert!(
        !profile.contains("Explicitly denied read paths"),
        "Profile should NOT have denied read section when no paths configured"
    );
}

#[test]
fn test_generate_seatbelt_profile_denied_paths_also_deny_read() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        writable_roots: vec![],
        denied_paths: vec![std::path::PathBuf::from("/sensitive/data")],
        denied_read_paths: vec![],
        allow_network: true,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "ls", "_test_SBX", &[]);
    assert!(
        profile.contains("(deny file-read* (subpath \"/sensitive/data\"))"),
        "denied_paths should also generate file-read* deny rules"
    );
}

#[test]
fn test_generate_seatbelt_profile_allow_read_emits_after_deny() {
    // TS parity: `allow_read` carve-outs override matching `deny_read` rules.
    // Seatbelt is bottom-to-top last-match-wins, so the allow rule has to
    // appear AFTER the deny rule in the profile text.
    let config = SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        writable_roots: vec![],
        denied_paths: vec![],
        denied_read_paths: vec![std::path::PathBuf::from("/etc/shadow")],
        allowed_read_paths: vec![std::path::PathBuf::from("/etc/shadow/public")],
        allow_network: true,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "ls", "_test_SBX", &[]);
    let deny_idx = profile
        .find("(deny file-read* (subpath \"/etc/shadow\"))")
        .expect("deny rule should be present");
    let allow_idx = profile
        .find("(allow file-read* (subpath \"/etc/shadow/public\"))")
        .expect("allow_read carve-out should be present");
    assert!(
        allow_idx > deny_idx,
        "allow_read rule must appear AFTER deny_read so Seatbelt's \
         last-match-wins resolution lets the carve-out win for matching paths"
    );
    assert!(
        profile.contains("allow_read carve-outs"),
        "Profile should include the carve-out section header"
    );
}

#[test]
fn test_generate_seatbelt_profile_no_allow_read_section_when_empty() {
    let config = SandboxConfig {
        enforcement: EnforcementLevel::ReadOnly,
        writable_roots: vec![],
        denied_paths: vec![],
        denied_read_paths: vec![],
        allowed_read_paths: vec![],
        allow_network: true,
        ..Default::default()
    };

    let profile = generate_seatbelt_profile(&config, "ls", "_test_SBX", &[]);
    assert!(
        !profile.contains("allow_read carve-outs"),
        "Profile should NOT have allow_read section when no carve-outs configured"
    );
}
