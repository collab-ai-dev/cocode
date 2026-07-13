use std::{future::Future, pin::Pin, sync::Arc};

use super::*;
use crate::app_server_host::{CliInitializeBootstrap, TurnRunner};

struct TestTurnRunner;

impl TurnRunner for TestTurnRunner {
    fn run_turn<'a>(
        &'a self,
        _session: crate::session_runtime::SessionHandle,
        _app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
        _params: coco_types::TurnStartParams,
        _turn_id: coco_types::TurnId,
        _event_tx: tokio::sync::mpsc::Sender<coco_types::CoreEvent>,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn constructor_installs_startup_inputs_without_late_mutation() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let cwd = home.path().join("workspace");
    let manager = Arc::new(coco_session::SessionManager::new(
        home.path().join("sessions"),
    ));
    let bootstrap = Arc::new(CliInitializeBootstrap::new("default".into()).with_cwd(cwd.clone()));
    let runner: Arc<dyn TurnRunner> = Arc::new(TestTurnRunner);

    let state = AppServerHostState::new(HostInputs {
        startup_cwd: Some(cwd.clone()),
        initialize_bootstrap: Some(bootstrap),
        session_manager: Some(Arc::clone(&manager)),
        bypass_permissions_available: true,
        turn_runner: Some(Arc::clone(&runner)),
        ..Default::default()
    });

    let observed_cwd = match state.workspace_cwd().await {
        Ok(cwd) => cwd,
        Err(_) => panic!("workspace cwd should be available"),
    };
    assert_eq!(observed_cwd, cwd);
    assert!(state.initialize_bootstrap_snapshot().await.is_some());
    assert!(state.bypass_permissions_available());
    let snapshot = state
        .session_manager_snapshot()
        .await
        .expect("session manager snapshot");
    assert!(Arc::ptr_eq(&snapshot, &manager));
    let runner_snapshot = state.turn_runner_snapshot().await;
    assert!(Arc::ptr_eq(&runner_snapshot, &runner));
}
