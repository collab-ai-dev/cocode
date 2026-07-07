use std::path::{Path, PathBuf};

use crate::ProjectPaths;
use crate::projects_root;

/// Runtime-wide path facade. It separates config-home paths from the
/// memory-base layout used for projects, transcripts, and auto-memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    config_home: PathBuf,
    memory_base: PathBuf,
}

impl RuntimePaths {
    pub fn new(config_home: PathBuf, memory_base_override: Option<PathBuf>) -> Self {
        let memory_base = memory_base_override.unwrap_or_else(|| config_home.clone());
        Self {
            config_home,
            memory_base,
        }
    }

    pub fn config_home(&self) -> &Path {
        &self.config_home
    }

    pub fn memory_base(&self) -> &Path {
        &self.memory_base
    }

    pub fn projects_root(&self) -> PathBuf {
        projects_root(&self.memory_base)
    }

    pub fn project_paths(&self, project_root: &Path) -> ProjectPaths {
        ProjectPaths::new(self.memory_base.clone(), project_root)
    }

    pub fn user_memory_dir(&self) -> PathBuf {
        self.memory_base.join("memory")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.config_home.join("sessions")
    }

    pub fn pids_dir(&self) -> PathBuf {
        self.sessions_dir().join("pids")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.config_home.join("logs")
    }

    pub fn plugins_dir(&self) -> PathBuf {
        self.config_home.join("plugins")
    }

    pub fn output_styles_dir(&self) -> PathBuf {
        self.config_home.join("output-styles")
    }

    pub fn models_file(&self) -> PathBuf {
        self.config_home.join("models.json")
    }

    pub fn file_history_dir(&self) -> PathBuf {
        self.config_home.join("file-history")
    }
}

#[cfg(test)]
#[path = "runtime_paths.test.rs"]
mod tests;
