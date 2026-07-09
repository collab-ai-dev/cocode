use std::sync::Arc;

use coco_hooks::HookRegistry;
use coco_session::SessionManager;
use coco_types::ModelSpec;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::process_runtime::ProcessRuntime;
use crate::project_services::ProjectServices;

/// Project/process service resources used by a session.
///
/// `ProcessRuntime` owns the process-level project registry, while
/// `ProjectServices` is the project-root snapshot selected for this session.
#[derive(Clone)]
pub(in crate::session_runtime) struct SessionProjectResources {
    pub(in crate::session_runtime) process_runtime: Arc<ProcessRuntime>,
    pub(in crate::session_runtime) project_services: Arc<ProjectServices>,
}

impl SessionProjectResources {
    pub(in crate::session_runtime) fn new(
        process_runtime: Arc<ProcessRuntime>,
        project_services: Arc<ProjectServices>,
    ) -> Self {
        Self {
            process_runtime,
            project_services,
        }
    }

    pub(in crate::session_runtime) fn process_runtime(&self) -> &Arc<ProcessRuntime> {
        &self.process_runtime
    }

    pub(in crate::session_runtime) fn project_services(&self) -> &Arc<ProjectServices> {
        &self.project_services
    }
}

/// Session storage and transcript persistence resources.
///
/// These values are process-backed but scoped to the session's project/cwd
/// choice at build time. Keeping them behind one owner is a step toward
/// splitting the fused runtime into smaller lifetime-specific containers.
#[derive(Clone)]
pub(in crate::session_runtime) struct SessionPersistenceResources {
    pub(in crate::session_runtime) session_manager: Arc<SessionManager>,
    pub(in crate::session_runtime) project_paths: Arc<coco_paths::ProjectPaths>,
    pub(in crate::session_runtime) transcript_store: Arc<dyn coco_session::SessionStore>,
    pub(in crate::session_runtime) persist_session: bool,
}

impl SessionPersistenceResources {
    pub(in crate::session_runtime) fn new(
        session_manager: Arc<SessionManager>,
        project_paths: Arc<coco_paths::ProjectPaths>,
        transcript_store: Arc<dyn coco_session::SessionStore>,
        persist_session: bool,
    ) -> Self {
        Self {
            session_manager,
            project_paths,
            transcript_store,
            persist_session,
        }
    }

    pub(in crate::session_runtime) fn session_manager(&self) -> &Arc<SessionManager> {
        &self.session_manager
    }

    pub(in crate::session_runtime) fn project_paths(&self) -> &Arc<coco_paths::ProjectPaths> {
        &self.project_paths
    }

    pub(in crate::session_runtime) fn transcript_store(
        &self,
    ) -> &Arc<dyn coco_session::SessionStore> {
        &self.transcript_store
    }

    pub(in crate::session_runtime) fn persist_session(&self) -> bool {
        self.persist_session
    }
}

/// Session lifecycle resources that should live and drop with the runtime.
pub(in crate::session_runtime) struct SessionLifecycleResources {
    pub(in crate::session_runtime) cancel: CancellationToken,
    pub(in crate::session_runtime) pid_registry: Option<coco_session::SessionRegistry>,
}

impl SessionLifecycleResources {
    pub(in crate::session_runtime) fn new(
        cancel: CancellationToken,
        pid_registry: Option<coco_session::SessionRegistry>,
    ) -> Self {
        Self {
            cancel,
            pid_registry,
        }
    }

    pub(in crate::session_runtime) fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub(in crate::session_runtime) fn update_session_registry_name(&self, name: &str) {
        if let Some(reg) = self.pid_registry.as_ref() {
            reg.update_session_name(name);
        }
    }
}

/// Hook orchestration resources installed on every engine and used by
/// runtime-level hook firing.
///
/// Kept as a dedicated owner so the runtime split can move hook orchestration
/// behind its own lifecycle without keeping these handles as flat runtime
/// fields.
#[derive(Clone)]
pub(in crate::session_runtime) struct SessionHookResources {
    pub(in crate::session_runtime) hook_registry: Arc<HookRegistry>,
    pub(in crate::session_runtime) hook_llm_handle: Arc<coco_query::hook_llm::QueryHookLlm>,
    pub(in crate::session_runtime) sync_hook_buffer: coco_hooks::SyncHookEventBuffer,
    pub(in crate::session_runtime) async_hook_registry:
        Arc<coco_hooks::async_registry::AsyncHookRegistry>,
    pub(in crate::session_runtime) file_changed_watcher:
        Arc<RwLock<Option<crate::file_changed_watcher::FileChangedHookWatcher>>>,
}

impl SessionHookResources {
    pub(in crate::session_runtime) fn new(
        hook_registry: Arc<HookRegistry>,
        hook_llm_handle: Arc<coco_query::hook_llm::QueryHookLlm>,
        sync_hook_buffer: coco_hooks::SyncHookEventBuffer,
        async_hook_registry: Arc<coco_hooks::async_registry::AsyncHookRegistry>,
        file_changed_watcher: Arc<
            RwLock<Option<crate::file_changed_watcher::FileChangedHookWatcher>>,
        >,
    ) -> Self {
        Self {
            hook_registry,
            hook_llm_handle,
            sync_hook_buffer,
            async_hook_registry,
            file_changed_watcher,
        }
    }

    pub(in crate::session_runtime) fn registry(&self) -> Arc<HookRegistry> {
        self.hook_registry.clone()
    }

    pub(in crate::session_runtime) fn llm_handle(&self) -> Arc<coco_query::hook_llm::QueryHookLlm> {
        self.hook_llm_handle.clone()
    }

    pub(in crate::session_runtime) fn sync_buffer(&self) -> coco_hooks::SyncHookEventBuffer {
        self.sync_hook_buffer.clone()
    }

    pub(in crate::session_runtime) fn async_registry(
        &self,
    ) -> Arc<coco_hooks::async_registry::AsyncHookRegistry> {
        self.async_hook_registry.clone()
    }

    pub(in crate::session_runtime) fn file_changed_watcher(
        &self,
    ) -> Arc<RwLock<Option<crate::file_changed_watcher::FileChangedHookWatcher>>> {
        self.file_changed_watcher.clone()
    }
}

#[derive(Clone)]
pub(in crate::session_runtime) struct SessionTitleResources {
    pub(in crate::session_runtime) fast_model_spec: Option<ModelSpec>,
    pub(in crate::session_runtime) auto_title_enabled: bool,
}

impl SessionTitleResources {
    pub(in crate::session_runtime) fn new(
        fast_model_spec: Option<ModelSpec>,
        auto_title_enabled: bool,
    ) -> Self {
        Self {
            fast_model_spec,
            auto_title_enabled,
        }
    }

    pub(in crate::session_runtime) fn fast_model_spec(&self) -> Option<&ModelSpec> {
        self.fast_model_spec.as_ref()
    }

    pub(in crate::session_runtime) fn auto_title_enabled(&self) -> bool {
        self.auto_title_enabled
    }
}
