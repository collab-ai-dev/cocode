use std::path::PathBuf;
use std::sync::Arc;

use coco_commands::CommandRegistry;
use coco_config::RuntimeConfig;
use coco_query::CommandQueue;
use coco_tool_runtime::MailboxHandleRef;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolRegistry;
use tokio::sync::RwLock;

/// Process/shared execution resources installed on every engine built for a
/// session.
///
/// Kept separate from mutable per-session state so the runtime split can move
/// these registries behind a dedicated owner without changing turn behavior.
#[derive(Clone)]
pub(in crate::session::session_runtime) struct SessionExecutionResources {
    pub(in crate::session::session_runtime) tools: Arc<ToolRegistry>,
    pub(in crate::session::session_runtime) model_runtimes:
        Arc<coco_inference::ModelRuntimeRegistry>,
    pub(in crate::session::session_runtime) execution_profile:
        crate::session::session_runtime::SessionExecutionProfile,
}

impl SessionExecutionResources {
    pub(in crate::session::session_runtime) fn new(
        tools: Arc<ToolRegistry>,
        model_runtimes: Arc<coco_inference::ModelRuntimeRegistry>,
        execution_profile: crate::session::session_runtime::SessionExecutionProfile,
    ) -> Self {
        Self {
            tools,
            model_runtimes,
            execution_profile,
        }
    }

    pub(in crate::session::session_runtime) fn execution_profile(
        &self,
    ) -> crate::session::session_runtime::SessionExecutionProfile {
        self.execution_profile
    }

    pub(in crate::session::session_runtime) fn tools(&self) -> &Arc<ToolRegistry> {
        &self.tools
    }

    pub(in crate::session::session_runtime) fn model_runtimes(
        &self,
    ) -> Arc<coco_inference::ModelRuntimeRegistry> {
        self.model_runtimes.clone()
    }
}

/// Session-owned configuration snapshot and reload publisher.
///
/// Runtime construction picks the config home and per-session folded
/// `RuntimeConfig`; hot-reload paths subscribe through the paired reloader.
pub(in crate::session::session_runtime) struct SessionConfigResources {
    pub(in crate::session::session_runtime) config_home: PathBuf,
    pub(in crate::session::session_runtime) runtime_config: Arc<RuntimeConfig>,
    pub(in crate::session::session_runtime) config_reloader:
        Option<coco_config_reload::RuntimeReloader>,
}

impl SessionConfigResources {
    pub(in crate::session::session_runtime) fn new(
        config_home: PathBuf,
        runtime_config: Arc<RuntimeConfig>,
        config_reloader: Option<coco_config_reload::RuntimeReloader>,
    ) -> Self {
        Self {
            config_home,
            runtime_config,
            config_reloader,
        }
    }

    pub(in crate::session::session_runtime) fn config_home(&self) -> &PathBuf {
        &self.config_home
    }

    pub(in crate::session::session_runtime) fn runtime_config(&self) -> &Arc<RuntimeConfig> {
        &self.runtime_config
    }

    pub(in crate::session::session_runtime) fn runtime_publisher(
        &self,
    ) -> Option<Arc<coco_config::RuntimePublisher>> {
        self.config_reloader
            .as_ref()
            .map(coco_config_reload::RuntimeReloader::publisher)
    }

