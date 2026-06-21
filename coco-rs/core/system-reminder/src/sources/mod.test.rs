use super::*;
use coco_config::SystemReminderConfig;
use std::time::Duration;

#[tokio::test]
async fn empty_sources_materializes_all_defaults() {
    let sources = ReminderSources::default();
    let cfg = SystemReminderConfig::default();
    let out = sources
        .materialize(MaterializeContext {
            config: &cfg,
            agent_id: None,
            user_input: None,
            mentioned_paths: &[],
            recent_tools: &[],
            just_compacted: false,
            per_source_timeout: Duration::from_millis(1000),
            skill_overrides: &coco_config::SkillOverrideTiers::default(),
            skill_tool_loaded: true,
        })
        .await;
    assert!(out.hook_events.is_empty());
    assert!(out.diagnostics.is_empty());
    assert!(out.task_statuses.is_empty());
    assert!(out.skill_listing.is_none());
    assert!(out.invoked_skills.is_empty());
    assert!(out.mcp_instructions_current.is_empty());
    assert!(out.mcp_resources.is_empty());
    assert!(out.teammate_mailbox.is_none());
    assert!(out.team_context.is_none());
    assert!(out.agent_pending_messages.is_empty());
    assert!(out.ide_selection.is_none());
    assert!(out.ide_opened_file.is_none());
    assert!(out.nested_memories.is_empty());
    assert!(out.relevant_memories.is_empty());
}

#[tokio::test]
async fn noop_bundle_also_materializes_defaults_through_trait_dispatch() {
    let sources = ReminderSources::noop();
    let cfg = SystemReminderConfig::default();
    let out = sources
        .materialize(MaterializeContext {
            config: &cfg,
            agent_id: None,
            user_input: Some("hello"),
            mentioned_paths: &[],
            recent_tools: &[],
            just_compacted: false,
            per_source_timeout: Duration::from_millis(1000),
            skill_overrides: &coco_config::SkillOverrideTiers::default(),
            skill_tool_loaded: true,
        })
        .await;
    // Every field should still be default — NoOps return empty/None.
    assert!(out.hook_events.is_empty());
    assert!(out.diagnostics.is_empty());
    assert!(out.task_statuses.is_empty());
    assert!(out.mcp_resources.is_empty());
    assert!(out.nested_memories.is_empty());
    assert!(out.relevant_memories.is_empty());
}

