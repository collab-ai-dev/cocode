// Adapted from path-absolutize 3.1.1:
// Copyright (c) 2018 magiclen.org (Ron Li)
// Licensed under the MIT License.
//
// Keep this implementation local so explicit-base normalization can be
// infallible for absolute inputs and explicit-base normalization.

#[cfg(windows)]
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn absolutize_from(path: &Path, base_path: &Path) -> PathBuf {
    // `.`/`..` collapsing is shared with the containment fence primitive;
    // only the empty-result contract differs (absolutize yields `"."`).
    let normalized = crate::containment::lexical_normalize(&path_with_base(path, base_path));
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

#[cfg(not(windows))]
fn path_with_base(path: &Path, base_path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_path.join(path)
    }
}

#[cfg(windows)]
fn path_with_base(path: &Path, base_path: &Path) -> PathBuf {
    if path.is_absolute() || path.has_root() {
        return base_path.join(path);
    }

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return base_path.join(path);
    };

    let mut path = PathBuf::new();
    path.push(prefix.as_os_str());

    if components.clone().next().is_none() {
        path.push(std::path::MAIN_SEPARATOR_STR);
        return path;
    }

    let skip_base_prefix = matches!(base_path.components().next(), Some(Component::Prefix(_)));
    for component in base_path
        .components()
        .skip(usize::from(skip_base_prefix))
        .chain(components)
    {
        path.push(component.as_os_str());
    }
    path
}

#[cfg(test)]
#[path = "absolutize.test.rs"]
mod tests;
