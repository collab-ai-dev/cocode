//! Terminal compatibility decisions for native scrollback.

use std::sync::OnceLock;
use std::sync::RwLock;

use crate::terminal_detect::Multiplexer;
use crate::terminal_detect::TerminalName;
use crate::terminal_detect::terminal_info_with;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerminalCompatibility {
    #[default]
    NativeScrollback,
    ZellijNativeScrollbackDisabled,
}

/// Read a non-empty environment variable. The injectable seam every detection
/// below is written against, so they are testable without mutating process env.
fn env_lookup(name: &str) -> Option<String> {
    std::env::var_os(name).and_then(|value| {
        let text = value.to_string_lossy();
        (!text.is_empty()).then(|| text.into_owned())
    })
}

/// Whether another writer may repaint coco's pane out of band — a multiplexer
/// (tmux, screen, Zellij) or an embedded-editor terminal.
///
/// Such a writer can paint over the retained viewport while coco is unfocused.
/// The cell diff's previous buffer still believes those cells are intact, so the
/// stranded content survives until some unrelated invalidation; the shell heals
/// it by forcing one full repaint on focus-gain.
///
/// Env-sniffing is deliberately the whole implementation for now — it folds into
/// the terminal capability model (plan item G3) when that lands.
pub fn repaints_pane_out_of_band() -> bool {
    repaints_pane_out_of_band_with(env_lookup)
}

pub fn repaints_pane_out_of_band_with<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    terminal_info_with(get_env).in_multiplexer()
}

/// Whether finalized history may safely emit OSC 8 hyperlinks.
///
/// OSC 8 has no useful feature probe, so this deliberately recognizes only
/// terminals with established support. Multiplexers are rejected unless tmux
/// itself reports a passthrough-capable version; emitting an unknown OSC into
/// screen/Zellij is worse than leaving an ordinary, copyable URL visible.
pub fn osc8_hyperlinks_supported() -> bool {
    osc8_hyperlinks_supported_with(env_lookup)
}

pub fn osc8_hyperlinks_supported_with<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    let info = terminal_info_with(&get_env);
    if info.is_inside(Multiplexer::Screen) || info.is_inside(Multiplexer::Zellij) {
        return false;
    }

    if info.is_inside(Multiplexer::Tmux) {
        return get_env("TERM_PROGRAM").is_some_and(|program| program.eq_ignore_ascii_case("tmux"))
            && get_env("TERM_PROGRAM_VERSION")
                .as_deref()
                .is_some_and(|version| version_at_least(version, 3, 4));
    }

    match info.name {
        TerminalName::Iterm2
        | TerminalName::WezTerm
        | TerminalName::Kitty
        | TerminalName::Ghostty => true,
        // Every other terminal must prove VTE ≥ 0.50, which is where OSC 8
        // landed. Unknown terminals stay off: an ordinary copyable URL beats
        // an unrecognized escape printed into the transcript.
        TerminalName::Alacritty
        | TerminalName::AppleTerminal
        | TerminalName::Dumb
        | TerminalName::GnomeTerminal
        | TerminalName::Hyper
        | TerminalName::Konsole
        | TerminalName::Unknown
        | TerminalName::VsCode
        | TerminalName::Vte
        | TerminalName::Warp
        | TerminalName::WindowsTerminal => get_env("VTE_VERSION")
            .and_then(|version| version.parse::<u32>().ok())
            .is_some_and(|version| version >= 5_000),
    }
}

fn version_at_least(version: &str, required_major: u32, required_minor: u32) -> bool {
    let mut components = version.split('.');
    let Some(major) = components
        .next()
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return false;
    };
    let minor = components
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_default();
    (major, minor) >= (required_major, required_minor)
}

impl TerminalCompatibility {
    pub fn detect() -> Self {
        Self::detect_with(env_lookup)
    }

    pub fn detect_with<F>(get_env: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        // Zellij anywhere in the chain governs: an inner tmux cannot restore
        // the scrollback semantics the outer Zellij pane took away.
        if terminal_info_with(get_env).is_inside(Multiplexer::Zellij) {
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

fn keyboard_enhancement_cache() -> &'static RwLock<Option<bool>> {
    static CACHE: OnceLock<RwLock<Option<bool>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(None))
}

/// Record whether the terminal answered the kitty keyboard-protocol query
/// (`CSI ? u`). `coco-tui` calls this once at startup from the merged probe.
pub fn set_keyboard_enhancement_supported(supported: bool) {
    if let Ok(mut guard) = keyboard_enhancement_cache().write() {
        *guard = Some(supported);
    }
}

/// The probed keyboard-enhancement support, or `None` until a probe records one.
///
/// Deliberately not collapsed to a `bool` with a default: the two "we don't
/// know" cases pull in opposite directions. Pushing the flags is harmless on a
/// terminal that ignores them, so an unprobed terminal still gets the push; but
/// telling the user "Shift+Enter inserts a newline" when it does not is a lie,
/// so hint text needs to distinguish unknown from confirmed.
pub fn keyboard_enhancement_probed() -> Option<bool> {
    keyboard_enhancement_cache()
        .read()
        .ok()
        .and_then(|guard| *guard)
}

#[cfg(test)]
#[path = "compatibility.test.rs"]
mod tests;
