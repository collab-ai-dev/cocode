use super::*;

impl SessionHandle {
    pub async fn reload_plugin_mcp_servers(&self) -> usize {
        self.runtime.reload_plugin_mcp_servers().await
    }

    pub fn mcp_reconnect_key(&self) -> u64 {
        self.runtime.mcp_reconnect_key()
    }

    pub async fn mcp_status_result(&self) -> Option<coco_types::McpStatusResult> {
        let manager = {
            let manager = self.runtime.current_mcp_manager().await?;
            manager.lock().await.clone()
        };
        let names = manager.registered_server_names();
        let mut statuses = Vec::with_capacity(names.len());
        for name in &names {
            let state = manager.get_state(name).await;
            let (status, error, advertised) = match state {
                Some(coco_mcp::McpConnectionState::Connected(server)) => (
                    coco_types::McpConnectionStatus::Connected,
                    None,
                    server.tools.len() as i32,
                ),
                Some(coco_mcp::McpConnectionState::Pending { .. }) => {
                    (coco_types::McpConnectionStatus::Pending, None, 0)
                }
                Some(coco_mcp::McpConnectionState::Failed { error }) => {
                    (coco_types::McpConnectionStatus::Failed, Some(error), 0)
                }
                Some(coco_mcp::McpConnectionState::NeedsAuth { .. }) => {
                    (coco_types::McpConnectionStatus::NeedsAuth, None, 0)
                }
                Some(coco_mcp::McpConnectionState::Disabled) => {
                    (coco_types::McpConnectionStatus::Disabled, None, 0)
                }
                None => (coco_types::McpConnectionStatus::Disconnected, None, 0),
            };
            let registration = self.mcp_registration_status(name).await;
            statuses.push(coco_types::McpServerStatus {
                name: name.clone(),
                status,
                tool_count: registration
                    .as_ref()
                    .map_or(advertised, |report| report.tool_count),
                error,
                skipped_tools: registration
                    .as_ref()
                    .map_or_else(Vec::new, |report| report.skipped_tools.clone()),
                tombstoned_tools: registration
                    .map_or_else(Vec::new, |report| report.tombstoned_tools),
            });
        }
        Some(coco_types::McpStatusResult {
            mcp_servers: statuses,
        })
    }

    pub async fn set_dynamic_mcp_servers(
        &self,
        servers: Vec<(String, coco_mcp::McpServerConfig)>,
    ) -> Option<Vec<String>> {
        let manager = self.runtime.current_mcp_manager().await?;
        let mut manager = manager.lock().await;
        let mut added = Vec::with_capacity(servers.len());
        for (name, config) in servers {
            manager.register_server(coco_mcp::ScopedMcpServerConfig {
                name: name.clone(),
                config,
                scope: coco_mcp::ConfigScope::Dynamic,
                plugin_source: None,
            });
            added.push(name);
        }
        Some(added)
    }

    pub async fn install_client_mcp_route(&self, route: coco_mcp::ClientRouteMessage) -> bool {
        let Some(manager) = self.runtime.current_mcp_manager().await else {
            return false;
        };
        manager.lock().await.set_client_route_message(route);
        true
    }

    pub async fn register_client_mcp_servers(&self, server_names: &[String]) -> bool {
        let Some(manager) = self.runtime.current_mcp_manager().await else {
            return false;
        };
        let mut manager = manager.lock().await;
        for name in server_names {
            manager.register_server(coco_mcp::ScopedMcpServerConfig {
                name: name.clone(),
                config: coco_mcp::McpServerConfig::ClientHosted(
                    coco_mcp::types::McpClientHostedConfig { name: name.clone() },
                ),
                scope: coco_mcp::ConfigScope::Dynamic,
                plugin_source: None,
            });
        }
        true
    }

    pub async fn reconnect_mcp_server(
        &self,
        server_name: &str,
        send_elicitation: coco_mcp::SendElicitation,
    ) -> Option<super::SessionMcpConnectionChange> {
        let manager = {
            let manager = self.runtime.current_mcp_manager().await?;
            manager.lock().await.clone()
        };
        manager.disconnect(server_name).await;
        Some(
            self.connect_mcp_server_with_manager(&manager, server_name, send_elicitation)
                .await,
        )
    }

