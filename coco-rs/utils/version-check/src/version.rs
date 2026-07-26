//! Version comparison, narrow enough to stay obviously correct.
//!
//! A full semver implementation is not needed to answer "is the registry ahead
//! of me": released versions are `MAJOR.MINOR.PATCH`, optionally with a
//! pre-release suffix. The only rule beyond numeric ordering that matters is
//! that `1.2.0-rc.1` precedes `1.2.0`, so a user on a release candidate is told
//! about the release.

/// Whether `candidate` is a strictly newer version than `current`.
///
/// Unparseable input on either side answers `false`: a version string that
/// cannot be ordered is not evidence that an upgrade exists.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (Some(candidate), Some(current)) = (parse(candidate), parse(current)) else {
        return false;
    };
    candidate > current
}

/// Whether a version denotes a locally built binary rather than a release.
///
/// Cargo's `0.0.0` placeholder and any `+build`/`-dev` metadata mean the user
/// built it themselves; there is nothing for a registry version to be "newer"
/// than in a way they would act on.
pub fn is_source_build(version: &str) -> bool {
    version.starts_with("0.0.0") || version.contains("-dev") || version.contains('+')
}

/// `(numeric components, is_release)`. `is_release` is the tiebreaker that puts
/// `1.2.0` above `1.2.0-rc.1`, because `true > false`.
fn parse(version: &str) -> Option<([u32; 3], bool)> {
    let version = version.trim().trim_start_matches('v');
    // Homebrew cask versions carry a packaging revision after a comma
    // (`1.2.3,45`). It tracks the cask, not the software, so it says nothing
    // about which build is newer; without dropping it the whole version would
    // fail to parse and every brew install would silently never see an update.
    let version = version.split(',').next().unwrap_or(version);
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    // Build metadata is not part of precedence; drop it before parsing.
    let core = core.split('+').next().unwrap_or(core);
    let mut components = [0u32; 3];
    let mut parts = core.split('.');
    for slot in &mut components {
        // A missing component is 0 (`1.2` == `1.2.0`); a present-but-garbage one
        // makes the whole version unorderable.
        match parts.next() {
            None => break,
            Some(part) => *slot = part.parse().ok()?,
        }
    }
    if parts.next().is_some() {
        // More than three components: not a shape this compares.
        return None;
    }
    Some((components, pre.is_none()))
}

#[cfg(test)]
#[path = "version.test.rs"]
mod tests;
