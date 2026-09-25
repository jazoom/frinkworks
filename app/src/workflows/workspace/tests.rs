use super::*;

impl super::WorkflowWorkspaces {
    pub(crate) fn in_memory() -> Self {
        let hold = tempfile::tempdir().expect("workspaces");
        let root = hold.path().to_path_buf();
        storage::ensure_private_dir(&root).expect("dir");
        Self {
            root,
            _hold: Some(hold),
        }
    }
}

use super::WorkflowWorkspaces;
use crate::workflows::id::{AttemptId, RunId};

#[test]
fn recursive_disposal_does_not_follow_a_symbolic_link() {
    let workspaces = WorkflowWorkspaces::in_memory();
    let run = RunId::generate().expect("run");
    let attempt = AttemptId::generate().expect("attempt");
    let workspace = workspaces.create_attempt(run, attempt).expect("create");
    let sentinel = tempfile::tempdir().expect("outside");
    let target = sentinel.path().join("keep.txt");
    std::fs::write(&target, b"keep").expect("write");
    std::os::unix::fs::symlink(&target, workspace.project.join("link")).expect("symlink");
    workspace.destroy().expect("destroy");
    assert_eq!(std::fs::read(&target).expect("read"), b"keep");
    assert!(!workspaces.attempt_dir(run, attempt).expect("path").exists());
}

#[test]
fn recovery_retains_a_workspace_until_its_guest_is_absent() {
    let workspaces = WorkflowWorkspaces::in_memory();
    let run = RunId::generate().expect("run");
    let retained = AttemptId::generate().expect("retained");
    let removed = AttemptId::generate().expect("removed");
    let retained_workspace = workspaces.create_attempt(run, retained).expect("retained");
    let removed_workspace = workspaces.create_attempt(run, removed).expect("removed");
    let retained_path = retained_workspace.root.clone();
    let removed_path = removed_workspace.root.clone();
    drop(retained_workspace);
    drop(removed_workspace);

    let results = workspaces
        .recover_leftovers(|_, _| false, |_, attempt| *attempt == retained)
        .expect("recover");

    assert!(
        results
            .iter()
            .any(|result| { result.attempt == retained && result.remains })
    );
    assert!(
        results
            .iter()
            .any(|result| { result.attempt == removed && !result.remains })
    );
    assert!(retained_path.exists());
    assert!(!removed_path.exists());
}
