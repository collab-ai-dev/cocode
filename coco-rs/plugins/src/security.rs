//! Plugin security validation.
//!
//! Three pillars:
//! 1. Path traversal — reject `..`, absolute paths, escaping symlinks.
//! 2. Official-name impersonation — regex + non-ASCII homograph check.
//! 3. Enterprise policy — strict marketplaces / blocklist / managed-only.

use std::collections::HashSet;
use std::path::Component;
use std::path::Path;

use crate::identifier::PluginId;

/// Result of [`validate_paths`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathValidation {
    Ok,
    Absolute,
    DotDotSegment,
    EscapesRoot,
}

/// Validate a relative path against a plugin root directory.
pub fn validate_paths(rel_path: &str) -> PathValidation {
    if rel_path.is_empty() {
        return PathValidation::Ok;
    }
    if Path::new(rel_path).is_absolute() {
        return PathValidation::Absolute;
    }
    // Reject `..` against both native sep AND literal `/`.
    for c in Path::new(rel_path).components() {
        if matches!(c, Component::ParentDir) {
            return PathValidation::DotDotSegment;
        }
    }
    if rel_path.split('/').any(|s| s == "..") {
        return PathValidation::DotDotSegment;
    }
    PathValidation::Ok
}

/// Resolve `rel` under `root` and verify the canonical result stays inside
/// (catches symlink escapes).
pub fn validate_resolved_under(root: &Path, rel: &str) -> PathValidation {
    let v = validate_paths(rel);
    if v != PathValidation::Ok {
        return v;
    }
    let combined = root.join(rel);
    let canon_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let canon = std::fs::canonicalize(&combined).unwrap_or(combined);
    if canon.starts_with(&canon_root) {
        PathValidation::Ok
    } else {
        PathValidation::EscapesRoot
    }
}

/// Impersonation regex patterns for official plugin names.
fn official_patterns() -> &'static [regex::Regex] {
    use std::sync::OnceLock;
    static PATTERNS: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        // Pattern strings are compile-time constants verified by tests below;
        // a Regex::new failure here would be a programmer error, not a
        // runtime condition. We still tolerate it gracefully by skipping any
        // pattern that fails to compile so downstream checks never panic.
        [
            // claude-plugins-official, claude-plugin-official, etc.
            r"^claude[-_]?plugins?[-_]?official(?:[-_].*)?$",
            // anthropic-* official prefixes
            r"^anthropic[-_](?:plugins?|skills?|official)",
            // claude-code-official
            r"^claude[-_]code[-_]official(?:[-_].*)?$",
        ]
        .iter()
        .filter_map(|s| match regex::Regex::new(s) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(pattern = s, error = %e, "skipping invalid impersonation regex");
                None
            }
        })
        .collect()
    })
}

/// Result of [`check_impersonation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImpersonationResult {
    Ok,
    /// Direct ASCII match against an official-name pattern.
    OfficialNameMatch {
        pattern: String,
    },
    /// Non-ASCII homograph match (NFKD-normalized matches an official pattern).
    HomographMatch {
        normalized: String,
        pattern: String,
    },
}

/// Check if a plugin name impersonates an official name.
///
/// Two checks:
/// 1. Direct regex against the raw input.
/// 2. NFKD normalization, then regex against the normalized form.
///    Catches Cyrillic 'а' → 'a' style attacks.
///
/// **Allowlist**: official plugins are permitted to use `claude*official*`
/// names — pass `official_marketplace_name` (e.g. `"claude-plugins-official"`)
/// to skip the check for plugins coming from that marketplace.
pub fn check_impersonation(name: &str, is_from_official_marketplace: bool) -> ImpersonationResult {
    if is_from_official_marketplace {
        return ImpersonationResult::Ok;
    }
    let lowered = name.to_lowercase();
    for pat in official_patterns() {
        if pat.is_match(&lowered) {
            return ImpersonationResult::OfficialNameMatch {
                pattern: pat.to_string(),
            };
        }
    }
    // NFKD homograph normalization — strip combining marks and confusables.
    let normalized = ascii_fold(&lowered);
    if normalized != lowered {
        for pat in official_patterns() {
            if pat.is_match(&normalized) {
                return ImpersonationResult::HomographMatch {
                    normalized,
                    pattern: pat.to_string(),
                };
            }
        }
    }
    ImpersonationResult::Ok
}

/// Cheap homograph-fold: map common Cyrillic/Greek confusables to ASCII.
/// Not a full NFKD implementation, but covers the confusable attack surface.
fn ascii_fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'а' | 'А' => 'a',
            'е' | 'Е' => 'e',
            'о' | 'О' => 'o',
            'р' | 'Р' => 'p',
            'с' | 'С' => 'c',
            'у' | 'У' => 'y',
            'х' | 'Х' => 'x',
            'і' | 'І' => 'i',
            'ѕ' => 's',
            'ο' | 'Ο' => 'o',
            'α' | 'Α' => 'a',
            'ε' | 'Ε' => 'e',
            'ι' | 'Ι' => 'i',
            'τ' | 'Τ' => 't',
            'ѵ' => 'v',
            other => other,
        })
        .collect()
}

