//! Tests for the `QueryEngineRunner`.
//!
//! `QueryEngineRunner` now holds `Arc<SessionRuntime>` whose
//! construction needs a full `RuntimeConfig` + provider clients +
//! settings layers — building one in a unit test would essentially
//! rebuild `run_sdk_mode`. End-to-end behavior is exercised via the
//! CLI integration path; `ScriptedRunner` in `dispatcher.test.rs` is
//! the unit-level stand-in for the `TurnRunner` trait contract.
//!
//! What we keep here is the compile-time Send+Sync assertion: the
//! `SdkServerState` holds `Arc<dyn TurnRunner>` across await points,
//! so dropping that guarantee would silently break dispatch.

use super::*;

#[test]
fn runner_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<QueryEngineRunner>();
}
