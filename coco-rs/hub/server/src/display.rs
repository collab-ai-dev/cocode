use coco_types::ToolName;

use crate::store::msg_type;

const MULTI_EDIT_TOOL: &str = "MultiEdit";

pub(crate) struct DisplaySource {
    pub(crate) text: String,
    pub(crate) language: String,
    mode: DisplayMode,
}

impl DisplaySource {
    pub(crate) fn from_block(
        msg_type: &str,
        tool_name: Option<&str>,
        block: Option<&serde_json::Value>,
        preview: Option<&str>,
    ) -> Self {
        if msg_type == msg_type::TOOL_USE
            && let Some(block) = block
            && let Some(input) = block.get("input")
        {
            return Self::from_tool_input(tool_name, input);
        }

        if let Some(block) = block {
            if matches!(msg_type, msg_type::REASONING | "user" | "assistant")
                && let Some(text) = text_from_content_block(block)
            {
                return Self::markdown(text);
            }

            if msg_type == msg_type::TOOL_RESULT
                && let Some(text) = tool_result_text(block)
            {
                return Self::from_tool_result(text);
            }
        }

        preview
            .map(|preview| Self::markdown(preview.to_string()))
            .unwrap_or_else(Self::empty)
    }

    pub(crate) fn mode_name(&self) -> &'static str {
        match self.mode {
            DisplayMode::Markdown => "markdown",
            DisplayMode::Code => "code",
        }
    }

    fn from_tool_input(tool_name: Option<&str>, input: &serde_json::Value) -> Self {
        match tool_name {
            Some(name)
                if name == ToolName::Bash.as_str() || name == ToolName::PowerShell.as_str() =>
            {
                input
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .map(|command| Self::code(command.to_string(), "bash"))
                    .unwrap_or_else(|| Self::json(input))
            }
            Some(name) if name == ToolName::Edit.as_str() => {
                edit_diff(input).unwrap_or_else(|| Self::json(input))
            }
            Some(name) if name == MULTI_EDIT_TOOL => {
                multi_edit_diff(input).unwrap_or_else(|| Self::json(input))
            }
            Some(name)
                if name == ToolName::Write.as_str() || name == ToolName::NotebookEdit.as_str() =>
            {
                let language = input
                    .get("file_path")
                    .or_else(|| input.get("path"))
                    .and_then(serde_json::Value::as_str)
                    .map(language_from_path)
                    .unwrap_or_else(|| "plaintext".to_string());
                input
                    .get("content")
                    .or_else(|| input.get("new_string"))
                    .and_then(serde_json::Value::as_str)
                    .map(|content| Self::code(content.to_string(), &language))
                    .unwrap_or_else(|| Self::json(input))
            }
            _ => Self::json(input),
        }
    }

    fn from_tool_result(text: String) -> Self {
        if looks_like_diff(&text) {
            Self::code(text, "diff")
        } else if looks_like_shell_output(&text) {
            Self::code(text, "bash")
        } else {
            Self::markdown(text)
        }
    }

    fn markdown(text: String) -> Self {
        Self {
            text,
            mode: DisplayMode::Markdown,
            language: "markdown".to_string(),
        }
    }

    fn code(text: String, language: &str) -> Self {
        Self {
            text,
            mode: DisplayMode::Code,
            language: normalize_prism_language(language),
        }
    }

    fn json(value: &serde_json::Value) -> Self {
        let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
        Self::code(text, "json")
    }

    fn empty() -> Self {
        Self {
            text: String::new(),
            mode: DisplayMode::Markdown,
            language: "markdown".to_string(),
        }
    }
}

enum DisplayMode {
    Markdown,
    Code,
}

fn text_from_content_block(block: &serde_json::Value) -> Option<String> {
    block
        .get("text")
        .or_else(|| block.get("thinking"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn tool_result_text(block: &serde_json::Value) -> Option<String> {
    if let Some(output) = block.get("output")
        && let Some(text) = output
            .get("value")
            .and_then(serde_json::Value::as_str)
            .or_else(|| output.as_str())
    {
        return Some(text.to_string());
    }

    let content = block.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let text = content
        .as_array()?
        .iter()
        .filter_map(|block| {
            block.as_str().map(str::to_string).or_else(|| {
                block
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

fn edit_diff(input: &serde_json::Value) -> Option<DisplaySource> {
    let old = input.get("old_string")?.as_str()?;
    let new = input.get("new_string")?.as_str()?;
    let path = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("edited-file");
    Some(DisplaySource::code(unified_diff(path, old, new), "diff"))
}

fn multi_edit_diff(input: &serde_json::Value) -> Option<DisplaySource> {
    let path = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("edited-file");
    let edits = input.get("edits")?.as_array()?;
    let mut diff = String::new();
    for edit in edits {
        let old = edit.get("old_string")?.as_str()?;
        let new = edit.get("new_string")?.as_str()?;
        if !diff.is_empty() {
            diff.push('\n');
        }
        diff.push_str(&unified_diff(path, old, new));
    }
    Some(DisplaySource::code(diff, "diff"))
}

fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let mut diff = format!("--- a/{path}\n+++ b/{path}\n@@\n");
    for line in old.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn looks_like_diff(text: &str) -> bool {
    text.lines()
        .any(|line| line.starts_with("@@") || line.starts_with("diff --git"))
        || text.lines().any(|line| line.starts_with("--- "))
            && text.lines().any(|line| line.starts_with("+++ "))
}

fn looks_like_shell_output(text: &str) -> bool {
    text.lines().take(8).any(|line| {
        line.starts_with("$ ")
            || line.starts_with("> ")
            || line.starts_with("cargo ")
            || line.starts_with("npm ")
            || line.starts_with("git ")
    })
}

fn language_from_path(path: &str) -> String {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    extension
        .map(normalize_prism_language)
        .unwrap_or_else(|| "plaintext".to_string())
}

fn normalize_prism_language(language: &str) -> String {
    match language.to_ascii_lowercase().as_str() {
        "sh" | "shell" | "zsh" | "powershell" | "ps1" => "bash",
        "patch" | "udiff" => "diff",
        "md" | "mdown" => "markdown",
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "yml" => "yaml",
        "jsonl" => "json",
        "go" | "golang" => "go",
        "text" | "txt" => "plaintext",
        "bash" | "diff" | "json" | "markdown" | "rust" | "typescript" | "javascript" | "python"
        | "toml" | "yaml" | "sql" => language,
        _ => "plaintext",
    }
    .to_string()
}