/// Enterprise policy applied to plugin install / load.
#[derive(Debug, Clone, Default)]
pub struct EnterprisePolicy {
    /// Per-plugin org force-disable list, keyed by `name@marketplace`
    /// ([`PluginId`] display form). Single source of truth across install /
    /// enable / UI.
    pub blocked_plugins: HashSet<String>,
    /// Only allow plugins from approved marketplaces.
    pub strict_known_marketplaces: bool,
    /// Approved marketplace allowlist (used when strict mode is on).
    pub known_marketplaces: Vec<String>,
    /// Explicit blocklist (overrides allowlist).
    pub blocked_marketplaces: Vec<String>,
    /// Managed `strictPluginOnlyCustomization` policy. When it locks all
    /// surfaces (`true`), users cannot install plugins outside `Managed`
    /// scope. Surface-specific arrays don't lock plugin install (no
    /// `"plugins"` surface exists).
    pub strict_plugin_only_customization: coco_config::StrictPluginOnlyCustomization,
}

impl EnterprisePolicy {
    /// Build the policy from managed/enterprise (`Policy`-scope) settings.
    ///
    /// The per-plugin blocklist is every `enabled_plugins[id].enabled == false`
    /// entry in the policy layer. Marketplace-level fields map from the managed
    /// `strict_known_marketplaces` allowlist (presence ⇒ strict) and
    /// `blocked_marketplaces` denylist.
    pub fn from_managed_settings() -> Self {
        coco_config::settings::policy::load_policy_settings()
            .map(|p| Self::from_policy_settings(&p))
            .unwrap_or_default()
    }

    /// Build the policy from an already-loaded `Policy`-scope [`coco_config::Settings`].
    ///
    /// Split out from [`Self::from_managed_settings`] (the disk-loading entry
    /// point) so the settings → policy mapping is unit-testable without
    /// touching the system managed-settings path.
    pub fn from_policy_settings(policy: &coco_config::Settings) -> Self {
        let blocked_plugins = policy
            .enabled_plugins
            .iter()
            .filter(|(_, cfg)| !cfg.enabled)
            .map(|(id, _)| id.clone())
            .collect();
        Self {
            blocked_plugins,
            strict_known_marketplaces: !policy.strict_known_marketplaces.is_empty(),
            known_marketplaces: policy.strict_known_marketplaces.clone(),
            blocked_marketplaces: policy.blocked_marketplaces.clone(),
            strict_plugin_only_customization: policy.strict_plugin_only_customization.clone(),
        }
    }
}

/// Verdict from [`check_policy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    Ok,
    /// Per-plugin org force-disable.
    BlockedPlugin {
        plugin: String,
    },
    BlockedMarketplace {
        marketplace: String,
    },
    UnapprovedMarketplace {
        marketplace: String,
    },
    UserScopeForbidden,
}

/// Check a plugin against enterprise policy.
pub fn check_policy(
    plugin: &PluginId,
    is_user_scope: bool,
    policy: &EnterprisePolicy,
) -> PolicyVerdict {
    // Primary gate: per-plugin org blocklist. Applies regardless of
    // marketplace (a blocked id can be bare or `name@marketplace`).
    if policy.blocked_plugins.contains(&plugin.to_string()) {
        return PolicyVerdict::BlockedPlugin {
            plugin: plugin.to_string(),
        };
    }

    // Plugin install is gated only by the all-surfaces lock (`true`). There is
    // no `"plugins"` customization surface, so a surface-specific array never
    // blocks install — `is_restricted_to_plugin_only("plugins")` returns true
    // only for the `AllLocked(true)` form.
    if policy
        .strict_plugin_only_customization
        .is_restricted_to_plugin_only("plugins")
        && is_user_scope
    {
        return PolicyVerdict::UserScopeForbidden;
    }

    let Some(mkt) = &plugin.marketplace else {
        return PolicyVerdict::Ok;
    };

    if policy.blocked_marketplaces.iter().any(|b| b == mkt) {
        return PolicyVerdict::BlockedMarketplace {
            marketplace: mkt.clone(),
        };
    }

    if policy.strict_known_marketplaces && !policy.known_marketplaces.iter().any(|k| k == mkt) {
        return PolicyVerdict::UnapprovedMarketplace {
            marketplace: mkt.clone(),
        };
    }
    PolicyVerdict::Ok
}

#[cfg(test)]
#[path = "security.test.rs"]
mod tests;
