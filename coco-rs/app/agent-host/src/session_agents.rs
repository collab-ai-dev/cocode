use crate::session_runtime::SessionHandle;

#[derive(Debug, thiserror::Error)]
pub enum DeleteAgentFileError {
    #[error("{0}")]
    Io(String),
}

#[derive(Debug)]
pub enum CreateAgentError {
    NonWritableSource(coco_types::AgentSource),
    AlreadyExists(std::path::PathBuf),
    Io(String),
}

impl CreateAgentError {
    pub fn to_user_string(&self) -> String {
        match self {
            Self::NonWritableSource(source) => {
                format!("source {source:?} is not writable from the wizard")
            }
            Self::AlreadyExists(path) => {
                format!("agent file already exists at {}", path.display())
            }
            Self::Io(message) => message.clone(),
        }
    }
}

pub async fn prepare_agent_create(
    session: &SessionHandle,
    name: &str,
    description: &str,
    source: coco_types::AgentSource,
) -> Result<std::path::PathBuf, CreateAgentError> {
    let snapshot = session.agent_catalog_snapshot().await;
    let color = coco_subagent::next_unused_color(&snapshot);

    let name_owned = name.to_string();
    let description_owned = description.to_string();
    let cwd = session.workspace_cwd().await;
    let blocking =
        tokio::task::spawn_blocking(move || -> Result<std::path::PathBuf, CreateAgentError> {
            let config_home = coco_config::global_config::config_home();
            let dir = coco_subagent::resolve_writable_agent_dir(source, &config_home, &cwd)
                .ok_or(CreateAgentError::NonWritableSource(source))?;
            std::fs::create_dir_all(&dir).map_err(|err| CreateAgentError::Io(err.to_string()))?;
            let path = dir.join(format!("{name_owned}.md"));
            if path.exists() {
                return Err(CreateAgentError::AlreadyExists(path));
            }
            let template = build_agent_template(&name_owned, &description_owned, color);
            std::fs::write(&path, template).map_err(|err| CreateAgentError::Io(err.to_string()))?;
            Ok(path)
        })
        .await
        .map_err(|join_err| CreateAgentError::Io(format!("write task panicked: {join_err}")))??;

    session.reload_agent_catalog().await;
    Ok(blocking)
}

pub async fn reload_agent_catalog(session: &SessionHandle) {
    session.reload_agent_catalog().await;
}

pub async fn delete_agent_file(
    session: &SessionHandle,
    path: std::path::PathBuf,
) -> Result<(), DeleteAgentFileError> {
    tokio::task::spawn_blocking(move || std::fs::remove_file(path))
        .await
        .map_err(|join_err| DeleteAgentFileError::Io(format!("delete task panicked: {join_err}")))?
        .map_err(|err| DeleteAgentFileError::Io(err.to_string()))?;

    reload_agent_catalog(session).await;
    Ok(())
}

pub fn build_agent_template(
    name: &str,
    description: &str,
    color: Option<coco_types::AgentColorName>,
) -> String {
    let description_yaml = yaml_single_quote(description);
    let color_line = match color {
        Some(color) => format!("color: {}\n", color.as_str()),
        None => String::new(),
    };
    format!(
        "---\n\
         name: {name}\n\
         description: {description_yaml}\n\
         {color_line}\
         ---\n\
         \n\
         # {name}\n\
         \n\
         <!-- Describe how this agent should behave. Frontmatter \
         fields you can add: tools, model, memory, isolation, \
         background, maxTurns, initialPrompt. -->\n",
    )
}

pub fn yaml_single_quote(value: &str) -> String {
    let escaped = value.replace('\'', "''");
    format!("'{escaped}'")
}

#[cfg(test)]
#[path = "session_agents.test.rs"]
mod tests;
