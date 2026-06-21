use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use coco_config::SystemReminderConfig;
use coco_context::Phase4Variant;
use coco_context::PlanWorkflow;
use pretty_assertions::assert_eq;
use std::path::PathBuf;

fn cfg() -> SystemReminderConfig {
    SystemReminderConfig::default()
}

// ── PlanModeEnterGenerator ──

#[tokio::test]
async fn enter_returns_none_when_not_in_plan_mode() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c).is_plan_mode(false).build();
    let g = PlanModeEnterGenerator;
    assert!(g.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn enter_emits_full_when_full_flag_set() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .plan_exists(false)
        .explore_plan_agents_available(true)
        .plan_mode_attachments_since_exit(0)
        .build();
    let g = PlanModeEnterGenerator;
    let r = g.generate(&ctx).await.unwrap().expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::PlanMode);
    let text = r.content().expect("text");
    // Full content contains the phase labels (5-phase workflow by default).
    assert!(text.contains("Phase 1"), "full = 5-phase w/ labels: {text}");
}

#[tokio::test]
async fn enter_emits_sparse_when_full_flag_unset() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .plan_exists(false)
        .plan_mode_attachments_since_exit(1)
        .build();
    let g = PlanModeEnterGenerator;
    let r = g.generate(&ctx).await.unwrap().expect("emits");
    let text = r.content().expect("text");
    // Sparse content does NOT include "Phase 1" / "Phase 2" / etc. labels.
    assert!(
        !text.contains("### Phase 1"),
        "sparse must not include phase headers: {text}"
    );
}

#[tokio::test]
async fn enter_attachment_type_is_plan_mode() {
    let g = PlanModeEnterGenerator;
    assert_eq!(g.attachment_type(), AttachmentType::PlanMode);
    assert_eq!(g.name(), "PlanModeEnterGenerator");
}

#[tokio::test]
async fn enter_cadence_is_history_derived() {
    let c = cfg();
    let g = PlanModeEnterGenerator;

    // First plan-mode turn (no prior attachment) always emits.
    let first = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .plan_mode_turns_since_attachment(None)
        .build();
    assert!(g.generate(&first).await.unwrap().is_some());

    // Within the 5-turn window → throttled.
    let within = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .plan_mode_turns_since_attachment(Some(3))
        .build();
    assert!(g.generate(&within).await.unwrap().is_none());

    // At/after the window → emits again.
    let after = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .plan_mode_turns_since_attachment(Some(5))
        .build();
    assert!(g.generate(&after).await.unwrap().is_some());
}

#[tokio::test]
async fn enter_is_disabled_when_config_flag_off() {
    let mut c = cfg();
    c.attachments.plan_mode = false;
    let g = PlanModeEnterGenerator;
    assert!(!g.is_enabled(&c));
}

#[tokio::test]
async fn enter_full_sub_agent_path_is_independent_of_workflow() {
    let c = cfg();
    // Sub-agent flag overrides normal 5-phase rendering.
    let ctx = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .is_sub_agent(true)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .plan_exists(false)
        .plan_mode_attachments_since_exit(0)
        .build();
    let g = PlanModeEnterGenerator;
    let r = g.generate(&ctx).await.unwrap().expect("emits");
    let text = r.content().expect("text");
    // Sub-agent rendering does not reference parallel Explore agent count.
    assert!(
        !text.contains("Explore agents in parallel"),
        "sub-agent prompt should not mention parallel Explore: {text}"
    );
}

#[tokio::test]
async fn enter_interview_workflow_full_content_differs_from_five_phase() {
    let c = cfg();
    let five_phase = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .plan_workflow(PlanWorkflow::FivePhase)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .plan_exists(false)
        .explore_plan_agents_available(true)
        .plan_mode_attachments_since_exit(0)
        .build();
    let interview = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .plan_workflow(PlanWorkflow::Interview)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .plan_exists(false)
        .explore_plan_agents_available(true)
        .plan_mode_attachments_since_exit(0)
        .build();

    let g = PlanModeEnterGenerator;
    let five_text = g
        .generate(&five_phase)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    let interview_text = g
        .generate(&interview)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert_ne!(five_text, interview_text, "workflows must differ");
}

#[tokio::test]
async fn enter_phase4_variant_affects_full_five_phase_only() {
    let c = cfg();
    let standard = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .phase4_variant(Phase4Variant::Standard)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .plan_exists(false)
        .explore_plan_agents_available(true)
        .plan_mode_attachments_since_exit(0)
        .build();
    let cut = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .phase4_variant(Phase4Variant::Cut)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .plan_exists(false)
        .explore_plan_agents_available(true)
        .plan_mode_attachments_since_exit(0)
        .build();

    let g = PlanModeEnterGenerator;
    let s_text = g
        .generate(&standard)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    let c_text = g
        .generate(&cut)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert_ne!(
        s_text, c_text,
        "Phase-4 variants must render different text"
    );
}

