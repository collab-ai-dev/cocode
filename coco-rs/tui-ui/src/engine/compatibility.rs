//! Terminal compatibility decisions for native scrollback.

use std::sync::OnceLock;
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerminalCompatibility {
    #[default]
    NativeScrollback,
    ZellijNativeScrollbackDisabled,
}

impl TerminalCompatibility {
    pub fn detect() -> Self {
        Self::detect_with(|name| {
            std::env::var_os(name).and_then(|value| {
                let text = value.to_string_lossy();
                (!text.is_empty()).then(|| text.into_owned())
            })
        })
    }

    pub fn detect_with<F>(get_env: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        if ["ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_VERSION"]
            .into_iter()
            .any(|name| get_env(name).is_some_and(|value| !value.is_empty()))
        {
            Self::ZellijNativeScrollbackDisabled
        } else {
            Self::NativeScrollback
        }
    }

    pub fn native_scrollback_enabled(self) -> bool {
        matches!(self, Self::NativeScrollback)
    }

    pub fn status_message(self) -> Option<&'static str> {
        match self {
            Self::NativeScrollback => None,
            Self::ZellijNativeScrollbackDisabled => Some("native scrollback disabled in Zellij"),
        }
    }
}

/// Whether the terminal supports synchronized output (DECSET mode 2026), per the
/// startup DECRQM probe ([`set_synchronized_update_supported`]).
///
/// Defaults to `true` when no probe ran (non-tty / SDK / no reply): the surface
/// emits BSU/ESU unconditionally and assumes support until proven otherwise, so
/// the non-flicker fallback only engages for terminals positively known to lack
/// mode 2026. A free function rather than a [`TerminalCompatibility`] method:
/// synchronized-update support is a process-global capability (the DECRQM probe
/// result in a cache), orthogonal to the per-instance native-scrollback choice.
pub fn synchronized_update_supported() -> bool {
    synchronized_update_probed().unwrap_or(true)
}

fn synchronized_update_cache() -> &'static RwLock<Option<bool>> {
    static CACHE: OnceLock<RwLock<Option<bool>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(None))
}

/// Record the DECRQM probe result for synchronized output (DECSET mode 2026).
/// `coco-tui` calls this once at startup after parsing the reply; the value is
/// read back by [`TerminalCompatibility::synchronized_update_supported`].
pub fn set_synchronized_update_supported(supported: bool) {
    if let Ok(mut guard) = synchronized_update_cache().write() {
        *guard = Some(supported);
    }
}

/// The probed synchronized-output support, or `None` until a probe records one.
pub fn synchronized_update_probed() -> Option<bool> {
    synchronized_update_cache()
        .read()
        .ok()
        .and_then(|guard| *guard)
}

#[cfg(test)]
#[path = "compatibility.test.rs"]
mod tests;
