use std::collections::HashMap;

use tokio::sync::RwLock;

#[derive(Default)]
pub(super) struct McpRegistrationState {
    reports: RwLock<HashMap<String, coco_tools::RegisterMcpToolsReport>>,
}

pub(super) struct McpRegistrationStatusProjection {
    pub(super) tool_count: i32,
    pub(super) skipped_tools: Vec<coco_types::McpSkippedToolStatus>,
    pub(super) tombstoned_tools: Vec<String>,
}

impl McpRegistrationState {
    pub(super) async fn record(&self, server: &str, report: coco_tools::RegisterMcpToolsReport) {
        self.reports
            .write()
            .await
            .insert(server.to_string(), report);
    }

    pub(super) async fn clear(&self, server: &str) {
        self.reports.write().await.remove(server);
    }

    pub(super) async fn status_projection(
        &self,
        server: &str,
        advertised_tool_count: i32,
    ) -> McpRegistrationStatusProjection {
        let reports = self.reports.read().await;
        let Some(report) = reports.get(server) else {
            return McpRegistrationStatusProjection {
                tool_count: advertised_tool_count,
                skipped_tools: Vec::new(),
                tombstoned_tools: Vec::new(),
            };
        };
        McpRegistrationStatusProjection {
            tool_count: report.registered.len() as i32,
            skipped_tools: report
                .skipped
                .iter()
                .map(|skipped| coco_types::McpSkippedToolStatus {
                    tool_name: skipped.tool_name.clone(),
                    error: skipped.error.to_string(),
                })
                .collect(),
            tombstoned_tools: report
                .tombstones
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
        }
    }
}
