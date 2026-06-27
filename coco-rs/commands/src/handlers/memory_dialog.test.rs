use super::*;
use std::fs;
use tempfile::TempDir;

fn empty_handler(project: &Path, user: &Path, managed: Option<PathBuf>) -> MemoryDialogHandler {
    MemoryDialogHandler::new(project.to_path_buf(), user.to_path_buf(), managed)
}

#[tokio::test]
async fn entries_emits_only_new_placeholders_when_no_files_exist() {
    let project = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let h = empty_handler(project.path(), user.path(), None);
    let entries = h.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].scope, MemoryScope::User);
    assert!(entries[0].is_new);
    assert_eq!(entries[1].scope, MemoryScope::Project);
    assert!(entries[1].is_new);
}

#[tokio::test]
async fn entries_lists_discovered_project_claude_md() {
    let project = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    fs::write(project.path().join("CLAUDE.md"), "# project").unwrap();
    let h = empty_handler(project.path(), user.path(), None);
    let entries = h.entries();
    let project_entry = entries
        .iter()
        .find(|e| e.scope == MemoryScope::Project && !e.is_new)
        .expect("project CLAUDE.md discovered");
    assert_eq!(project_entry.label, "Project memory");
    assert!(!project_entry.is_folder);
}

#[tokio::test]
async fn entries_lists_managed_first_then_user_then_project() {
    let project = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let managed = TempDir::new().unwrap();
    fs::write(managed.path().join("CLAUDE.md"), "# managed").unwrap();
    let h = empty_handler(
        project.path(),
        user.path(),
        Some(managed.path().to_path_buf()),
    );
    let entries = h.entries();
    assert_eq!(entries[0].scope, MemoryScope::Managed);
}

#[tokio::test]
async fn entries_includes_auto_mem_folder_when_wired() {
    let project = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let memdir = TempDir::new().unwrap();
    let h = empty_handler(project.path(), user.path(), None).with_auto_mem(memdir.path().into());
    let entries = h.entries();
    let folder = entries
        .iter()
        .find(|e| e.scope == MemoryScope::AutoMemFolder)
        .expect("auto-mem folder row present");
    assert!(folder.is_folder);
    assert_eq!(folder.path, memdir.path());
}

#[tokio::test]
async fn entries_includes_team_mem_only_when_auto_mem_wired() {
    let project = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let team = TempDir::new().unwrap();
    // No auto_mem_dir set ⇒ team mem entry suppressed.
    let h = empty_handler(project.path(), user.path(), None).with_team_mem(team.path().into());
    let entries = h.entries();
    assert!(
        !entries
            .iter()
            .any(|e| e.scope == MemoryScope::TeamMemFolder),
        "team mem row gated on auto_mem_dir"
    );
}

#[tokio::test]
async fn entries_lists_agent_memories_sorted_by_type() {
    let project = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let memdir = TempDir::new().unwrap();
    let agent_z_dir = TempDir::new().unwrap();
    let agent_a_dir = TempDir::new().unwrap();
    let agents = vec![
        AgentMemoryEntry {
            agent_type: "z-agent".into(),
            scope_name: "project".into(),
            dir: agent_z_dir.path().into(),
        },
        AgentMemoryEntry {
            agent_type: "a-agent".into(),
            scope_name: "user".into(),
            dir: agent_a_dir.path().into(),
        },
    ];
    let h = empty_handler(project.path(), user.path(), None)
        .with_auto_mem(memdir.path().into())
        .with_agent_memories(agents);
    let entries = h.entries();
    let agent_rows: Vec<&MemoryFileEntry> = entries
        .iter()
        .filter(|e| e.scope == MemoryScope::AgentMemFolder)
        .collect();
    assert_eq!(agent_rows.len(), 2);
    assert!(agent_rows[0].label.contains("a-agent"));
    assert!(agent_rows[1].label.contains("z-agent"));
}

#[tokio::test]
async fn execute_emits_open_dialog() {
    let project = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let h = empty_handler(project.path(), user.path(), None);
    match h.execute_command("").await.unwrap() {
        CommandResult::OpenDialog(DialogSpec::MemoryFileSelector { entries }) => {
            assert!(!entries.is_empty());
        }
        other => panic!("unexpected: {other:?}"),
    }
}
