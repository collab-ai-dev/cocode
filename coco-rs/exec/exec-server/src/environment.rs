use std::collections::HashMap;
use std::sync::Arc;

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
use crate::protocol::ShellInfo;
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
        Self {
            info: local_environment_info(),
            exec: Arc::new(LocalProcess::default()),
            filesystem: Arc::new(LocalFileSystem::with_runtime_paths(runtime_paths)),
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
        let http_client = Arc::new(client);
        Ok(Self {
            info,
            exec,
            filesystem,
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

    pub fn get_http_client(&self) -> Arc<dyn HttpClient> {
        Arc::clone(&self.http_client)
    }
}

fn local_environment_info() -> EnvironmentInfo {
    let shell_path = std::env::var("SHELL")
        .or_else(|_| std::env::var("COMSPEC"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "cmd.exe".to_string()
            } else {
                "/bin/sh".to_string()
            }
        });
    let name = std::path::Path::new(&shell_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("sh")
        .to_string();
    EnvironmentInfo {
        shell: ShellInfo {
            name,
            path: shell_path,
        },
    }
}
