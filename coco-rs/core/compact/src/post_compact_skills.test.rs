use super::*;

fn skill(name: &str, body: &str) -> PostCompactSkill {
    PostCompactSkill {
        name: name.into(),
        path: format!("skills/{name}.md"),
        content: body.into(),
    }
}

#[test]
fn test_no_skills_returns_empty() {
    let out = create_post_compact_skill_attachments(&[]);
    assert!(out.is_empty());
}

#[test]
fn test_single_skill_renders_attachment() {
    let out = create_post_compact_skill_attachments(&[skill("foo", "body content")]);
    assert_eq!(out.len(), 1);
}

#[test]
fn test_total_budget_caps_count() {
    let big_body = "x".repeat(200_000); // ~50K tokens
    let skills: Vec<PostCompactSkill> = (0..10)
        .map(|i| skill(&format!("s{i}"), &big_body))
        .collect();
    let out = create_post_compact_skill_attachments(&skills);
    // Per-skill cap is 5K → total budget 25K → ~5 skills max, never 10.
    assert!(out.len() <= 6, "got {}", out.len());
}

#[test]
fn test_per_skill_truncation_marker_present_for_oversize() {
    let big_body = "y".repeat(80_000); // ~20K tokens (above 5K per-skill cap)
    let out = create_post_compact_skill_attachments(&[skill("big", &big_body)]);
    assert_eq!(out.len(), 1);
    let json = serde_json::to_string(&out[0]).unwrap();
    assert!(json.contains("skill truncated"));
}
