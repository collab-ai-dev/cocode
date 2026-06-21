use pretty_assertions::assert_eq;

use super::ForkLabel;

#[test]
fn test_as_str_round_trip_with_serde() {
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
    for (variant, wire) in cases {
        assert_eq!(variant.as_str(), wire, "as_str for {variant:?}");
        assert_eq!(variant.to_string(), wire, "Display for {variant:?}");
        let json = serde_json::to_string(&variant).unwrap();
        assert_eq!(json, format!("\"{wire}\""), "serialize for {variant:?}");
        let round: ForkLabel = serde_json::from_str(&json).unwrap();
        assert_eq!(round, variant, "round-trip for {variant:?}");
    }
}

#[test]
fn test_wire_strings_are_unique() {
    let all = [
        ForkLabel::PromptSuggestion,
        ForkLabel::SideQuestion,
        ForkLabel::Compact,
        ForkLabel::ExtractMemories,
        ForkLabel::SessionMemoryAuto,
        ForkLabel::SessionMemoryManual,
        ForkLabel::AgentSummary,
        ForkLabel::AutoDream,
        ForkLabel::Speculation,
        ForkLabel::HookAgent,
    ];
    let mut wires: Vec<&'static str> = all.iter().map(|v| v.as_str()).collect();
    wires.sort_unstable();
    let len_before = wires.len();
    wires.dedup();
    assert_eq!(wires.len(), len_before, "wire strings must be unique");
}
