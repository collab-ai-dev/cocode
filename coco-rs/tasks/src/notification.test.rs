use super::*;

#[test]
fn shell_terminal_completed_with_exit_code() {
    let n = TaskNotification {
        task_id: "tb01".into(),
        tool_use_id: Some("toolu_a".into()),
        agent_id: None,
        output_file: "/tmp/tb01.output".into(),
        description: "ls -la".into(),
        kind: NotificationKind::ShellTerminal {
            status: TerminalStatus::Completed,
            exit_code: Some(0),
        },
    };
    let xml = render(&n);
    assert!(xml.contains("<task-id>tb01</task-id>"));
    assert!(xml.contains("<tool-use-id>toolu_a</tool-use-id>"));
    assert!(xml.contains("<output-file>/tmp/tb01.output</output-file>"));
    assert!(xml.contains("<status>completed</status>"));
    assert!(xml.contains(
        "<summary>Background command &quot;ls -la&quot; completed (exit code 0)</summary>"
    ));
    assert!(!xml.contains("<result>"));
    assert!(!xml.contains("<usage>"));
    assert!(!xml.contains("<worktree>"));
}

#[test]
fn shell_terminal_failed_with_exit_code() {
    let n = TaskNotification {
        task_id: "tb02".into(),
        tool_use_id: None,
        agent_id: None,
        output_file: "/tmp/tb02.output".into(),
        description: "make".into(),
        kind: NotificationKind::ShellTerminal {
            status: TerminalStatus::Failed,
            exit_code: Some(2),
        },
    };
    let xml = render(&n);
    assert!(xml.contains("<status>failed</status>"));
    assert!(xml.contains(
        "<summary>Background command &quot;make&quot; failed with exit code 2</summary>"
    ));
}

#[test]
fn shell_terminal_killed_omits_exit_code() {
    let n = TaskNotification {
        task_id: "tb03".into(),
        tool_use_id: None,
        agent_id: None,
        output_file: "/tmp/tb03.output".into(),
        description: "sleep 999".into(),
        kind: NotificationKind::ShellTerminal {
            status: TerminalStatus::Killed,
            exit_code: None,
        },
    };
    let xml = render(&n);
    assert!(xml.contains("<status>killed</status>"));
    assert!(
        xml.contains("<summary>Background command &quot;sleep 999&quot; was stopped</summary>")
    );
    assert!(!xml.contains("(exit code"));
}

#[test]
fn agent_terminal_completed_includes_result_usage_worktree() {
    // Full envelope with all three optional sections.
    let n = TaskNotification {
        task_id: "ta01".into(),
        tool_use_id: Some("toolu_a".into()),
        agent_id: None,
        output_file: "/tmp/ta01.output".into(),
        description: "explore repo".into(),
        kind: NotificationKind::AgentTerminal {
            status: TerminalStatus::Completed,
            result: Some("Found 3 callers.".into()),
            usage: Some(TaskUsage {
                total_tokens: 1234,
                tool_uses: 5,
                duration_ms: 7890,
            }),
            worktree: Some(Worktree {
                path: "/tmp/wt/ta01".into(),
                branch: Some("feat/x".into()),
            }),
            error: None,
        },
    };
    let xml = render(&n);
    assert!(xml.contains("<status>completed</status>"));
    assert!(xml.contains("<summary>Agent &quot;explore repo&quot; completed</summary>"));
    assert!(xml.contains("<result>Found 3 callers.</result>"));
    assert!(xml.contains(
        "<usage><total_tokens>1234</total_tokens><tool_uses>5</tool_uses><duration_ms>7890</duration_ms></usage>"
    ));
    assert!(xml.contains(
        "<worktree><worktreePath>/tmp/wt/ta01</worktreePath><worktreeBranch>feat/x</worktreeBranch></worktree>"
    ));
}

#[test]
fn agent_terminal_optional_sections_omitted_when_none() {
    // Template evaluates to '' when the input is undefined — sections are
    // omitted from the wire, not emitted as empty tags.
    let n = TaskNotification {
        task_id: "ta02".into(),
        tool_use_id: None,
        agent_id: None,
        output_file: "/tmp/ta02.output".into(),
        description: "thinking".into(),
        kind: NotificationKind::AgentTerminal {
            status: TerminalStatus::Completed,
            result: None,
            usage: None,
            worktree: None,
            error: None,
        },
    };
    let xml = render(&n);
    assert!(!xml.contains("<result>"));
    assert!(!xml.contains("<usage>"));
    assert!(!xml.contains("<worktree>"));
}

#[test]
fn agent_terminal_failed_uses_error_in_summary() {
    let n = TaskNotification {
        task_id: "ta03".into(),
        tool_use_id: None,
        agent_id: None,
        output_file: "/tmp/ta03.output".into(),
        description: "build".into(),
        kind: NotificationKind::AgentTerminal {
            status: TerminalStatus::Failed,
            result: None,
            usage: None,
            worktree: None,
            error: Some("compiler crash".into()),
        },
    };
    let xml = render(&n);
    assert!(xml.contains("<status>failed</status>"));
    assert!(xml.contains("<summary>Agent &quot;build&quot; failed: compiler crash</summary>"));
}

#[test]
fn agent_worktree_branch_optional() {
    let n = TaskNotification {
        task_id: "ta04".into(),
        tool_use_id: None,
        agent_id: None,
        output_file: "/tmp/ta04.output".into(),
        description: "x".into(),
        kind: NotificationKind::AgentTerminal {
            status: TerminalStatus::Completed,
            result: None,
            usage: None,
            worktree: Some(Worktree {
                path: "/wt".into(),
                branch: None,
            }),
            error: None,
        },
    };
    let xml = render(&n);
    assert!(xml.contains("<worktreePath>/wt</worktreePath>"));
    assert!(!xml.contains("<worktreeBranch>"));
    assert!(xml.contains("</worktree>"));
}

#[test]
fn stall_omits_status_tag() {
    // Stall must NOT carry <status>.
    let n = TaskNotification {
        task_id: "tb04".into(),
        tool_use_id: None,
        agent_id: None,
        output_file: "/tmp/tb04.output".into(),
        description: "sleep".into(),
        kind: NotificationKind::Stall {
            output_tail: "Continue? [y/N]".into(),
        },
    };
    let xml = render(&n);
    assert!(!xml.contains("<status>"));
    assert!(xml.contains("<summary>"));
    assert!(xml.contains("waiting for interactive input"));
    assert!(xml.contains("Last output:\nContinue? [y/N]"));
    assert!(xml.contains("Kill this task"));
}

#[test]
fn escape_xml_handles_5_chars() {
    let n = TaskNotification {
        task_id: "tb05".into(),
        tool_use_id: None,
        agent_id: None,
        output_file: "/tmp/x.out".into(),
        description: "<x>&\"'".into(),
        kind: NotificationKind::ShellTerminal {
            status: TerminalStatus::Completed,
            exit_code: None,
        },
    };
    let xml = render(&n);
    assert!(xml.contains("&lt;x&gt;&amp;&quot;&apos;"));
}

#[tokio::test]
async fn noop_sink_swallows() {
    let n = TaskNotification {
        task_id: "x".into(),
        tool_use_id: None,
        agent_id: None,
        output_file: String::new(),
        description: String::new(),
        kind: NotificationKind::ShellTerminal {
            status: TerminalStatus::Completed,
            exit_code: None,
        },
    };
    NoOpNotificationSink.push(n).await;
}