#[tokio::test]
async fn all_plan_generators_suppressed_when_feature_off() {
    // `features.plan_mode = false` (→ plan_mode_feature_enabled=false) wins
    // over every other firing condition for all three generators.
    let c = cfg();

    let enter = GeneratorContext::builder(&c)
        .plan_mode_feature_enabled(false)
        .is_plan_mode(true)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .plan_mode_attachments_since_exit(0)
        .build();
    assert!(
        PlanModeEnterGenerator
            .generate(&enter)
            .await
            .unwrap()
            .is_none(),
        "enter must be suppressed when plan_mode feature is off"
    );

    let exit = GeneratorContext::builder(&c)
        .plan_mode_feature_enabled(false)
        .needs_plan_mode_exit_attachment(true)
        .plan_exists(true)
        .build();
    assert!(
        PlanModeExitGenerator
            .generate(&exit)
            .await
            .unwrap()
            .is_none(),
        "exit must be suppressed when plan_mode feature is off"
    );

    let reentry = GeneratorContext::builder(&c)
        .plan_mode_feature_enabled(false)
        .is_plan_mode(true)
        .is_plan_reentry(true)
        .plan_exists(true)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .build();
    assert!(
        PlanModeReentryGenerator
            .generate(&reentry)
            .await
            .unwrap()
            .is_none(),
        "reentry must be suppressed when plan_mode feature is off"
    );
}

// ── PlanModeExitGenerator ──

#[tokio::test]
async fn exit_none_without_flag() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .needs_plan_mode_exit_attachment(false)
        .build();
    assert!(
        PlanModeExitGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn exit_emits_when_flag_set() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .needs_plan_mode_exit_attachment(true)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .plan_exists(true)
        .build();
    let r = PlanModeExitGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::PlanModeExit);
    let text = r.content().expect("text");
    assert!(text.contains("Exited Plan Mode"), "banner text: {text}");
    assert!(
        text.contains("/tmp/plan.md"),
        "references plan file: {text}"
    );
}

#[tokio::test]
async fn exit_suppressed_when_still_in_plan_mode() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .needs_plan_mode_exit_attachment(true)
        .build();
    assert!(
        PlanModeExitGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn exit_omits_plan_reference_when_no_file() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .needs_plan_mode_exit_attachment(true)
        .plan_exists(false)
        .build();
    let r = PlanModeExitGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    let text = r.content().expect("text");
    assert!(
        !text.contains("plan file is located"),
        "no plan → no 'plan file is located' fragment: {text}"
    );
}

#[tokio::test]
async fn exit_no_implementation_plan_omits_stale_plan_reference() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .needs_plan_mode_exit_attachment(true)
        .pending_plan_mode_exit_outcome(Some(coco_types::ExitPlanModeOutcome::NoImplementationPlan))
        .plan_file_path(Some(PathBuf::from("/tmp/old-plan.md")))
        .plan_exists(true)
        .build();
    let r = PlanModeExitGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    let text = r.content().expect("text");
    assert!(
        text.contains("without an implementation plan"),
        "no-plan exit should have typed copy: {text}"
    );
    assert!(
        !text.contains("/tmp/old-plan.md"),
        "no-plan exit must not reference stale plan file: {text}"
    );
}

// ── PlanModeReentryGenerator ──

#[tokio::test]
async fn reentry_requires_plan_mode() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .is_plan_mode(false)
        .is_plan_reentry(true)
        .plan_exists(true)
        .build();
    assert!(
        PlanModeReentryGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn reentry_requires_reentry_flag() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .is_plan_reentry(false)
        .plan_exists(true)
        .build();
    assert!(
        PlanModeReentryGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn reentry_requires_existing_plan() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .is_plan_reentry(true)
        .plan_exists(false)
        .build();
    assert!(
        PlanModeReentryGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn reentry_skipped_for_sub_agent() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .is_plan_reentry(true)
        .plan_exists(true)
        .is_sub_agent(true)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .build();
    assert!(
        PlanModeReentryGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none(),
        "sub-agents must be gated out of Reentry"
    );
}

#[tokio::test]
async fn reentry_emits_when_all_conditions_hold() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .is_plan_mode(true)
        .is_plan_reentry(true)
        .plan_exists(true)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .build();
    let r = PlanModeReentryGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::PlanModeReentry);
    let text = r.content().expect("text");
    assert!(
        text.contains("Re-entering Plan Mode"),
        "banner text: {text}"
    );
    assert!(
        text.contains("/tmp/plan.md"),
        "references plan file: {text}"
    );
}
