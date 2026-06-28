//! Prompt context snapshots for the model-facing system prompt.
//!
//! The first slice keeps current prompt text stable while giving the
//! query layer a durable-shaped boundary: ordered sources plus a
//! deterministic epoch fingerprint.

use std::borrow::Cow;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;

const DEFAULT_MAIN_SYSTEM_PROMPT: &str =
    "You are coco, an AI coding assistant. Be concise and helpful.\n\n";
const MEMORY_TRUNCATED_MARKER: &str = "\n[Memory file truncated]\n";

/// Hard cap for one memory source rendered into the system prompt.
pub const MAX_PROMPT_CONTEXT_SOURCE_BYTES: usize = 4_000;

/// Hard aggregate cap for eager memory rendered into one prompt context.
pub const MAX_PROMPT_CONTEXT_MEMORY_BYTES: usize = 24_000;

/// Stable identifier for one rendered prompt-context snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContextEpoch(String);

impl ContextEpoch {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContextEpoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a caller wants the prompt context rendered.
#[derive(Debug, Clone)]
pub enum PromptContextMode<'a> {
    /// Use a caller-provided system prompt verbatim.
    Literal {
        source: PromptContextLiteralSource,
        text: Cow<'a, str>,
    },
    /// Render the default main-agent prompt with eager memory files.
    DefaultWorkspace { cwd: &'a Path },
}

impl<'a> PromptContextMode<'a> {
    pub fn literal(source: PromptContextLiteralSource, text: impl Into<Cow<'a, str>>) -> Self {
        Self::Literal {
            source,
            text: text.into(),
        }
    }

    pub fn default_workspace(cwd: &'a Path) -> Self {
        Self::DefaultWorkspace { cwd }
    }
}

/// Provenance for verbatim prompts selected outside `coco-context`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptContextLiteralSource {
    ConfigOverride,
    Coordinator,
}

impl PromptContextLiteralSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::ConfigOverride => "config_override",
            Self::Coordinator => "coordinator",
        }
    }
}

/// One source that contributed to a prompt context snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptContextSource {
    pub kind: PromptContextSourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub original_size_bytes: i64,
    pub rendered_size_bytes: i64,
    pub truncated: bool,
    pub fingerprint: String,
}

/// Source taxonomy for prompt context assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptContextSourceKind {
    Literal,
    DefaultIdentity,
    MemoryFile,
}

impl PromptContextSourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::DefaultIdentity => "default_identity",
            Self::MemoryFile => "memory_file",
        }
    }
}

/// Rendered prompt context plus source/epoch metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptContext {
    pub epoch: ContextEpoch,
    pub prompt: crate::SystemPrompt,
    pub rendered_system_prompt: String,
    pub sources: Vec<PromptContextSource>,
}

impl PromptContext {
    pub fn build(mode: PromptContextMode<'_>) -> Self {
        match mode {
            PromptContextMode::Literal { source, text } => {
                let system_prompt = text.into_owned();
                let source_fingerprint = fingerprint_source(|hasher| {
                    hasher.update(b"literal");
                    hasher.update([0]);
                    hasher.update(source.as_str().as_bytes());
                    hasher.update([0]);
                    hasher.update(system_prompt.as_bytes());
                });
                let sources = vec![PromptContextSource {
                    kind: PromptContextSourceKind::Literal,
                    path: None,
                    original_size_bytes: system_prompt.len() as i64,
                    rendered_size_bytes: system_prompt.len() as i64,
                    truncated: false,
                    fingerprint: source_fingerprint,
                }];
                let mut prompt = crate::SystemPrompt::new();
                prompt.add_text(system_prompt);
                Self::from_parts(prompt, sources)
            }
            PromptContextMode::DefaultWorkspace { cwd } => {
                let memory_files = crate::discover_memory_files(cwd);
                let mut prompt = crate::SystemPrompt::new();
                prompt.add_text(DEFAULT_MAIN_SYSTEM_PROMPT);
                let mut sources = vec![PromptContextSource {
                    kind: PromptContextSourceKind::DefaultIdentity,
                    path: None,
                    original_size_bytes: DEFAULT_MAIN_SYSTEM_PROMPT.len() as i64,
                    rendered_size_bytes: DEFAULT_MAIN_SYSTEM_PROMPT.len() as i64,
                    truncated: false,
                    fingerprint: fingerprint_source(|hasher| {
                        hasher.update(b"default_identity");
                        hasher.update([0]);
                        hasher.update(DEFAULT_MAIN_SYSTEM_PROMPT.as_bytes());
                    }),
                }];

                let mut memory_section = String::new();
                let mut remaining_memory_bytes = MAX_PROMPT_CONTEXT_MEMORY_BYTES;
                for file in &memory_files {
                    let (rendered, truncated) =
                        render_bounded_memory_file(file, remaining_memory_bytes);
                    remaining_memory_bytes = remaining_memory_bytes.saturating_sub(rendered.len());
                    sources.push(PromptContextSource {
                        kind: PromptContextSourceKind::MemoryFile,
                        path: Some(file.path.clone()),
                        original_size_bytes: file.content.len() as i64,
                        rendered_size_bytes: rendered.len() as i64,
                        truncated,
                        fingerprint: fingerprint_source(|hasher| {
                            hasher.update(b"memory_file");
                            hasher.update([0]);
                            hasher.update(file.path.as_os_str().as_encoded_bytes());
                            hasher.update([0]);
                            hasher.update(memory_source_name(file.source).as_bytes());
                            hasher.update([0]);
                            hasher.update(file.content.as_bytes());
                        }),
                    });
                    memory_section.push_str(&rendered);
                }

                if !memory_section.is_empty() {
                    prompt.add_text(memory_section);
                }

                Self::from_parts(prompt, sources)
            }
        }
    }

