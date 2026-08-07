//! Single aggregated integration-test binary: one compile + link unit instead
//! of one per file. Submodules live in `tests/suite/`. See root CLAUDE.md
//! ("Pre-commit is expensive") — fewer test binaries means less link time.
mod suite;
