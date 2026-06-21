//! TUI-side wrapper around `coco_keybindings::ChordResolver`.
//!
//! Owns the chord-state machine for the lifetime of the app and
//! exposes a [`KeybindingHandle`] consumed by `keybinding_bridge` and
//! the render layer. Hot-reload-aware: when
//! `coco_keybindings::KeybindingsWatcher` fires a new
//! `KeybindingsLoadResult`, the handle rebuilds its resolver
//! atomically.
//!
//! Also exposes:
//!
//! * [`KeybindingHandle::tick`] — drive the 1 s chord-timeout from
//!   the TUI animation/poll loop.
//! * [`KeybindingHandle::pending_display`] — `"ctrl+x …"` hint for the
//!   status bar.
//! * [`KeybindingHandle::display_for`] — render a shortcut hint for an
//!   action so footer/help text reflects user customizations.
//! * [`KeybindingHandle::warnings`] — typed validation issues from the
//!   most recent load.

use std::sync::Arc;
use std::sync::RwLock;
use std::time::Instant;

use coco_keybindings::ChordResolver;
use coco_keybindings::DisplayPlatform;
use coco_keybindings::Keybinding;
use coco_keybindings::KeybindingAction;
use coco_keybindings::KeybindingContext as KbContext;
use coco_keybindings::KeybindingsLoadResult;
use coco_keybindings::KeybindingsWatcher;
use coco_keybindings::ResolveOutcome;
use coco_keybindings::ValidationIssue;
use coco_keybindings::defaults::default_blocks;
use coco_keybindings::from_crossterm;
use crossterm::event::KeyEvent;
use tokio::sync::broadcast::error::RecvError;

use crate::keybinding_bridge::KeybindingContext as TuiContext;

/// TUI-shared, clone-able handle wrapping the resolver + last
/// validation result. Cheap to clone (`Arc` internally).
#[derive(Debug, Clone)]
pub struct KeybindingHandle {
    inner: Arc<RwLock<HandleInner>>,
    platform: DisplayPlatform,
}

#[derive(Debug)]
struct HandleInner {
    resolver: ChordResolver,
    warnings: Vec<ValidationIssue>,
}

impl KeybindingHandle {
    /// Build a handle from defaults only. Cheap; suitable for tests
    /// and for the early-startup window before
    /// [`KeybindingHandle::with_watcher`] runs.
    pub fn from_defaults() -> Self {
        let bindings = parse_default_blocks();
        Self::from_parts(ChordResolver::new(&bindings), Vec::new())
    }

    fn from_parts(resolver: ChordResolver, warnings: Vec<ValidationIssue>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HandleInner { resolver, warnings })),
            platform: DisplayPlatform::current(),
        }
    }

    /// Build a handle from an initial [`KeybindingsLoadResult`] and a
    /// running [`KeybindingsWatcher`]. Spawns a tokio task that
    /// rebuilds the resolver on each hot-reload event.
    ///
    /// The handle is returned immediately; updates happen in the
    /// background.
    pub fn with_watcher(initial: KeybindingsLoadResult, watcher: &KeybindingsWatcher) -> Self {
        let handle = Self::from_parts(ChordResolver::new(&initial.bindings), initial.warnings);

        let inner = handle.inner.clone();
        let mut rx = watcher.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(result) => {
                        let resolver = ChordResolver::new(&result.bindings);
                        if let Ok(mut guard) = inner.write() {
                            guard.resolver = resolver;
                            guard.warnings = result.warnings;
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });

        handle
    }

    /// Feed a key event through the resolver.
    pub fn resolve_key(&self, event: KeyEvent, tui_ctx: TuiContext) -> ResolverResult {
        let Some(combo) = from_crossterm(event) else {
            return ResolverResult::NotResolved;
        };
        let stack = context_stack(tui_ctx);
        let mut guard = match self.inner.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // Time out a stale pending chord before feeding the new combo.
        let _ = guard.resolver.tick(Instant::now());
        match guard.resolver.feed(&combo, &stack) {
            ResolveOutcome::Fire(action) => ResolverResult::Action(action),
            ResolveOutcome::Pending => ResolverResult::Pending,
            ResolveOutcome::Unbound | ResolveOutcome::ChordCancelled => ResolverResult::Consumed,
            ResolveOutcome::NoMatch => ResolverResult::NotResolved,
        }
    }

    /// Drive the chord-timeout from the TUI's animation tick. Returns
    /// `true` if a pending chord was just cancelled — the caller
    /// should redraw to clear the status indicator.
    ///
    /// Cheap when no chord is pending: a read-lock fast path avoids
    /// taking the write lock 4×/sec under the TUI tick interval. Only
    /// upgrades to a write lock when there's actually a pending chord
    /// to evaluate.
    pub fn tick(&self, now: Instant) -> bool {
        // Fast path — no chord pending, nothing to time out.
        let needs_check = match self.inner.read() {
            Ok(g) => g.resolver.has_pending(),
            Err(p) => p.into_inner().resolver.has_pending(),
        };
        if !needs_check {
            return false;
        }
        let mut guard = match self.inner.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.resolver.tick(now).is_some()
    }

    /// Pending-chord status-bar hint (e.g. `"ctrl+x …"`). `None` when
    /// no chord is in flight.
    pub fn pending_display(&self) -> Option<String> {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.resolver.pending_display(self.platform)
    }

    /// Whether a partial chord is in flight. Cheaper than
    /// [`Self::pending_display`] when you only need the boolean.
    pub fn has_pending_chord(&self) -> bool {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.resolver.has_pending()
    }

    /// Render a shortcut hint for `action` in the given TUI context.
    /// Used by footer/help/status text so user customizations show
    /// through.
    pub fn display_for(&self, action: &KeybindingAction, tui_ctx: TuiContext) -> Option<String> {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard
            .resolver
            .display_for(action, &context_stack(tui_ctx), self.platform)
    }

    /// Most-recent validation warnings (empty when defaults-only or
    /// the user file is clean).
    pub fn warnings(&self) -> Vec<ValidationIssue> {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.warnings.clone()
    }
}

