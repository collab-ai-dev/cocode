use std::collections::HashMap;
use std::sync::Arc;

use crate::CheckedFileSystem;
use crate::ExecBackend;
use crate::ExecServerError;
use crate::ExecutorFileSystem;
use crate::HttpClient;
use crate::client::LazyRemoteExecServerClient;
use crate::client::http_client::ReqwestHttpClient;
use crate::client_api::DEFAULT_REMOTE_EXEC_SERVER_CONNECT_TIMEOUT;
use crate::client_api::ExecServerTransportParams;
use crate::local_file_system::LocalFileSystem;
use crate::local_process::LocalProcess;
use crate::protocol::EnvironmentInfo;
use crate::remote_file_system::RemoteFileSystem;
use crate::remote_process::RemoteProcess;
use crate::runtime_paths::ExecServerRuntimePaths;

pub const CODEX_EXEC_SERVER_URL_ENV_VAR: &str = "CODEX_EXEC_SERVER_URL";
pub const LOCAL_ENVIRONMENT_ID: &str = "local";
pub const REMOTE_ENVIRONMENT_ID: &str = "remote";

#[derive(Clone)]
pub struct EnvironmentManager {
    default_environment: Option<String>,
    environments: HashMap<String, Arc<Environment>>,
}

impl EnvironmentManager {
    pub fn without_environments() -> Self {
        Self {
            default_environment: None,
            environments: HashMap::new(),
        }
    }

    pub fn local(runtime_paths: ExecServerRuntimePaths) -> Self {
        let mut environments = HashMap::new();
        environments.insert(
            LOCAL_ENVIRONMENT_ID.to_string(),
            Arc::new(Environment::local(runtime_paths)),
        );
        Self {
            default_environment: Some(LOCAL_ENVIRONMENT_ID.to_string()),
            environments,
        }
    }

    pub async fn from_env(runtime_paths: ExecServerRuntimePaths) -> Result<Self, ExecServerError> {
        match std::env::var(CODEX_EXEC_SERVER_URL_ENV_VAR) {
            Ok(value) if value == "none" => Ok(Self::without_environments()),
            Ok(value) if !value.trim().is_empty() => Self::remote(value).await,
            Ok(_) | Err(std::env::VarError::NotPresent) => Ok(Self::local(runtime_paths)),
            Err(std::env::VarError::NotUnicode(_)) => Err(ExecServerError::Protocol(format!(
                "{CODEX_EXEC_SERVER_URL_ENV_VAR} is not valid unicode"
            ))),
        }
    }

    pub async fn remote(exec_server_url: String) -> Result<Self, ExecServerError> {
        let mut environments = HashMap::new();
        environments.insert(
            REMOTE_ENVIRONMENT_ID.to_string(),
            Arc::new(Environment::remote(exec_server_url).await?),
        );
        Ok(Self {
            default_environment: Some(REMOTE_ENVIRONMENT_ID.to_string()),
            environments,
        })
    }

    pub fn default_environment_id(&self) -> Option<&str> {
        self.default_environment.as_deref()
    }

    pub fn get_environment(&self, id: &str) -> Option<Arc<Environment>> {
        self.environments.get(id).cloned()
    }

    pub fn environment_ids(&self) -> Vec<String> {
        let mut ids = self.environments.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        ids
    }
}

pub struct Environment {
    info: EnvironmentInfo,
    exec: Arc<dyn ExecBackend>,
    filesystem: Arc<dyn ExecutorFileSystem>,
    checked_filesystem: Option<Arc<dyn CheckedFileSystem>>,
    http_client: Arc<dyn HttpClient>,
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl Environment {
    pub fn local(runtime_paths: ExecServerRuntimePaths) -> Self {
        let filesystem = Arc::new(LocalFileSystem::with_runtime_paths(runtime_paths));
        let checked_filesystem = if cfg!(target_os = "linux") {
            Some(filesystem.clone() as Arc<dyn CheckedFileSystem>)
        } else {
            None
        };
        Self {
            info: EnvironmentInfo::local(),
            exec: Arc::new(LocalProcess::default()),
            filesystem,
            checked_filesystem,
            http_client: Arc::new(ReqwestHttpClient),
        }
    }

    pub async fn remote(exec_server_url: String) -> Result<Self, ExecServerError> {
        if !(exec_server_url.starts_with("ws://") || exec_server_url.starts_with("wss://")) {
            return Err(ExecServerError::Protocol(format!(
                "unsupported exec-server URL `{exec_server_url}`; expected ws:// or wss://"
            )));
        }
        let client = LazyRemoteExecServerClient::new(ExecServerTransportParams::websocket_url(
            exec_server_url,
            DEFAULT_REMOTE_EXEC_SERVER_CONNECT_TIMEOUT,
        ));
        let info = client.environment_info().await?;
        let exec = Arc::new(RemoteProcess::new(client.clone()));
        let filesystem = Arc::new(RemoteFileSystem::new(client.clone()));
        let checked_filesystem = info
            .capabilities
            .checked_file_mutations
            .then(|| filesystem.clone() as Arc<dyn CheckedFileSystem>);
        let http_client = Arc::new(client);
        Ok(Self {
            info,
            exec,
            filesystem,
            checked_filesystem,
            http_client,
        })
    }

    pub fn get_info(&self) -> EnvironmentInfo {
        self.info.clone()
    }

    pub fn get_exec(&self) -> Arc<dyn ExecBackend> {
        Arc::clone(&self.exec)
    }

    pub fn get_filesystem(&self) -> Arc<dyn ExecutorFileSystem> {
        Arc::clone(&self.filesystem)
    }

    pub fn get_checked_filesystem(&self) -> Option<Arc<dyn CheckedFileSystem>> {
        self.checked_filesystem.clone()
    }

    pub fn get_http_client(&self) -> Arc<dyn HttpClient> {
        Arc::clone(&self.http_client)
    }
}
