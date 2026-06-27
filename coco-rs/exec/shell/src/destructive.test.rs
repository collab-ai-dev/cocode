use super::*;

#[test]
fn detects_rm_recursive_and_force() {
    assert!(get_destructive_warning("rm -rf /").is_some());
    assert!(get_destructive_warning("rm -rf ~").is_some());
    assert!(get_destructive_warning("rm -rf .").is_some());
    assert!(get_destructive_warning("rm -rf *").is_some());
    assert!(get_destructive_warning("rm -r build/").is_some());
    assert!(get_destructive_warning("rm -f stale.lock").is_some());
    // A plain remove with no -r/-f is not flagged.
    assert!(get_destructive_warning("rm file.txt").is_none());
}

#[test]
fn detects_force_push() {
    let warning = get_destructive_warning("git push --force origin main");
    assert!(warning.unwrap().contains("remote history"));
    assert!(get_destructive_warning("git push -f origin main").is_some());
    assert!(get_destructive_warning("git push --force-with-lease").is_some());
    // A normal push is not flagged.
    assert!(get_destructive_warning("git push origin main").is_none());
}

#[test]
fn detects_git_data_loss() {
    assert!(get_destructive_warning("git reset --hard HEAD~1").is_some());
    assert!(get_destructive_warning("git clean -fd").is_some());
    assert!(get_destructive_warning("git clean -f").is_some());
    assert!(get_destructive_warning("git checkout -- .").is_some());
    assert!(get_destructive_warning("git restore .").is_some());
    assert!(get_destructive_warning("git stash drop").is_some());
    assert!(get_destructive_warning("git stash clear").is_some());
    assert!(get_destructive_warning("git branch -D feature").is_some());
    assert!(get_destructive_warning("git commit --amend -m msg").is_some());
    assert!(get_destructive_warning("git commit --no-verify -m msg").is_some());
}

#[test]
fn git_clean_dry_run_is_not_flagged() {
    // The dry-run exclusion must suppress the warning.
    assert!(get_destructive_warning("git clean -n").is_none());
    assert!(get_destructive_warning("git clean --dry-run").is_none());
    assert!(get_destructive_warning("git clean -fn").is_none());
}

#[test]
fn detects_sql_destructive() {
    assert!(get_destructive_warning("DROP TABLE users").is_some());
    assert!(get_destructive_warning("DROP DATABASE mydb").is_some());
    assert!(get_destructive_warning("TRUNCATE TABLE logs").is_some());
    // Case-insensitive.
    assert!(get_destructive_warning("drop table users").is_some());
    assert!(get_destructive_warning("Drop Database mydb").is_some());
    // DELETE FROM only flags a bare statement (matches TS) — `users;` or a
    // table at end-of-string — not an arbitrary trailing WHERE clause.
    assert!(get_destructive_warning("delete from users;").is_some());
    assert!(get_destructive_warning("DELETE FROM logs").is_some());
}

#[test]
fn non_ts_patterns_are_not_flagged() {
    // These patterns are not in the defined rule set and must not trigger an advisory.
    assert!(get_destructive_warning("docker rm container_id").is_none());
    assert!(get_destructive_warning("docker rmi image_id").is_none());
    assert!(get_destructive_warning("docker system prune -a").is_none());
    assert!(get_destructive_warning("shutdown -h now").is_none());
    assert!(get_destructive_warning("reboot").is_none());
    assert!(get_destructive_warning("kill -9 1234").is_none());
    assert!(get_destructive_warning("killall node").is_none());
    assert!(get_destructive_warning("pkill node").is_none());
    assert!(get_destructive_warning("mkfs.ext4 /dev/sda1").is_none());
    assert!(get_destructive_warning("dd if=/dev/zero of=/dev/sda").is_none());
}

#[test]
fn safe_commands_no_warning() {
    assert!(get_destructive_warning("ls -la").is_none());
    assert!(get_destructive_warning("git status").is_none());
    assert!(get_destructive_warning("cargo test").is_none());
    assert!(get_destructive_warning("cat file.txt").is_none());
    assert!(get_destructive_warning("git push origin main").is_none());
    assert!(get_destructive_warning("docker ps").is_none());
    assert!(get_destructive_warning("SELECT * FROM users").is_none());
}
