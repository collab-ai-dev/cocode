//! Resolution: aggregate all sources, then pick the active style.
//!
//! Resolution rules:
//!
//! 1. Start with built-in styles.
//! 2. Layer plugin → user → project → managed groups on top, in that
//!    order. A later group overwrites an earlier one when names collide
//!    (priority enforced by [`crate::catalog::OutputStyleSource::priority`]).
//! 3. The active style is determined first by checking for any plugin
//!    style with `force_for_plugin: Some(true)`; if multiple match, the
//!    first loaded style wins and a warning is logged.
//! 4. If no plugin force is set, look up `settings.output_style` in the
//!    aggregated catalog. The sentinel name `default` returns `None`.

use std::collections::HashMap;

use crate::builtin::DEFAULT_OUTPUT_STYLE_NAME;
use crate::builtin::builtin_styles;
use crate::catalog::OutputStyleConfig;
use crate::catalog::OutputStyleSource;

/// Result of aggregating every source into a name-keyed catalog.
#[derive(Debug, Clone, Default)]
pub struct Aggregated {
    /// Name → resolved config. Includes built-ins and every loaded
    /// custom/plugin style. The sentinel `default` is intentionally
    /// **absent** here — `aggregated.get("default")` correctly returns `None`.
    pub by_name: HashMap<String, OutputStyleConfig>,
    /// Catalog insertion order, matching JavaScript object key order.
    order: Vec<String>,
}

impl Aggregated {
    /// Return all loaded names in insertion order. Useful for pickers and SDK
    /// `available_output_styles`.
    pub fn names(&self) -> Vec<String> {
        self.order.clone()
    }

    /// Look up a single style by name. Returns `None` for the `default`
    /// sentinel and unknown names.
    pub fn get(&self, name: &str) -> Option<&OutputStyleConfig> {
        if name == DEFAULT_OUTPUT_STYLE_NAME {
            return None;
        }
        self.by_name.get(name)
    }
}

/// Aggregate every source into a single catalog, applying priority.
///
/// `dir_groups` is a slice of pre-loaded directory groups, each tagged
/// with its `OutputStyleSource`. The CLI walks `~/.coco/output-styles`
/// (user), the project tree (`<cwd>/.coco/output-styles` plus any
/// ancestors up to the git root), and the managed/policy directory,
/// passing each as a separate group.
///
/// Plugin styles are passed flat — they all share
/// [`OutputStyleSource::Plugin`] priority and name collisions resolve
/// by later assignment, matching JS object property writes.
pub fn aggregate(
    dir_groups: &[Vec<OutputStyleConfig>],
    plugin_styles: &[OutputStyleConfig],
) -> Aggregated {
    let mut aggregated = Aggregated {
        by_name: HashMap::new(),
        order: Vec::new(),
    };

    // Layer 0: built-ins.
    for style in builtin_styles() {
        insert_with_priority(&mut aggregated, style);
    }

    // Layer 1: plugin styles (before user/project).
    for style in plugin_styles {
        insert_with_priority(&mut aggregated, style.clone());
    }

    // Layer 2..: user / project / managed dir groups, caller-ordered.
    for group in dir_groups {
        for style in group {
            insert_with_priority(&mut aggregated, style.clone());
        }
    }

    aggregated
}

fn insert_with_priority(aggregated: &mut Aggregated, style: OutputStyleConfig) {
    use std::collections::hash_map::Entry;
    match aggregated.by_name.entry(style.name.clone()) {
        Entry::Vacant(slot) => {
            aggregated.order.push(style.name.clone());
            slot.insert(style);
        }
        Entry::Occupied(mut slot) => {
            if style.source.priority() >= slot.get().source.priority() {
                slot.insert(style);
            }
        }
    }
}

/// Outcome of evaluating plugin force-for-plugin styles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForceForPluginVerdict {
    /// No plugin style has `force_for_plugin: Some(true)`.
    None,
    /// One or more plugin styles requested force-for-plugin; the named
    /// style wins. `competing` lists the runner-ups for diagnostic
    /// logging by the caller.
    Selected {
        winner: String,
        competing: Vec<String>,
    },
}

/// Pick the active style for this session.
///
/// `settings_name` should be the value of `Settings.output_style`
/// (default `"default"`). Returns `None` when:
///
/// - The resolved name is the `default` sentinel.
/// - The resolved name doesn't exist in the catalog.
///
/// The returned bool tuple component is `true` when the active style
/// came from a plugin force-for-plugin, so the caller can log the
/// override at startup.
pub fn resolve_active_style<'a>(
    aggregated: &'a Aggregated,
    settings_name: Option<&str>,
) -> (Option<&'a OutputStyleConfig>, ForceForPluginVerdict) {
    let verdict = evaluate_force_for_plugin(aggregated);

    if let ForceForPluginVerdict::Selected { winner, .. } = &verdict
        && let Some(forced) = aggregated.by_name.get(winner)
    {
        return (Some(forced), verdict);
    }

    let name = settings_name.unwrap_or(DEFAULT_OUTPUT_STYLE_NAME);
    (aggregated.get(name), verdict)
}

fn evaluate_force_for_plugin(aggregated: &Aggregated) -> ForceForPluginVerdict {
    let forced: Vec<&OutputStyleConfig> = aggregated
        .order
        .iter()
        .filter_map(|name| {
            let style = aggregated.by_name.get(name)?;
            if matches!(style.source, OutputStyleSource::Plugin)
                && style.force_for_plugin == Some(true)
            {
                Some(style)
            } else {
                None
            }
        })
        .collect();
    if forced.is_empty() {
        return ForceForPluginVerdict::None;
    }
    let winner = forced[0].name.clone();
    let competing: Vec<String> = forced.iter().skip(1).map(|s| s.name.clone()).collect();
    ForceForPluginVerdict::Selected { winner, competing }
}

#[cfg(test)]
#[path = "resolver.test.rs"]
mod tests;
