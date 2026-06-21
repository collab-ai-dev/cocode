/// Provider-specific wire behavior for OpenAI-compatible endpoints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OpenAICompatibleProviderProfile {
    #[default]
    Generic,
    DeepSeek,
}

impl OpenAICompatibleProviderProfile {
    pub(crate) fn default_include_usage(self) -> bool {
        matches!(self, Self::DeepSeek)
    }

    pub(crate) fn is_deepseek(self) -> bool {
        matches!(self, Self::DeepSeek)
    }
}
