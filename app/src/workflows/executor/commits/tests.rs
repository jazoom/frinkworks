use super::*;

fn git(project: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(project)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@localhost")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@localhost")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn commit_preflight_refuses_existing_user_changes_without_touching_the_index() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path();
    let git = |args: &[&str]| git(project, args);
    git(&["init", "-q"]);
    std::fs::write(project.join("file.txt"), "initial\n").unwrap();
    std::fs::write(project.join(".gitignore"), "ignored.txt\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "initial"]);
    std::fs::write(project.join("ignored.txt"), "private data\n").unwrap();
    let clean_index = std::fs::read(project.join(".git/index")).unwrap();
    assert_eq!(require_clean_repository(project), Ok(()));
    assert_eq!(
        std::fs::read(project.join(".git/index")).unwrap(),
        clean_index
    );
    std::fs::write(project.join("file.txt"), "user change\n").unwrap();
    for staged in [false, true] {
        if staged {
            git(&["add", "file.txt"]);
        }
        let index = std::fs::read(project.join(".git/index")).unwrap();
        assert_eq!(require_clean_repository(project), Err(CommitError::Dirty));
        assert_eq!(std::fs::read(project.join(".git/index")).unwrap(), index);
        assert_eq!(
            std::fs::read(project.join("file.txt")).unwrap(),
            b"user change\n"
        );
    }
}

#[test]
fn commit_preflight_never_executes_a_repository_filesystem_monitor() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let project = directory.path();
    git(project, &["init", "-q"]);
    std::fs::write(project.join("file.txt"), "initial\n").unwrap();
    git(project, &["add", "."]);
    git(project, &["commit", "-qm", "initial"]);
    let monitor = project.join(".git/fsmonitor");
    std::fs::write(
        &monitor,
        "#!/bin/sh\nprintf executed > .git/fsmonitor-ran\nprintf 'token\\000'\n",
    )
    .unwrap();
    std::fs::set_permissions(&monitor, std::fs::Permissions::from_mode(0o700)).unwrap();
    git(project, &["config", "core.fsmonitor", ".git/fsmonitor"]);
    let index = std::fs::read(project.join(".git/index")).unwrap();

    let result = require_clean_repository(project);

    assert!(!project.join(".git/fsmonitor-ran").exists());
    assert_eq!(result, Ok(()));
    assert_eq!(std::fs::read(project.join(".git/index")).unwrap(), index);
}
