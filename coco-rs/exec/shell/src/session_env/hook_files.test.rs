use super::*;
use std::fs;

fn write(dir: &Path, name: &str, content: &str) {
    fs::write(dir.join(name), content).unwrap();
}

#[test]
fn returns_none_when_dir_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let r = SessionEnvReader::new(tmp.path(), "missing");
    assert!(r.script().is_none());
}

#[test]
fn reads_one_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = session_env_dir(tmp.path(), "abc");
    fs::create_dir_all(&dir).unwrap();
    write(&dir, "sessionstart-hook-0.sh", "export FOO=bar");

    let r = SessionEnvReader::new(tmp.path(), "abc");
    assert_eq!(r.script().as_deref(), Some("export FOO=bar"));
}

#[test]
fn priority_order_setup_first_filechanged_last() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = session_env_dir(tmp.path(), "abc");
    fs::create_dir_all(&dir).unwrap();
    // Write in reverse order so we know sort is doing the work.
    write(&dir, "filechanged-hook-0.sh", "export D=4");
    write(&dir, "cwdchanged-hook-0.sh", "export C=3");
    write(&dir, "sessionstart-hook-0.sh", "export B=2");
    write(&dir, "setup-hook-0.sh", "export A=1");

    let r = SessionEnvReader::new(tmp.path(), "abc");
    let script = r.script().unwrap();
    // Confirm the order of substring positions.
    let a = script.find("export A=1").unwrap();
    let b = script.find("export B=2").unwrap();
    let c = script.find("export C=3").unwrap();
    let d = script.find("export D=4").unwrap();
    assert!(a < b && b < c && c < d, "wrong order: {script}");
}

#[test]
fn index_order_within_event() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = session_env_dir(tmp.path(), "abc");
    fs::create_dir_all(&dir).unwrap();
    write(&dir, "sessionstart-hook-2.sh", "export THIRD=3");
    write(&dir, "sessionstart-hook-0.sh", "export FIRST=1");
    write(&dir, "sessionstart-hook-1.sh", "export SECOND=2");

    let r = SessionEnvReader::new(tmp.path(), "abc");
    let script = r.script().unwrap();
    let i1 = script.find("FIRST").unwrap();
    let i2 = script.find("SECOND").unwrap();
    let i3 = script.find("THIRD").unwrap();
    assert!(i1 < i2 && i2 < i3, "wrong order: {script}");
}

#[test]
fn ignores_other_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = session_env_dir(tmp.path(), "abc");
    fs::create_dir_all(&dir).unwrap();
    write(&dir, "sessionstart-hook-0.sh", "export FOO=bar");
    write(&dir, "garbage.txt", "nope");
    write(&dir, "sessionstart-hook-abc.sh", "nope");
    write(&dir, "unknown-event-hook-0.sh", "nope");

    let r = SessionEnvReader::new(tmp.path(), "abc");
    let s = r.script().unwrap();
    assert!(!s.contains("nope"));
    assert!(s.contains("export FOO=bar"));
}

#[test]
fn invalidate_re_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = session_env_dir(tmp.path(), "abc");
    fs::create_dir_all(&dir).unwrap();
    write(&dir, "sessionstart-hook-0.sh", "export FOO=1");

    let r = SessionEnvReader::new(tmp.path(), "abc");
    assert_eq!(r.script().unwrap(), "export FOO=1");

    write(&dir, "sessionstart-hook-0.sh", "export FOO=2");
    // Cache still serves stale.
    assert_eq!(r.script().unwrap(), "export FOO=1");

    r.invalidate();
    assert_eq!(r.script().unwrap(), "export FOO=2");
}
