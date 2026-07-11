use super::*;

fn rewriter_with(config: RtkConfig) -> RtkRewriter {
    RtkRewriter::new(config)
}

const NO_SKIP: RewriteSite = RewriteSite {
    background: false,
    sandboxed: false,
};

#[test]
fn tier_as_str() {
    assert_eq!(RtkTier::Builtin.as_str(), "builtin");
    assert_eq!(RtkTier::External.as_str(), "external");
}

#[test]
fn mode_projects_to_tier_capabilities() {
    // (mode, does_pre_spawn_rewrite, does_post_exec_filter) — the §3.5 policy
    // that `BashTool` consumes without ever seeing `RtkMode`.
    let cases = [
        (RtkMode::BuiltinFirst, false, true),
        (RtkMode::BuiltinOnly, false, true),
        (RtkMode::ExternalFirst, true, true),
        (RtkMode::ExternalOnly, true, false),
    ];
    for (mode, pre, post) in cases {
        let rewriter = rewriter_with(RtkConfig {
            mode,
            ..Default::default()
        });
        assert_eq!(
            rewriter.does_pre_spawn_rewrite(),
            pre,
            "pre_spawn for {mode:?}"
        );
        assert_eq!(
            rewriter.does_post_exec_filter(),
            post,
            "post_exec for {mode:?}"
        );
    }
}

#[tokio::test]
async fn filter_output_delegates_to_embedded_core() {
    // The `BashOutputRewriter::filter_output` impl routes through the post-exec
    // filter core: a single covered command compresses, a compound one stays raw.
    let rewriter = rewriter_with(RtkConfig::default());
    let mut input = String::from("Filesystem     1K-blocks   Used Available Use% Mounted on\n");
    for i in 0..40 {
        input.push_str(&format!(
            "/dev/sda{i}        4096000 123456   3972544   4% /mnt/{i}\n"
        ));
    }
    assert!(rewriter.filter_output("df -h", 0, &input).await.is_some());
    assert_eq!(
        rewriter.filter_output("df -h && echo x", 0, &input).await,
        None
    );
}

#[tokio::test]
async fn background_skips_before_probe() {
    // A bogus binary_path would fail if probed — proving background short-
    // circuits before any subprocess.
    let rewriter = rewriter_with(RtkConfig {
        binary_path: Some("/nonexistent/rtk".to_string()),
        ..Default::default()
    });
    let site = RewriteSite {
        background: true,
        sandboxed: false,
    };
    assert_eq!(
        rewriter.rewrite("git status", site).await,
        RewriteOutcome::Passthrough(PassthroughReason::Background)
    );
}

#[tokio::test]
async fn sandboxed_skips_before_probe() {
    let rewriter = rewriter_with(RtkConfig {
        binary_path: Some("/nonexistent/rtk".to_string()),
        ..Default::default()
    });
    let site = RewriteSite {
        background: false,
        sandboxed: true,
    };
    assert_eq!(
        rewriter.rewrite("git status", site).await,
        RewriteOutcome::Passthrough(PassthroughReason::Sandboxed)
    );
}

#[tokio::test]
async fn excluded_first_word_skips_before_probe() {
    let rewriter = rewriter_with(RtkConfig {
        binary_path: Some("/nonexistent/rtk".to_string()),
        exclude_commands: vec!["docker".to_string()],
        ..Default::default()
    });
    assert_eq!(
        rewriter.rewrite("docker ps", NO_SKIP).await,
        RewriteOutcome::Passthrough(PassthroughReason::Excluded)
    );
}

#[tokio::test]
async fn excluded_matches_after_safe_env_prefix() {
    let rewriter = rewriter_with(RtkConfig {
        binary_path: Some("/nonexistent/rtk".to_string()),
        exclude_commands: vec!["cargo".to_string()],
        ..Default::default()
    });
    // `get_first_word_prefix` strips the safe `RUST_LOG` env assignment.
    assert_eq!(
        rewriter.rewrite("RUST_LOG=debug cargo test", NO_SKIP).await,
        RewriteOutcome::Passthrough(PassthroughReason::Excluded)
    );
}

#[tokio::test]
async fn non_excluded_command_proceeds_to_probe() {
    // A command not on the exclude list falls through to the probe; the bogus
    // path fails detection → BinaryMissing (not Excluded).
    let rewriter = rewriter_with(RtkConfig {
        binary_path: Some("/nonexistent/rtk".to_string()),
        exclude_commands: vec!["docker".to_string()],
        ..Default::default()
    });
    assert_eq!(
        rewriter.rewrite("git status", NO_SKIP).await,
        RewriteOutcome::Passthrough(PassthroughReason::BinaryMissing)
    );
}

#[tokio::test]
async fn missing_binary_passes_through() {
    let rewriter = rewriter_with(RtkConfig {
        binary_path: Some("/nonexistent/definitely/not/rtk".to_string()),
        ..Default::default()
    });
    assert_eq!(
        rewriter.rewrite("git status", NO_SKIP).await,
        RewriteOutcome::Passthrough(PassthroughReason::BinaryMissing)
    );
}

/// End-to-end against a real `rtk` on `$PATH`. Ignored by default — run with
/// `cargo test -p coco-shell -- --ignored rtk` on a machine with rtk >= 0.23.
#[tokio::test]
#[ignore = "requires a real rtk binary on PATH"]
async fn real_rtk_rewrites_git_status() {
    if which::which("rtk").is_err() {
        eprintln!("skipping: no rtk on PATH");
        return;
    }
    let rewriter = rewriter_with(RtkConfig::default());
    match rewriter.rewrite("git status", NO_SKIP).await {
        RewriteOutcome::Rewritten(cmd) => {
            assert!(cmd.starts_with("rtk "), "unexpected rewrite: {cmd}");
        }
        RewriteOutcome::Passthrough(reason) => {
            panic!("expected a rewrite from real rtk, got passthrough: {reason:?}");
        }
    }
}
