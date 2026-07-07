use coco_tool_runtime::ToolError;

/// Narrow wrapper for blocking filesystem searches launched from async tools.
pub(super) struct BlockingFsTask<T> {
    label: &'static str,
    join: tokio::task::JoinHandle<T>,
}

impl<T> BlockingFsTask<T>
where
    T: Send + 'static,
{
    pub(super) fn spawn<F>(label: &'static str, task: F) -> Self
    where
        F: FnOnce() -> T + Send + 'static,
    {
        Self {
            label,
            join: tokio::task::spawn_blocking(task),
        }
    }

    pub(super) async fn join(self) -> Result<T, ToolError> {
        self.join.await.map_err(|error| ToolError::ExecutionFailed {
            message: format!("{} task failed: {error}", self.label),
            display_data: None,
            source: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::BlockingFsTask;

    #[tokio::test]
    async fn blocking_fs_task_maps_join_failure() {
        let task = BlockingFsTask::spawn("test search", || -> Result<(), String> {
            panic!("boom");
        });

        let err = task.join().await.expect_err("panic maps to tool error");
        let message = err.to_string();
        assert!(
            message.contains("test search task failed"),
            "got: {message}"
        );
    }
}