    pub async fn set_mcp_server_enabled(
        &self,
        server_name: &str,
        enabled: bool,
        send_elicitation: Option<coco_mcp::SendElicitation>,
    ) -> Option<super::SessionMcpConnectionChange> {
        let manager = {
            let manager = self.runtime.current_mcp_manager().await?;
            manager.lock().await.clone()
        };
        if enabled {
            let send_elicitation = send_elicitation?;
            Some(
                self.connect_mcp_server_with_manager(&manager, server_name, send_elicitation)
                    .await,
            )
        } else {
            manager.disconnect(server_name).await;
            self.deregister_mcp_server(server_name).await;
            Some(super::SessionMcpConnectionChange::Disconnected)
        }
    }

    async fn connect_mcp_server_with_manager(
        &self,
        manager: &coco_mcp::McpConnectionManager,
        server_name: &str,
        send_elicitation: coco_mcp::SendElicitation,
    ) -> super::SessionMcpConnectionChange {
        match manager.connect(server_name, send_elicitation).await {
            Ok(()) => {
                let schemas = collect_connected_mcp_server_schemas(manager, server_name).await;
                self.register_mcp_tools(server_name, schemas).await;
                super::SessionMcpConnectionChange::Connected
            }
            Err(error) => {
                if let Some((transport, url)) =
                    mcp_needs_auth_descriptor(manager, server_name).await
                {
                    self.register_mcp_auth_tool(server_name, &transport, url.as_deref());
                    super::SessionMcpConnectionChange::NeedsAuth { transport, url }
                } else {
                    super::SessionMcpConnectionChange::Failed(error.to_string())
                }
            }
        }
    }

    pub async fn register_mcp_tools(
        &self,
        server_name: &str,
        schemas: Vec<coco_tool_runtime::McpToolSchema>,
    ) {
        let report = coco_tools::register_mcp_tools(self.tools(), server_name, schemas);
        self.record_mcp_registration_report(server_name, report)
            .await;
    }

    pub fn register_mcp_auth_tool(&self, server_name: &str, transport: &str, url: Option<&str>) {
        coco_tools::register_mcp_auth_tool(self.tools(), server_name, transport, url);
    }

    pub async fn deregister_mcp_server(&self, server_name: &str) {
        coco_tools::deregister_mcp_server(self.tools(), server_name);
        self.clear_mcp_registration_status(server_name).await;
    }

    pub(crate) async fn record_mcp_registration_report(
        &self,
        server_name: &str,
        report: coco_tools::RegisterMcpToolsReport,
    ) {
        let status = super::McpRegistrationStatus {
            tool_count: report.registered.len() as i32,
            skipped_tools: report
                .skipped
                .into_iter()
                .map(|skipped| coco_types::McpSkippedToolStatus {
                    tool_name: skipped.tool_name,
                    error: skipped.error.to_string(),
                })
                .collect(),
            tombstoned_tools: report
                .tombstones
                .into_iter()
                .map(|tool_id| tool_id.to_string())
                .collect(),
        };
        self.runtime
            .integration_resources
            .mcp_registration_reports()
            .write()
            .await
            .insert(server_name.to_string(), status);
    }

    pub(crate) async fn mcp_registration_status(
        &self,
        server_name: &str,
    ) -> Option<super::McpRegistrationStatus> {
        self.runtime
            .integration_resources
            .mcp_registration_reports()
            .read()
            .await
            .get(server_name)
            .cloned()
    }

    pub(crate) async fn clear_mcp_registration_status(&self, server_name: &str) {
        self.runtime
            .integration_resources
            .mcp_registration_reports()
            .write()
            .await
            .remove(server_name);
    }
}

async fn collect_connected_mcp_server_schemas(
    manager: &coco_mcp::McpConnectionManager,
    server_name: &str,
) -> Vec<coco_tool_runtime::McpToolSchema> {
    let Some(coco_mcp::McpConnectionState::Connected(server)) =
        manager.get_state(server_name).await
    else {
        return vec![];
    };
    server
        .tools
        .iter()
        .map(|tool| coco_tool_runtime::McpToolSchema {
            server_name: server_name.to_string(),
            tool_name: tool.name.clone(),
            description: tool.description.clone(),
            annotations: coco_tool_runtime::McpToolAnnotations::from_input_schema_meta(
                &tool.input_schema,
            ),
            input_schema: tool.input_schema.clone(),
        })
        .collect()
}

async fn mcp_needs_auth_descriptor(
    manager: &coco_mcp::McpConnectionManager,
    server_name: &str,
) -> Option<(String, Option<String>)> {
    match manager.get_state(server_name).await {
        Some(coco_mcp::McpConnectionState::NeedsAuth { .. }) => {
            manager.auth_descriptor(server_name)
        }
        _ => None,
    }
}
