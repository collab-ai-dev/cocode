use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

/// Lexically normalize a path without touching the filesystem.
///
/// This intentionally does not resolve symlinks. It is appropriate for
/// permission and config-path checks where the caller separately decides
/// whether a realpath lookup is needed.
pub fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                out.push(component.as_os_str());
            }
        }
    }
    out
}

/// Return `path` relative to `root` using `/` separators.
///
/// Returns `None` when `path` is outside `root`.
pub fn relative_posix_path(root: &Path, path: &Path) -> Option<String> {
    let root = normalize_lexical(root);
    let path = normalize_lexical(path);
    if path == root {
        return Some(String::new());
    }
    let root_str = path_to_posix(&root);
    let path_str = path_to_posix(&path);
    let root_with_sep = if root_str.ends_with('/') {
        root_str
    } else {
        format!("{root_str}/")
    };
    path_str.strip_prefix(&root_with_sep).map(str::to_string)
}

/// Convert a platform path to a POSIX-style string.
pub fn path_to_posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
#[path = "relative.test.rs"]
mod tests;