    pub(in crate::session::session_runtime) fn subscribe_config_changes(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<coco_config_reload::ConfigChange>> {
        self.config_reloader
            .as_ref()
            .map(coco_config_reload::RuntimeReloader::subscribe_changes)
    }

    pub(in crate::session::session_runtime) fn subscribe_config_reload_errors(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<coco_config_reload::ConfigReloadError>> {
        self.config_reloader
            .as_ref()
            .map(coco_config_reload::RuntimeReloader::subscribe_errors)
    }
}

/// Session command and skill catalog resources.
///
/// These are loaded from the same per-session project/config fold and reloaded
/// together when plugin or skill settings change.
#[derive(Clone)]
pub(in crate::session::session_runtime) struct SessionCatalogResources {
    pub(in crate::session::session_runtime) command_registry: Arc<RwLock<Arc<CommandRegistry>>>,
    pub(in crate::session::session_runtime) skill_manager: Arc<coco_skills::SkillManager>,
}

impl SessionCatalogResources {
    pub(in crate::session::session_runtime) fn new(
        command_registry: Arc<RwLock<Arc<CommandRegistry>>>,
        skill_manager: Arc<coco_skills::SkillManager>,
    ) -> Self {
        Self {
            command_registry,
            skill_manager,
        }
    }

    pub(in crate::session::session_runtime) fn command_registry(
        &self,
    ) -> &Arc<RwLock<Arc<CommandRegistry>>> {
        &self.command_registry
    }

    pub(in crate::session::session_runtime) fn skill_manager(
        &self,
    ) -> &Arc<coco_skills::SkillManager> {
        &self.skill_manager
    }
}

/// Per-turn engine plumbing shared by engines built for one session.
#[derive(Clone)]
pub(in crate::session::session_runtime) struct SessionTurnResources {
    pub(in crate::session::session_runtime) schedule_store: coco_tool_runtime::ScheduleStoreRef,
    pub(in crate::session::session_runtime) side_query: coco_tool_runtime::SideQueryHandle,
    pub(in crate::session::session_runtime) usage_accounting:
        coco_query::usage_accounting::UsageAccounting,
    pub(in crate::session::session_runtime) mailbox: MailboxHandleRef,
    pub(in crate::session::session_runtime) permission_bridge: Option<ToolPermissionBridgeRef>,
}

impl SessionTurnResources {
    pub(in crate::session::session_runtime) fn new(
        schedule_store: coco_tool_runtime::ScheduleStoreRef,
        side_query: coco_tool_runtime::SideQueryHandle,
        usage_accounting: coco_query::usage_accounting::UsageAccounting,
        mailbox: MailboxHandleRef,
        permission_bridge: Option<ToolPermissionBridgeRef>,
    ) -> Self {
        Self {
            schedule_store,
            side_query,
            usage_accounting,
            mailbox,
            permission_bridge,
        }
    }

    pub(in crate::session::session_runtime) fn schedule_store(
        &self,
    ) -> coco_tool_runtime::ScheduleStoreRef {
        self.schedule_store.clone()
    }

    pub(in crate::session::session_runtime) fn side_query(
        &self,
    ) -> coco_tool_runtime::SideQueryHandle {
        self.side_query.clone()
    }

    pub(in crate::session::session_runtime) fn usage_accounting(
        &self,
    ) -> coco_query::usage_accounting::UsageAccounting {
        self.usage_accounting.clone()
    }

    pub(in crate::session::session_runtime) fn mailbox(&self) -> MailboxHandleRef {
        self.mailbox.clone()
    }

    pub(in crate::session::session_runtime) fn permission_bridge(
        &self,
    ) -> Option<ToolPermissionBridgeRef> {
        self.permission_bridge.clone()
    }
}

type SessionAttachmentRx =
    Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<coco_messages::AttachmentMessage>>>;

/// Cross-turn command and attachment channels shared by rebuilt engines.
#[derive(Clone)]
pub(in crate::session::session_runtime) struct SessionCommandResources {
    pub(in crate::session::session_runtime) attachment_tx:
        tokio::sync::mpsc::UnboundedSender<coco_messages::AttachmentMessage>,
    pub(in crate::session::session_runtime) attachment_rx: SessionAttachmentRx,
    pub(in crate::session::session_runtime) command_queue: CommandQueue,
}

impl SessionCommandResources {
    pub(in crate::session::session_runtime) fn new(
        attachment_tx: tokio::sync::mpsc::UnboundedSender<coco_messages::AttachmentMessage>,
        attachment_rx: SessionAttachmentRx,
        command_queue: CommandQueue,
    ) -> Self {
        Self {
            attachment_tx,
            attachment_rx,
            command_queue,
        }
    }

    pub(in crate::session::session_runtime) fn attachment_emitter(
        &self,
    ) -> coco_messages::AttachmentEmitter {
        coco_messages::AttachmentEmitter::new(self.attachment_tx.clone())
    }

    pub(in crate::session::session_runtime) fn attachment_channel(
        &self,
    ) -> (
        tokio::sync::mpsc::UnboundedSender<coco_messages::AttachmentMessage>,
        SessionAttachmentRx,
    ) {
        (self.attachment_tx.clone(), self.attachment_rx.clone())
    }

    pub(in crate::session::session_runtime) fn command_queue(&self) -> &CommandQueue {
        &self.command_queue
    }
}
