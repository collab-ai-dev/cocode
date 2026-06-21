use super::*;

struct FakePaneBackend {
    backend_type: BackendType,
    available: bool,
    commands: tokio::sync::Mutex<Vec<String>>,
}

impl FakePaneBackend {
    fn new(backend_type: BackendType, available: bool) -> Self {
        Self {
            backend_type,
            available,
            commands: tokio::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl PaneBackend for FakePaneBackend {
    fn backend_type(&self) -> BackendType {
        self.backend_type
    }

    fn display_name(&self) -> &str {
        "fake-pane"
    }

    fn supports_hide_show(&self) -> bool {
        true
    }

    async fn is_available(&self) -> bool {
        self.available
    }

    async fn is_running_inside(&self) -> bool {
        false
    }

    async fn create_teammate_pane(
        &self,
        _name: &str,
        _color: AgentColorName,
    ) -> crate::Result<CreatePaneResult> {
        Ok(CreatePaneResult {
            pane_id: "pane-1".into(),
            is_first_teammate: true,
        })
    }

    async fn send_command_to_pane(&self, _pane_id: &PaneId, command: &str) -> crate::Result<()> {
        self.commands.lock().await.push(command.to_string());
        Ok(())
    }

    async fn set_pane_border_color(
        &self,
        _pane_id: &PaneId,
        _color: AgentColorName,
    ) -> crate::Result<()> {
        Ok(())
    }

    async fn set_pane_title(
        &self,
        _pane_id: &PaneId,
        _name: &str,
        _color: AgentColorName,
    ) -> crate::Result<()> {
        Ok(())
    }

    async fn enable_pane_border_status(&self, _window_target: Option<&str>) -> crate::Result<()> {
        Ok(())
    }

    async fn rebalance_panes(&self, _window_target: &str, _has_leader: bool) -> crate::Result<()> {
        Ok(())
    }

    async fn kill_pane(&self, _pane_id: &PaneId) -> crate::Result<bool> {
        Ok(true)
    }

    async fn hide_pane(&self, _pane_id: &PaneId) -> crate::Result<bool> {
        Ok(true)
    }

    async fn show_pane(
        &self,
        _pane_id: &PaneId,
        _target_window_or_pane: &str,
    ) -> crate::Result<bool> {
        Ok(true)
    }
}

#[test]
fn test_system_prompt_mode_default() {
    assert_eq!(SystemPromptMode::default(), SystemPromptMode::Default);
}

#[test]
fn test_backend_detection_result_clone() {
    let result = BackendDetectionResult {
        backend_type: BackendType::Tmux,
        is_native: true,
        needs_it2_setup: false,
    };
    let cloned = result;
    assert_eq!(cloned.backend_type, BackendType::Tmux);
    assert!(cloned.is_native);
}

#[test]
fn test_backend_registry_new() {
    let _registry = BackendRegistry::new();
    let _registry2 = BackendRegistry::default();
}

#[tokio::test]
async fn test_backend_registry_reset() {
    let registry = BackendRegistry::new();
    registry.mark_in_process_fallback().await;
    assert!(registry.is_in_process_enabled().await);

    registry.reset().await;
    assert!(!registry.is_in_process_enabled().await);
}

#[tokio::test]
async fn test_backend_registry_in_process_mode() {
    let registry = BackendRegistry::new();

    // Default: not in-process
    assert!(!registry.is_in_process_enabled().await);
    assert_eq!(registry.get_resolved_teammate_mode().await, "tmux");

    // Mark fallback
    registry.mark_in_process_fallback().await;
    assert!(registry.is_in_process_enabled().await);
    assert_eq!(registry.get_resolved_teammate_mode().await, "in-process");
}

#[tokio::test]
async fn test_get_teammate_executor_none_when_empty() {
    let registry = BackendRegistry::new();
    assert!(
        registry
            .get_teammate_executor(/*prefer_in_process*/ false)
            .await
            .is_none()
    );
    assert!(
        registry
            .get_teammate_executor(/*prefer_in_process*/ true)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn test_select_teammate_executor_explicit_in_process() {
    let registry = BackendRegistry::new();
    let runner = std::sync::Arc::new(crate::runner::InProcessAgentRunner::new(
        "/tmp".into(),
        /*max_agents*/ 8,
    ));
    registry
        .register_in_process_backend(std::sync::Arc::new(crate::InProcessBackend::new(runner)))
        .await;

    let executor = registry
        .select_teammate_executor(coco_config::TeammateMode::InProcess, false)
        .await
        .expect("in-process executor");
    assert_eq!(executor.backend_type(), BackendType::InProcess);
}

#[tokio::test]
async fn test_select_teammate_executor_explicit_tmux_errors_when_unavailable() {
    let registry = BackendRegistry::new();
    let result = registry
        .select_teammate_executor(coco_config::TeammateMode::Tmux, false)
        .await;
    assert!(
        result.is_err(),
        "tmux should require a registered available pane backend"
    );
    let err = result.err().unwrap();
    assert!(err.contains("tmux"), "unexpected error: {err}");
}

#[tokio::test]
async fn test_select_teammate_executor_auto_falls_back_to_in_process() {
    let registry = BackendRegistry::new();
    let runner = std::sync::Arc::new(crate::runner::InProcessAgentRunner::new(
        "/tmp".into(),
        /*max_agents*/ 8,
    ));
    registry
        .register_in_process_backend(std::sync::Arc::new(crate::InProcessBackend::new(runner)))
        .await;
    registry.mark_in_process_fallback().await;

    let executor = registry
        .select_teammate_executor(coco_config::TeammateMode::Auto, false)
        .await
        .expect("auto fallback executor");
    assert_eq!(executor.backend_type(), BackendType::InProcess);
}

#[tokio::test]
async fn test_select_teammate_executor_explicit_tmux_uses_registered_pane() {
    // Spawns a teammate that writes a real mailbox under `teams_base_dir()`;
    // isolate `COCO_TEAMS_DIR` so a sibling test flipping it mid-body can't
    // break the `read_mailbox(...).unwrap()` below (shared `ENV_LOCK`).
    let _teams = crate::test_support::isolate_teams_dir().await;
    let team_name = format!("pane-test-{}", uuid::Uuid::new_v4().simple());

    let registry = BackendRegistry::new();
    let pane = std::sync::Arc::new(FakePaneBackend::new(BackendType::Tmux, true));
    registry.register_pane_backend(pane.clone()).await;

    let executor = registry
        .select_teammate_executor(coco_config::TeammateMode::Tmux, false)
        .await
        .expect("tmux pane executor");
    assert_eq!(executor.backend_type(), BackendType::Tmux);

    let result = executor
        .spawn(TeammateSpawnConfig {
            name: "researcher".into(),
            team_name: team_name.clone(),
            prompt: "initial assignment".into(),
            cwd: "/tmp".into(),
            ..Default::default()
        })
        .await;
    assert!(result.success, "spawn failed: {:?}", result.error);
    assert_eq!(result.pane_id.as_deref(), Some("pane-1"));

    let commands = pane.commands.lock().await;
    assert_eq!(commands.len(), 1);
    // Identity rides COCO_* env, not CLI flags (clap rejects identity flags).
    assert!(
        commands[0].contains(&format!("COCO_TEAM_NAME={team_name}")),
        "pane command should carry teammate identity via env: {}",
        commands[0]
    );
    assert!(
        !commands[0].contains("--team-name="),
        "identity must not be a CLI flag: {}",
        commands[0]
    );

    let mailbox = crate::mailbox::read_mailbox("researcher", &team_name).unwrap();
    assert_eq!(mailbox.len(), 1);
    assert_eq!(mailbox[0].text, "initial assignment");
    assert_eq!(mailbox[0].summary.as_deref(), Some("initial task"));

    let _ = crate::team_file::cleanup_team_directories(&team_name);
}

#[test]
fn test_is_inside_tmux() {
    // This test's result depends on the environment, just ensure no panic
    let _result = is_inside_tmux();
}

#[test]
fn test_get_leader_pane_id() {
    let _result = get_leader_pane_id();
}

#[test]
fn test_is_in_iterm2() {
    let _result = is_in_iterm2();
}

#[tokio::test]
async fn test_detect_backend_no_panic() {
    // Detection should complete without panicking regardless of env
    let startup_env = StartupPaneEnv::capture();
    let _result = detect_backend_impl(&startup_env).await;
}
