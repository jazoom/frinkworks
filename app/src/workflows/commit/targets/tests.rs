use super::*;
use crate::workflows::artefacts::{CandidateCapture, CandidatePayload};

fn git(path: &std::path::Path, args: &[&str]) {
    let result = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@localhost")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@localhost")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn commit_detection_is_bound_to_changed_reviewed_roots_not_directory_order() {
    use crate::workflows::definition::{PinnedWorkflowDefinition, StepAction};
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let parent = tempfile::tempdir().unwrap();
    let mut grants = Vec::new();
    for name in ["first", "second", "unchanged", "notes"] {
        let path = parent.path().join(name);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("file.txt"), "before\n").unwrap();
        if name != "notes" {
            git(&path, &["init", "-q"]);
            git(&path, &["add", "."]);
            git(&path, &["commit", "-qm", "initial"]);
        }
        let mut grant = DirectoryGrant::from_selected(&path, &grants).unwrap();
        grant.access = DirectoryAccess::ReviewBeforeApply;
        grants.push(grant);
    }
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "model".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        crate::agents::ToolId::ALL.to_vec(),
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(grants.clone())
    .unwrap();
    let definition = crate::workflows::seeds::correctness_security_definition(settings.environment)
        .with_conversation_settings(&settings)
        .unwrap();
    let phases = definition
        .steps()
        .iter()
        .filter(|step| matches!(step.action, StepAction::Agent(_)))
        .map(|step| crate::workflows::PhaseModelSelection {
            step: step.key.clone(),
            selection: settings.model.clone(),
            instructions: String::new(),
            preset: None,
            settings: Some(settings.clone()),
        })
        .collect();
    let run = WorkflowRun::create_source_free_for_conversation(
        crate::workflows::RunId::generate().unwrap(),
        1,
        crate::conversations::ConversationId::generate().unwrap(),
        PinnedWorkflowDefinition::pin(None, definition.clone()),
        crate::tests::test_environment_set(&definition),
        phases,
    );
    let capture = || {
        CandidatePayload::Set(
            CandidateCapture::capture_set(
                &grants,
                state.local_data.root(),
                &state.workflow_artefacts,
            )
            .unwrap(),
        )
    };
    let before = capture();
    assert!(commit_targets(&run, &before, &before).unwrap().is_empty());
    for grant in &grants[..2] {
        std::fs::write(grant.host_path.join("file.txt"), "after\n").unwrap();
    }
    let after = capture();
    let targets = commit_targets(&run, &before, &after).unwrap();
    assert_eq!(targets.len(), 2);
    assert!(grants[..2].iter().all(|grant| targets.contains(grant)));
    let mut reversed = run.clone();
    for phase in &mut reversed.phase_models {
        phase.settings.as_mut().unwrap().directories.reverse();
    }
    assert_eq!(commit_targets(&reversed, &before, &after).unwrap(), targets);
    let mut denied = run.clone();
    for phase in &mut denied.phase_models {
        phase.settings.as_mut().unwrap().directories[1].access = DirectoryAccess::ReadOnly;
    }
    assert_eq!(
        commit_targets(&denied, &before, &after),
        Err(CommitError::Authority)
    );
    let mut substituted = after.clone();
    let CandidatePayload::Set(set) = &mut substituted else {
        unreachable!()
    };
    set.roots[0].identity.inode += 1;
    assert_eq!(
        commit_targets(&run, &before, &substituted),
        Err(CommitError::Authority)
    );
    std::fs::create_dir(grants[0].host_path.join(".jj")).unwrap();
    assert_eq!(
        commit_targets(&run, &before, &after),
        Err(CommitError::Repository)
    );
    std::fs::remove_dir(grants[0].host_path.join(".jj")).unwrap();
    std::fs::write(grants[3].host_path.join("file.txt"), "changed notes\n").unwrap();
    assert_eq!(
        commit_targets(&run, &before, &capture()),
        Err(CommitError::Repository)
    );
}
