use coco_types::ForkLabel;
use pretty_assertions::assert_eq;

use super::ForkContextOverrides;
use super::auto_agent_id;

#[test]
fn test_for_label_conservative_defaults() {
    let o = ForkContextOverrides::for_label(ForkLabel::PromptSuggestion);
    assert_eq!(o.fork_label, ForkLabel::PromptSuggestion);
    assert_eq!(o.query_source, "prompt_suggestion");
    assert!(o.agent_id.is_none());
    assert!(
        o.clone_file_read_state,
        "default deep-clone of FileReadState for per-fork dedup-cache isolation"
    );
    assert!(o.can_use_tool.is_none());
    assert!(!o.require_can_use_tool);
    assert!(o.allowed_write_roots.is_empty());
    assert_eq!(o.parent_query_depth, 0);
}

#[test]
fn test_child_query_depth_increments() {
    let mut o = ForkContextOverrides::for_label(ForkLabel::ExtractMemories);
    o.parent_query_depth = 0;
    assert_eq!(o.child_query_depth(), 1);
    o.parent_query_depth = 5;
    assert_eq!(o.child_query_depth(), 6);
}

#[test]
fn test_child_query_depth_caps_at_16() {
    let mut o = ForkContextOverrides::for_label(ForkLabel::AutoDream);
    o.parent_query_depth = 100;
    assert_eq!(
        o.child_query_depth(),
        16,
        "depth must cap to prevent runaway recursion"
    );
}

#[test]
fn test_auto_agent_id_format_and_uniqueness() {
    let a = auto_agent_id(ForkLabel::SessionMemoryAuto);
    let b = auto_agent_id(ForkLabel::SessionMemoryAuto);
    assert!(a.starts_with("fork-session_memory_auto-"));
    assert!(b.starts_with("fork-session_memory_auto-"));
    assert_ne!(a, b, "two simultaneous forks must get distinct ids");
}

#[test]
fn test_for_label_query_source_matches_label() {
    let cases = [
        (ForkLabel::PromptSuggestion, "prompt_suggestion"),
        (ForkLabel::SideQuestion, "side_question"),
        (ForkLabel::Compact, "compact"),
        (ForkLabel::ExtractMemories, "extract_memories"),
        (ForkLabel::SessionMemoryAuto, "session_memory_auto"),
        (ForkLabel::SessionMemoryManual, "session_memory_manual"),
        (ForkLabel::AgentSummary, "agent_summary"),
        (ForkLabel::AutoDream, "auto_dream"),
        (ForkLabel::Speculation, "speculation"),
        (ForkLabel::HookAgent, "hook_agent"),
    ];
    for (label, wire) in cases {
        let o = ForkContextOverrides::for_label(label);
        assert_eq!(o.query_source, wire);
    }
}