    pub fn system_prompt(&self) -> &str {
        &self.rendered_system_prompt
    }

    fn from_parts(prompt: crate::SystemPrompt, sources: Vec<PromptContextSource>) -> Self {
        let rendered_system_prompt = prompt.full_text();
        let mut hasher = sha2::Sha256::new();
        hasher.update(rendered_system_prompt.as_bytes());
        for source in &sources {
            hasher.update([0]);
            hasher.update(source.kind.as_str().as_bytes());
            hasher.update([0]);
            if let Some(path) = &source.path {
                hasher.update(path.as_os_str().as_encoded_bytes());
            }
            hasher.update([0]);
            hasher.update(source.fingerprint.as_bytes());
        }
        let digest = hasher.finalize();
        Self {
            epoch: ContextEpoch(format!("{digest:x}")),
            prompt,
            rendered_system_prompt,
            sources,
        }
    }
}

fn render_bounded_memory_file(
    file: &crate::MemoryFile,
    remaining_memory_bytes: usize,
) -> (String, bool) {
    if remaining_memory_bytes == 0 {
        return (String::new(), true);
    }

    let header = format!("# {}\n", file.path.display());
    let footer = "\n\n";
    let max_source_bytes = MAX_PROMPT_CONTEXT_SOURCE_BYTES.min(remaining_memory_bytes);
    let fixed_bytes = header.len() + footer.len();
    if fixed_bytes >= max_source_bytes {
        return (String::new(), true);
    }

    let available_content_bytes = max_source_bytes - fixed_bytes;
    let will_truncate = file.content.len() > available_content_bytes
        || file.content.len() + fixed_bytes > remaining_memory_bytes;
    let marker_bytes = if will_truncate {
        MEMORY_TRUNCATED_MARKER.len()
    } else {
        0
    };
    if marker_bytes >= available_content_bytes {
        return (String::new(), true);
    }
    let content_limit = if will_truncate {
        available_content_bytes - marker_bytes
    } else {
        available_content_bytes
    };
    let content = take_utf8_prefix(&file.content, content_limit);

    let mut rendered = String::with_capacity(header.len() + content.len() + marker_bytes + 2);
    rendered.push_str(&header);
    rendered.push_str(content);
    if will_truncate {
        rendered.push_str(MEMORY_TRUNCATED_MARKER);
    }
    rendered.push_str(footer);
    (rendered, will_truncate)
}

fn take_utf8_prefix(content: &str, max_bytes: usize) -> &str {
    if content.len() <= max_bytes {
        return content;
    }
    let cut = content.floor_char_boundary(max_bytes);
    &content[..cut]
}

fn memory_source_name(source: crate::MemoryFileSource) -> &'static str {
    match source {
        crate::MemoryFileSource::Managed => "managed",
        crate::MemoryFileSource::UserGlobal => "user_global",
        crate::MemoryFileSource::ProjectConfig => "project_config",
        crate::MemoryFileSource::Project => "project",
        crate::MemoryFileSource::Local => "local",
    }
}

fn fingerprint_source(update: impl FnOnce(&mut sha2::Sha256)) -> String {
    let mut hasher = sha2::Sha256::new();
    update(&mut hasher);
    let digest = hasher.finalize();
    format!("{digest:x}")
}

#[cfg(test)]
#[path = "prompt_context.test.rs"]
mod tests;
