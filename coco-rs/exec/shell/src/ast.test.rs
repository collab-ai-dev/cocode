use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_parse_simple_command() {
    let node = parse_command("ls -la /tmp");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.command, "ls");
        assert_eq!(cmd.args, vec!["-la", "/tmp"]);
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_pipeline() {
    let node = parse_command("cat file | grep pattern | wc -l");
    if let BashNode::Pipeline(nodes) = node {
        assert_eq!(nodes.len(), 3);
    } else {
        panic!("expected Pipeline");
    }
}

#[test]
fn test_parse_list_and() {
    let node = parse_command("cd /tmp && ls");
    if let BashNode::List { operator, .. } = node {
        assert_eq!(operator, ListOperator::And);
    } else {
        panic!("expected List");
    }
}

#[test]
fn test_parse_list_or() {
    let node = parse_command("cmd1 || cmd2");
    if let BashNode::List { operator, .. } = node {
        assert_eq!(operator, ListOperator::Or);
    } else {
        panic!("expected List");
    }
}

#[test]
fn test_parse_list_semicolon() {
    let node = parse_command("echo a ; echo b");
    if let BashNode::List { operator, .. } = node {
        assert_eq!(operator, ListOperator::Semicolon);
    } else {
        panic!("expected List");
    }
}

#[test]
fn test_parse_with_assignment() {
    let node = parse_command("FOO=bar cargo test");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.assignments.len(), 1);
        assert_eq!(cmd.assignments[0].name, "FOO");
        assert_eq!(cmd.assignments[0].value, "bar");
        assert_eq!(cmd.command, "cargo");
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_multiple_assignments() {
    let node = parse_command("A=1 B=2 cmd");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.assignments.len(), 2);
        assert_eq!(cmd.assignments[0].name, "A");
        assert_eq!(cmd.assignments[1].name, "B");
        assert_eq!(cmd.command, "cmd");
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_assignment_only() {
    let node = parse_command("FOO=bar");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.assignments.len(), 1);
        assert!(cmd.command.is_empty());
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_with_redirect_write() {
    let node = parse_command("echo hello > output.txt");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.command, "echo");
        assert_eq!(cmd.redirects.len(), 1);
        assert_eq!(cmd.redirects[0].operator, RedirectOperator::Write);
        assert_eq!(cmd.redirects[0].target, "output.txt");
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_with_redirect_append() {
    let node = parse_command("echo hello >> output.txt");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.redirects.len(), 1);
        assert_eq!(cmd.redirects[0].operator, RedirectOperator::Append);
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_with_redirect_input() {
    let node = parse_command("cat < input.txt");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.redirects.len(), 1);
        assert_eq!(cmd.redirects[0].operator, RedirectOperator::Input);
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_with_redirect_both() {
    let node = parse_command("cmd &> /dev/null");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.redirects.len(), 1);
        assert_eq!(cmd.redirects[0].operator, RedirectOperator::Both);
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_with_herestring() {
    let node = parse_command("cat <<< 'hello'");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.command, "cat");
        assert_eq!(cmd.redirects.len(), 1);
        assert_eq!(cmd.redirects[0].operator, RedirectOperator::HereString);
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_with_heredoc() {
    let node = parse_command("cat <<EOF");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.command, "cat");
        assert_eq!(cmd.redirects.len(), 1);
        assert_eq!(cmd.redirects[0].operator, RedirectOperator::Heredoc);
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_single_quoted_arg() {
    let node = parse_command("echo 'hello world'");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.command, "echo");
        assert_eq!(cmd.args, vec!["hello world"]);
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_double_quoted_arg() {
    let node = parse_command(r#"echo "hello world""#);
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.command, "echo");
        assert_eq!(cmd.args, vec!["hello world"]);
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_ansi_c_string() {
    let node = parse_command("echo $'hello\\nworld'");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.command, "echo");
        assert_eq!(cmd.args, vec!["hello\nworld"]);
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_extract_simple_commands() {
    let node = parse_command("git add . && git commit -m msg");
    let cmds = extract_simple_commands(&node);
    assert_eq!(cmds.len(), 2);
    assert_eq!(cmds[0].command, "git");
    assert_eq!(cmds[1].command, "git");
}

#[test]
fn test_has_output_redirect() {
    assert!(has_output_redirect(&parse_command("echo hi > file")));
    assert!(!has_output_redirect(&parse_command("echo hi")));
    assert!(!has_output_redirect(&parse_command("cat < input")));
}

#[test]
fn test_parse_subshell() {
    let node = parse_command("(cd /tmp && ls)");
    assert!(matches!(node, BashNode::Subshell(_)));
}

#[test]
fn test_parse_compound_if() {
    let node = parse_command("if true; then echo yes; fi");
    assert!(matches!(node, BashNode::Compound { .. }));
}

#[test]
fn test_parse_compound_for() {
    let node = parse_command("for i in 1 2 3; do echo $i; done");
    assert!(matches!(node, BashNode::Compound { .. }));
}

#[test]
fn test_parse_compound_while() {
    let node = parse_command("while true; do echo loop; done");
    assert!(matches!(node, BashNode::Compound { .. }));
}

#[test]
fn test_extract_argv() {
    let node = parse_command("git status --short");
    let argv = extract_argv(&node);
    assert_eq!(
        argv,
        Some(vec![
            "git".to_string(),
            "status".to_string(),
            "--short".to_string()
        ])
    );
}

#[test]
fn test_extract_argv_from_pipeline() {
    let node = parse_command("cat file | grep pattern");
    // extract_argv only works on SimpleCommand, not Pipeline
    assert_eq!(extract_argv(&node), None);
}

#[test]
fn test_parse_empty() {
    let node = parse_command("");
    assert!(matches!(node, BashNode::Raw(ref s) if s.is_empty()));
}

#[test]
fn test_parse_whitespace_only() {
    let node = parse_command("   ");
    assert!(matches!(node, BashNode::Raw(ref s) if s.is_empty()));
}

#[test]
fn test_parse_escaped_space_in_arg() {
    let node = parse_command("echo hello\\ world");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.command, "echo");
        assert_eq!(cmd.args, vec!["hello world"]);
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_redirect_clobber() {
    let node = parse_command("echo hi >| file");
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.redirects.len(), 1);
        assert_eq!(cmd.redirects[0].operator, RedirectOperator::Clobber);
    } else {
        panic!("expected SimpleCommand");
    }
}

#[test]
fn test_parse_complex_pipeline_with_args() {
    let node = parse_command("find . -name '*.rs' | xargs grep -l pattern");
    if let BashNode::Pipeline(nodes) = node {
        assert_eq!(nodes.len(), 2);
        if let BashNode::SimpleCommand(cmd) = &nodes[0] {
            assert_eq!(cmd.command, "find");
            assert_eq!(cmd.args, vec![".", "-name", "*.rs"]);
        } else {
            panic!("expected SimpleCommand in pipeline");
        }
    } else {
        panic!("expected Pipeline");
    }
}

#[test]
fn test_double_quote_escape_sequences() {
    let node = parse_command(r#"echo "path is \"here\"""#);
    if let BashNode::SimpleCommand(cmd) = node {
        assert_eq!(cmd.args, vec!["path is \"here\""]);
    } else {
        panic!("expected SimpleCommand");
    }
}
