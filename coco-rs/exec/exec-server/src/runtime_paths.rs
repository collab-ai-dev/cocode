use std::path::PathBuf;

use coco_utils_absolute_path::AbsolutePathBuf;

/// Runtime paths needed by exec-server child processes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecServerRuntimePaths {
    /// Stable path to the Coco executable used to launch hidden helper modes.
    pub coco_self_exe: AbsolutePathBuf,
    /// Path to the Linux sandbox helper alias used when the platform sandbox
    /// needs to re-enter Coco by argv0.
    pub coco_linux_sandbox_exe: Option<AbsolutePathBuf>,
}

impl ExecServerRuntimePaths {
    pub fn from_optional_paths(
        coco_self_exe: Option<PathBuf>,
        coco_linux_sandbox_exe: Option<PathBuf>,
    ) -> std::io::Result<Self> {
        let coco_self_exe = coco_self_exe.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Coco executable path is not configured",
            )
        })?;
        Self::new(coco_self_exe, coco_linux_sandbox_exe)
    }

    pub fn new(
        coco_self_exe: PathBuf,
        coco_linux_sandbox_exe: Option<PathBuf>,
    ) -> std::io::Result<Self> {
        Ok(Self {
            coco_self_exe: absolute_path(coco_self_exe)?,
            coco_linux_sandbox_exe: coco_linux_sandbox_exe.map(absolute_path).transpose()?,
        })
    }
}

fn absolute_path(path: PathBuf) -> std::io::Result<AbsolutePathBuf> {
    AbsolutePathBuf::from_absolute_path(path.as_path())
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))
}