#[tokio::test]
async fn disabled_config_skips_sources_even_when_present() {
    // Custom source that would return non-empty — if called.
    use crate::generator::DiagnosticFileSummary;
    use async_trait::async_trait;
    use std::sync::Arc;

    #[derive(Debug, Default)]
    struct SpyDiagSource {
        called: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl DiagnosticsSource for SpyDiagSource {
        async fn snapshot(&self, _agent_id: Option<&str>) -> Vec<DiagnosticFileSummary> {
            self.called
                .store(true, std::sync::atomic::Ordering::Relaxed);
            vec![DiagnosticFileSummary {
                path: "x".into(),
                formatted: "x: error".into(),
            }]
        }
    }

    let spy = Arc::new(SpyDiagSource::default());
    let sources = ReminderSources {
        diagnostics: Some(spy.clone()),
        ..Default::default()
    };
    let mut cfg = SystemReminderConfig::default();
    cfg.attachments.diagnostics = false;

    let out = sources
        .materialize(MaterializeContext {
            config: &cfg,
            agent_id: None,
            user_input: None,
            mentioned_paths: &[],
            recent_tools: &[],
            just_compacted: false,
            per_source_timeout: Duration::from_millis(1000),
            skill_overrides: &coco_config::SkillOverrideTiers::default(),
            skill_tool_loaded: true,
        })
        .await;

    assert!(out.diagnostics.is_empty());
    assert!(
        !spy.called.load(std::sync::atomic::Ordering::Relaxed),
        "config-gated source must not be called when disabled"
    );
}

#[tokio::test]
async fn skill_listing_gate_skips_listing_source_when_skill_tool_unavailable() {
    use async_trait::async_trait;
    use std::sync::Arc;

    #[derive(Debug, Default)]
    struct SpySkillsSource {
        listing_calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl SkillsSource for SpySkillsSource {
        async fn listing(
            &self,
            _agent_id: Option<&str>,
            _tiers: &coco_config::SkillOverrideTiers,
        ) -> Option<String> {
            self.listing_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some("- review: test skill".into())
        }

        async fn invoked(&self, _agent_id: Option<&str>) -> Vec<crate::InvokedSkillEntry> {
            Vec::new()
        }

        async fn activate_skills_for_paths(
            &self,
            _file_paths: &[std::path::PathBuf],
            _cwd: &std::path::Path,
        ) -> Vec<String> {
            Vec::new()
        }
    }

    let spy = Arc::new(SpySkillsSource::default());
    let sources = ReminderSources {
        skills: Some(spy.clone()),
        ..Default::default()
    };
    let cfg = SystemReminderConfig::default();
    let out = sources
        .materialize(MaterializeContext {
            config: &cfg,
            agent_id: None,
            user_input: None,
            mentioned_paths: &[],
            recent_tools: &[],
            just_compacted: false,
            per_source_timeout: Duration::from_millis(1000),
            skill_overrides: &coco_config::SkillOverrideTiers::default(),
            skill_tool_loaded: false,
        })
        .await;

    assert!(out.skill_listing.is_none());
    assert_eq!(
        spy.listing_calls.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[tokio::test]
async fn skill_discovery_gate_skips_discovery_source_when_skill_tool_unavailable() {
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    #[derive(Debug, Default)]
    struct SpySkillsSource {
        discovery_calls: AtomicUsize,
    }

    #[async_trait]
    impl SkillsSource for SpySkillsSource {
        async fn listing(
            &self,
            _agent_id: Option<&str>,
            _tiers: &coco_config::SkillOverrideTiers,
        ) -> Option<String> {
            None
        }

        async fn invoked(&self, _agent_id: Option<&str>) -> Vec<crate::InvokedSkillEntry> {
            Vec::new()
        }

        async fn skill_discovery(
            &self,
            _user_input: &str,
            _tiers: &coco_config::SkillOverrideTiers,
        ) -> Option<coco_types::SkillDiscoveryPayload> {
            self.discovery_calls.fetch_add(1, Ordering::Relaxed);
            None
        }

        async fn activate_skills_for_paths(
            &self,
            _file_paths: &[std::path::PathBuf],
            _cwd: &std::path::Path,
        ) -> Vec<String> {
            Vec::new()
        }
    }

    // `skill_discovery` is off by default and gated on the Skill tool being
    // loaded (like `skill_listing`): the reminder tells the model to invoke
    // `Skill(...)`, which is unactionable when the tool is filtered out.
    for (skill_tool_loaded, expected_calls) in [(false, 0usize), (true, 1usize)] {
        let spy = Arc::new(SpySkillsSource::default());
        let sources = ReminderSources {
            skills: Some(spy.clone()),
            ..Default::default()
        };
        let mut cfg = SystemReminderConfig::default();
        cfg.attachments.skill_discovery = true; // off by default
        let _ = sources
            .materialize(MaterializeContext {
                config: &cfg,
                agent_id: None,
                user_input: Some("help me refactor this module"),
                mentioned_paths: &[],
                recent_tools: &[],
                just_compacted: false,
                per_source_timeout: Duration::from_millis(1000),
                skill_overrides: &coco_config::SkillOverrideTiers::default(),
                skill_tool_loaded,
            })
            .await;
        assert_eq!(
            spy.discovery_calls.load(Ordering::Relaxed),
            expected_calls,
            "skill_discovery call count wrong for skill_tool_loaded={skill_tool_loaded}"
        );
    }
}
