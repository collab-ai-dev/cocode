//! Plugin dependency resolution — pure functions, no I/O.
//!
//! Semantics are `apt`-style: a dependency is a *presence guarantee*, not a
//! module graph. Plugin A depending on Plugin B means "B's namespaced
//! components (MCP servers, commands, agents) must be available when A runs."
//!
//! Two entry points:
//! - [`resolve_dependency_closure`] — install-time DFS walk, cycle detection.
//! - [`verify_and_demote`] — load-time fixed-point check, demotes plugins
//!   with unsatisfied deps.

use std::collections::BTreeMap;
use std::collections::HashSet;

use crate::identifier::INLINE_MARKETPLACE;
use crate::identifier::PluginId;

/// Minimal shape the resolver needs from a marketplace lookup.
#[derive(Debug, Clone, Default)]
pub struct DependencyLookupResult {
    pub dependencies: Vec<String>,
}

/// Outcome of [`resolve_dependency_closure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionResult {
    Ok {
        closure: Vec<PluginId>,
    },
    Cycle {
        chain: Vec<PluginId>,
    },
    NotFound {
        missing: PluginId,
        required_by: PluginId,
    },
    CrossMarketplace {
        dependency: PluginId,
        required_by: PluginId,
    },
}

/// Normalize a dependency reference to fully-qualified `name@marketplace` form.
///
/// - Bare names inherit the declarer's marketplace.
/// - **Exception**: `@inline` (--plugin-dir) plugins return bare deps unchanged
///   because the synthetic `inline` marketplace sentinel cannot meaningfully
///   resolve dep@inline. `verify_and_demote` then matches by name only.
pub fn qualify_dependency(dep: &str, declaring: &PluginId) -> PluginId {
    let parsed = PluginId::parse(dep);
    if parsed.marketplace.is_some() {
        return parsed;
    }
    match &declaring.marketplace {
        Some(m) if m != INLINE_MARKETPLACE => PluginId::new(parsed.name, m.clone()),
        _ => parsed,
    }
}

/// Walk the transitive dependency closure of `root` via DFS.
///
/// **Behavior**:
/// - The returned `closure` always contains `root`, plus every transitive
///   dependency NOT in `already_enabled`.
/// - Already-enabled deps are skipped (avoids surprise settings writes), but
///   the root is **never skipped** — re-installing must re-cache.
/// - Cross-marketplace deps blocked unless the root marketplace's
///   `allowed_cross_marketplaces` includes the target.
///   No transitive trust: A allowing B does not propagate to B's deps.
/// - Cycle detection via stack membership.
pub async fn resolve_dependency_closure<F, Fut>(
    root: &PluginId,
    lookup: F,
    already_enabled: &HashSet<PluginId>,
    allowed_cross_marketplaces: &HashSet<String>,
) -> ResolutionResult
where
    F: Fn(PluginId) -> Fut + Clone,
    Fut: std::future::Future<Output = Option<DependencyLookupResult>>,
{
    let root_marketplace = root.marketplace.clone();
    let mut closure: Vec<PluginId> = Vec::new();
    let mut visited: HashSet<PluginId> = HashSet::new();
    let mut stack: Vec<PluginId> = Vec::new();

    let result = walk(
        root,
        root,
        root,
        &root_marketplace,
        allowed_cross_marketplaces,
        already_enabled,
        &lookup,
        &mut closure,
        &mut visited,
        &mut stack,
    )
    .await;

    match result {
        WalkOutcome::Ok => ResolutionResult::Ok { closure },
        WalkOutcome::Err(e) => e,
    }
}

#[allow(clippy::too_many_arguments)]
async fn walk<F, Fut>(
    id: &PluginId,
    required_by: &PluginId,
    root: &PluginId,
    root_marketplace: &Option<String>,
    allowed: &HashSet<String>,
    already: &HashSet<PluginId>,
    lookup: &F,
    closure: &mut Vec<PluginId>,
    visited: &mut HashSet<PluginId>,
    stack: &mut Vec<PluginId>,
) -> WalkOutcome
where
    F: Fn(PluginId) -> Fut + Clone,
    Fut: std::future::Future<Output = Option<DependencyLookupResult>>,
{
    // Root is never skipped — even when already-enabled. Re-installing a plugin
    // that's in settings but missing from disk would otherwise return an empty
    // closure and the caller would skip cache+register. We compare against the
    // captured root id explicitly (not via `stack.is_empty()`, which becomes
    // false on every recursive call but still represents a "root" that
    // happens to also be its own dep — a self-cycle, not the install root).
    if id != root && already.contains(id) {
        return WalkOutcome::Ok;
    }

    // Cross-marketplace check (post already-enabled check).
    if id.marketplace != *root_marketplace {
        let allowed_here = id
            .marketplace
            .as_ref()
            .map(|m| allowed.contains(m))
            .unwrap_or(false);
        if !allowed_here {
            return WalkOutcome::Err(ResolutionResult::CrossMarketplace {
                dependency: id.clone(),
                required_by: required_by.clone(),
            });
        }
    }

    if stack.contains(id) {
        let mut chain = stack.clone();
        chain.push(id.clone());
        return WalkOutcome::Err(ResolutionResult::Cycle { chain });
    }
    if !visited.insert(id.clone()) {
        return WalkOutcome::Ok;
    }

    let entry = match lookup(id.clone()).await {
        Some(e) => e,
        None => {
            return WalkOutcome::Err(ResolutionResult::NotFound {
                missing: id.clone(),
                required_by: required_by.clone(),
            });
        }
    };

    stack.push(id.clone());
    for raw_dep in &entry.dependencies {
        let dep = qualify_dependency(raw_dep, id);
        let result = Box::pin(walk(
            &dep,
            id,
            root,
            root_marketplace,
            allowed,
            already,
            lookup,
            closure,
            visited,
            stack,
        ))
        .await;
        if let WalkOutcome::Err(e) = result {
            return WalkOutcome::Err(e);
        }
    }
    stack.pop();
    closure.push(id.clone());
    WalkOutcome::Ok
}