fn parse_default_blocks() -> Vec<Keybinding> {
    default_blocks()
        .iter()
        .flat_map(|block| {
            block
                .bindings
                .iter()
                .filter_map(|(chord_str, action)| {
                    coco_keybindings::parse_chord(chord_str)
                        .ok()
                        .map(|chord| Keybinding {
                            chord,
                            action: action.clone(),
                            context: block.context,
                        })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Map the coarse TUI [`crate::keybinding_bridge::KeybindingContext`]
/// to the ordered context stack the resolver expects.
///
/// Most-specific first, with `Global` always last so global shortcuts
/// remain reachable from every context.
pub fn context_stack(ctx: TuiContext) -> Vec<KbContext> {
    use TuiContext::*;
    match ctx {
        Confirmation => vec![
            KbContext::Confirmation,
            KbContext::Select,
            KbContext::Global,
        ],
        // Deliberately NOT KbContext::Confirmation: a question must never be
        // answerable by the Y/N/A approve/deny bindings. Generic Select nav
        // (up/down/enter) plus Global stay resolvable; everything else falls
        // to the legacy `map_question_key` cascade.
        Question => vec![KbContext::Select, KbContext::Global],
        Picker => vec![
            KbContext::MessageSelector,
            KbContext::Select,
            KbContext::Global,
        ],
        ModelPicker => vec![KbContext::ModelPicker, KbContext::Select, KbContext::Global],
        // The roster's keys come from `modal_pane::team_roster::map_key`; the
        // Select+Global stack just keeps generic nav/cancel resolvable.
        TeamRoster => vec![KbContext::Select, KbContext::Global],
        Scrollable => vec![KbContext::Help, KbContext::Transcript, KbContext::Global],
        Transcript => vec![KbContext::Transcript, KbContext::Global],
        // Do not include Chat here: Chat binds Enter to submit, but TS
        // `useTypeahead` handles Return before ordinary input submission
        // while suggestions are visible.
        Autocomplete => vec![KbContext::Autocomplete, KbContext::Task, KbContext::Global],
        ThemePicker => vec![
            KbContext::ThemePicker,
            KbContext::Settings,
            KbContext::Select,
            KbContext::Global,
        ],
        Settings => vec![KbContext::Settings, KbContext::Select, KbContext::Global],
        // Global-only: the editor's nav + text input come entirely from
        // `modal_pane::permissions_editor::map_key`. Deliberately NO
        // Select/Confirmation — those would resolve arrows to `Surface*`
        // (and chars to filter / Y-N-A) before the editor's `intercept`
        // sees them as the `Cursor*` / `InsertChar` it expects.
        PermissionsEditor => vec![KbContext::Global],
        // Global-only, same rationale as `PermissionsEditor`: the overlay's
        // text input + nav come entirely from `add_directory::map_key`. No
        // Select/Confirmation so chars/arrows aren't resolved to filter /
        // Surface* / y-n-a before reaching the input map.
        AddDirectory => vec![KbContext::Global],
        // Global-only, same rationale as `PermissionsEditor`: the prefix field's
        // nav + text input come entirely from `permission_prefix_edit_map_key`.
        // No Select/Confirmation so chars/arrows aren't resolved to filter /
        // Surface* / Y-N-A before reaching the input map.
        PermissionPrefixEdit => vec![KbContext::Global],
        // Global-only, same rationale as `PermissionsEditor`: the dialog's nav
        // (↑/↓/Enter/x/←) comes entirely from `background_tasks_map_key`. No
        // Select/Confirmation so arrows aren't resolved to `Surface*` before
        // `update::background_tasks::intercept` sees them as `Cursor*`.
        BackgroundTasks => vec![KbContext::Global],
        // Task sits between Chat and Global so `ctrl+b` (defaults.rs:200)
        // resolves while typing in the composer. TS `defaultBindings.ts:181-188`
        // makes the Task context active whenever a backgroundable task exists;
        // we accept it unconditionally because `BackgroundAllTasks` is a no-op
        // when no foreground task is running.
        Chat => vec![KbContext::Chat, KbContext::Task, KbContext::Global],
    }
}

/// Result of consulting the resolver. `Resolved` means the resolver
/// owned the keystroke (action fired or user explicitly null-bound);
/// `NotResolved` means the legacy cascade should run.
#[derive(Debug, Clone)]
pub enum ResolverResult {
    /// Resolver fired an action.
    Action(KeybindingAction),
    /// Pending chord — caller should swallow the keystroke and render
    /// a chord status hint.
    Pending,
    /// Resolver consumed the keystroke (Esc cancelled a chord, null
    /// unbind, etc.) without an action.
    Consumed,
    /// Resolver had nothing to say — caller should fall through.
    NotResolved,
}

#[cfg(test)]
#[path = "keybinding_resolver.test.rs"]
mod tests;
