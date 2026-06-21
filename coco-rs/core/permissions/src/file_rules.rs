//! File permission rule matching.
//!
//! File rules are matched against paths relative to a source-specific root,
//! with `//` meaning filesystem root, `~/` meaning home, `/` meaning the
//! settings/source root, and `./` normalized away.

use std::collections::HashMap;
use std::path::PathBuf;

use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRulesBySource;
use coco_types::ToolName;
use globset::GlobBuilder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRuleToolType {
    Read,
    Edit,
}

#[derive(Debug, Clone)]
pub struct FileRuleMatchContext {
    cwd: PathBuf,
    source_roots: HashMap<PermissionRuleSource, PathBuf>,
    home: Option<PathBuf>,
}

impl FileRuleMatchContext {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            source_roots: HashMap::new(),
            home: home_dir(),
        }
    }

    pub fn with_source_root(
        mut self,
        source: PermissionRuleSource,
        root: impl Into<PathBuf>,
    ) -> Self {
        self.source_roots.insert(source, root.into());
        self
    }

    pub fn with_source_roots(mut self, roots: &HashMap<PermissionRuleSource, PathBuf>) -> Self {
        self.source_roots
            .extend(roots.iter().map(|(source, root)| (*source, root.clone())));
        self
    }

    fn root_for_source(&self, source: PermissionRuleSource) -> PathBuf {
        if let Some(root) = self.source_roots.get(&source) {
            return root.clone();
        }
        match source {
            PermissionRuleSource::Session
            | PermissionRuleSource::Command
            | PermissionRuleSource::CliArg
            | PermissionRuleSource::ProjectSettings
            | PermissionRuleSource::LocalSettings
            | PermissionRuleSource::PolicySettings => self.cwd.clone(),
            PermissionRuleSource::FlagSettings => self.cwd.clone(),
            PermissionRuleSource::UserSettings => coco_config::global_config::config_home(),
        }
    }
}

pub const FILE_RULE_PRIORITY_ORDER: &[PermissionRuleSource] = &[
    PermissionRuleSource::Session,
    PermissionRuleSource::Command,
    PermissionRuleSource::CliArg,
    PermissionRuleSource::FlagSettings,
    PermissionRuleSource::LocalSettings,
    PermissionRuleSource::ProjectSettings,
    PermissionRuleSource::UserSettings,
    PermissionRuleSource::PolicySettings,
];

pub fn matching_file_rule<'a>(
    rules: &'a PermissionRulesBySource,
    paths_to_check: &[String],
    tool_type: FileRuleToolType,
    match_context: &FileRuleMatchContext,
) -> Option<&'a PermissionRule> {
    for source in FILE_RULE_PRIORITY_ORDER {
        let Some(source_rules) = rules.get(source) else {
            continue;
        };
        for rule in source_rules {
            if file_rule_matches_paths(rule, paths_to_check, tool_type, match_context) {
                return Some(rule);
            }
        }
    }
    None
}

pub fn file_rule_matches_paths(
    rule: &PermissionRule,
    paths_to_check: &[String],
    tool_type: FileRuleToolType,
    match_context: &FileRuleMatchContext,
) -> bool {
    if !rule_matches_file_tool(rule.value.tool_pattern.as_str(), tool_type) {
        return false;
    }
    let Some(content) = rule.value.rule_content.as_deref() else {
        return true;
    };
    paths_to_check
        .iter()
        .any(|path| file_rule_content_matches(content, rule.source, path, match_context))
}

fn rule_matches_file_tool(tool_pattern: &str, tool_type: FileRuleToolType) -> bool {
    match tool_type {
        FileRuleToolType::Read => tool_pattern == ToolName::Read.as_str(),
        FileRuleToolType::Edit => tool_pattern == ToolName::Edit.as_str(),
    }
}

fn file_rule_content_matches(
    rule_content: &str,
    source: PermissionRuleSource,
    file_path: &str,
    match_context: &FileRuleMatchContext,
) -> bool {
    let pattern = pattern_with_root(rule_content, source, match_context);
    let root = pattern.root.unwrap_or_else(|| match_context.cwd.clone());
    let Some(relative_path) =
        coco_paths::relative_posix_path(&root, std::path::Path::new(file_path))
    else {
        return false;
    };
    if relative_path.is_empty() {
        return false;
    }
    ignore_pattern_matches(&pattern.relative_pattern, &relative_path)
}

#[derive(Debug, Clone)]
struct RootedPattern {
    relative_pattern: String,
    root: Option<PathBuf>,
}

fn pattern_with_root(
    pattern: &str,
    source: PermissionRuleSource,
    match_context: &FileRuleMatchContext,
) -> RootedPattern {
    let normalized = pattern.replace('\\', "/");
    if let Some(rest) = normalized.strip_prefix("//") {
        return RootedPattern {
            relative_pattern: format!("/{rest}"),
            root: Some(PathBuf::from("/")),
        };
    }
    if let Some(rest) = normalized.strip_prefix("~/") {
        return RootedPattern {
            relative_pattern: format!("/{rest}"),
            root: match_context.home.clone(),
        };
    }
    if normalized.starts_with('/') {
        return RootedPattern {
            relative_pattern: normalized,
            root: Some(match_context.root_for_source(source)),
        };
    }
    RootedPattern {
        relative_pattern: normalized
            .strip_prefix("./")
            .unwrap_or(&normalized)
            .to_string(),
        root: None,
    }
}

fn ignore_pattern_matches(pattern: &str, relative_path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let anchored = pattern.starts_with('/');
    let pattern = pattern.trim_start_matches('/');
    if pattern.is_empty() {
        return false;
    }
    if let Some(dir) = pattern.strip_suffix("/**") {
        return directory_pattern_matches(dir, relative_path, anchored);
    }
    glob_pattern_matches(pattern, relative_path, anchored)
}

fn directory_pattern_matches(dir: &str, relative_path: &str, anchored: bool) -> bool {
    if anchored || dir.contains('/') {
        return relative_path == dir || relative_path.starts_with(&format!("{dir}/"));
    }
    relative_path
        .split('/')
        .enumerate()
        .any(|(index, component)| {
            component == dir
                && (relative_path == dir
                    || relative_path.starts_with(&format!("{dir}/"))
                    || relative_path.contains(&format!("/{dir}/"))
                    || relative_path.ends_with(&format!("/{dir}"))
                    || index > 0)
        })
}

fn glob_pattern_matches(pattern: &str, relative_path: &str, anchored: bool) -> bool {
    if glob_matches(pattern, relative_path) {
        return true;
    }
    if !anchored && !pattern.contains('/') {
        return glob_matches(&format!("**/{pattern}"), relative_path);
    }
    false
}

fn glob_matches(pattern: &str, relative_path: &str) -> bool {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher().is_match(relative_path))
        .unwrap_or(false)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
#[path = "file_rules.test.rs"]
mod tests;
