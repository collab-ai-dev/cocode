use std::ffi::OsString;

use coco_types::SessionId;

use super::*;

struct ConfigDirGuard {
    previous: Option<OsString>,
}

impl ConfigDirGuard {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::var_os(coco_utils_common::COCO_CONFIG_DIR_ENV);
        // SAFETY: this test holds the crate-wide config env lock.
        unsafe { std::env::set_var(coco_utils_common::COCO_CONFIG_DIR_ENV, path.as_os_str()) };
        Self { previous }
    }
}

impl Drop for ConfigDirGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => {
                // SAFETY: this test holds the crate-wide config env lock.
                unsafe { std::env::set_var(coco_utils_common::COCO_CONFIG_DIR_ENV, value) };
            }
            None => {
                // SAFETY: this test holds the crate-wide config env lock.
                unsafe { std::env::remove_var(coco_utils_common::COCO_CONFIG_DIR_ENV) };
            }
        }
    }
}

#[test]
#[serial_test::serial(config_env)]
fn process_announce_accepts_empty_live_session_snapshot() {
    let _lock = crate::test_support::CONFIG_ENV_LOCK.blocking_lock();
    let config_home = tempfile::TempDir::new().expect("config home tempdir");
    let _guard = ConfigDirGuard::set(config_home.path());

    let announce = announce_frame(Vec::new(), std::path::Path::new("/work"));

    assert!(announce.live_sessions.is_empty());
    assert_eq!(announce.cwd, "/work");
}

#[test]
#[serial_test::serial(config_env)]
fn process_announce_preserves_live_session_snapshot() {
    let _lock = crate::test_support::CONFIG_ENV_LOCK.blocking_lock();
    let config_home = tempfile::TempDir::new().expect("config home tempdir");
    let _guard = ConfigDirGuard::set(config_home.path());
    let first = SessionId::try_new("session-a").expect("valid session id");
    let second = SessionId::try_new("session-b").expect("valid session id");

    let announce = announce_frame(
        vec![first.clone(), second.clone()],
        std::path::Path::new("/work"),
    );

    assert_eq!(announce.live_sessions, vec![first, second]);
}