enum WalkOutcome {
    Ok,
    Err(ResolutionResult),
}

/// Demotion result for [`verify_and_demote`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemotionReport {
    pub demoted: HashSet<PluginId>,
    pub errors: Vec<DemotionError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemotionError {
    pub plugin: PluginId,
    pub dependency: PluginId,
    pub reason: DemotionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemotionReason {
    /// Dep exists in plugin set but is disabled.
    NotEnabled,
    /// Dep is entirely absent (not in any marketplace).
    NotFound,
}

/// Minimal shape the demoter needs about a loaded plugin.
#[derive(Debug, Clone)]
pub struct DemotePluginRecord {
    pub source: PluginId,
    pub enabled: bool,
    pub dependencies: Vec<String>,
}

/// Load-time safety net: for each enabled plugin, verify all manifest
/// dependencies are also enabled. Demote any that fail.
///
/// **Fixed-point loop**: demoting plugin A may break plugin B that depends on
/// A, so iterate until nothing changes. Bare deps from `@inline` plugins
/// match by name only against an `enabled_by_name` multiset (so demoting one
/// of two same-named plugins doesn't make the name disappear from the index).
pub fn verify_and_demote(plugins: &[DemotePluginRecord]) -> DemotionReport {
    let known: HashSet<PluginId> = plugins.iter().map(|p| p.source.clone()).collect();
    let known_by_name: HashSet<String> = plugins.iter().map(|p| p.source.name.clone()).collect();

    let mut enabled: HashSet<PluginId> = plugins
        .iter()
        .filter(|p| p.enabled)
        .map(|p| p.source.clone())
        .collect();
    let mut enabled_by_name: BTreeMap<String, usize> = BTreeMap::new();
    for id in &enabled {
        *enabled_by_name.entry(id.name.clone()).or_insert(0) += 1;
    }
    let mut errors: Vec<DemotionError> = Vec::new();

    let mut changed = true;
    while changed {
        changed = false;
        let snapshot: Vec<&DemotePluginRecord> = plugins
            .iter()
            .filter(|p| enabled.contains(&p.source))
            .collect();
        for p in snapshot {
            for raw_dep in &p.dependencies {
                let dep = qualify_dependency(raw_dep, &p.source);
                let is_bare = dep.marketplace.is_none();
                let satisfied = if is_bare {
                    enabled_by_name.get(&dep.name).copied().unwrap_or(0) > 0
                } else {
                    enabled.contains(&dep)
                };
                if !satisfied {
                    enabled.remove(&p.source);
                    let count = enabled_by_name.get(&p.source.name).copied().unwrap_or(0);
                    if count <= 1 {
                        enabled_by_name.remove(&p.source.name);
                    } else {
                        enabled_by_name.insert(p.source.name.clone(), count - 1);
                    }
                    let reason = if is_bare {
                        if known_by_name.contains(&dep.name) {
                            DemotionReason::NotEnabled
                        } else {
                            DemotionReason::NotFound
                        }
                    } else if known.contains(&dep) {
                        DemotionReason::NotEnabled
                    } else {
                        DemotionReason::NotFound
                    };
                    errors.push(DemotionError {
                        plugin: p.source.clone(),
                        dependency: dep,
                        reason,
                    });
                    changed = true;
                    break;
                }
            }
        }
    }

    let demoted: HashSet<PluginId> = plugins
        .iter()
        .filter(|p| p.enabled && !enabled.contains(&p.source))
        .map(|p| p.source.clone())
        .collect();
    DemotionReport { demoted, errors }
}

/// Find all enabled plugins that declare `target` as a dependency.
///
/// Bare deps from `@inline` plugins match by name only.
pub fn find_reverse_dependents(target: &PluginId, plugins: &[DemotePluginRecord]) -> Vec<PluginId> {
    plugins
        .iter()
        .filter(|p| {
            p.enabled
                && &p.source != target
                && p.dependencies.iter().any(|d| {
                    let qualified = qualify_dependency(d, &p.source);
                    if qualified.marketplace.is_some() {
                        &qualified == target
                    } else {
                        // bare → match by name only
                        qualified.name == target.name
                    }
                })
        })
        .map(|p| p.source.clone())
        .collect()
}

/// Format `(+ N dependencies)` install-success suffix.
pub fn format_dependency_count_suffix(installed_deps: &[PluginId]) -> String {
    let n = installed_deps.len();
    if n == 0 {
        String::new()
    } else if n == 1 {
        " (+ 1 dependency)".to_string()
    } else {
        format!(" (+ {n} dependencies)")
    }
}

/// Format `— warning: required by X, Y` uninstall suffix.
pub fn format_reverse_dependents_suffix(rdeps: &[PluginId]) -> String {
    if rdeps.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = rdeps.iter().map(|p| p.name.clone()).collect();
        format!(" — warning: required by {}", names.join(", "))
    }
}

#[cfg(test)]
#[path = "dependency.test.rs"]
mod tests;
