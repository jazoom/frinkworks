use std::path::PathBuf;

use super::super::definition::{
    AgentAuthority, CandidateAuthority, GuestDirectoryAccess, SystemCommandId,
};
use super::{
    StepOutcome, SuccessAttempt, attempt_spec, cleanup_after_start_failure, guest_command,
    intersect_authority, publish_success, record_unknown_observed,
};
use crate::agents::{AccessMode, AgentId, DirectoryPolicy, PolicyGrant};
use crate::sandbox::GUEST_PROJECT;
use crate::sessions::JobStatus;
use crate::workflows::capabilities::{CapabilityDirectory, DirectoryRole};

fn git_text(path: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
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
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn multi_repository_recovery_preserves_successful_commits_and_restores_only_uncommitted_roots() {
    use crate::workflows::{artefacts::*, commit::*, definition::*, run::*};
    for updated in [[false, false], [true, false], [true, true]] {
        let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
        let root = tempfile::tempdir().unwrap();
        let context = root.path().join("context");
        let project = root.path().join("repository");
        std::fs::create_dir(&context).unwrap();
        std::fs::create_dir(&project).unwrap();
        for path in [&context, &project] {
            git_text(path, &["init", "-q"]);
            std::fs::write(path.join("file.txt"), "initial\n").unwrap();
            git_text(path, &["add", "."]);
            git_text(path, &["commit", "-qm", "initial"]);
        }
        let mut context_grant =
            crate::execution::DirectoryGrant::from_selected(&context, &[]).unwrap();
        context_grant.access = crate::execution::DirectoryAccess::ReviewBeforeApply;
        let mut destination = crate::execution::DirectoryGrant::from_selected(
            &project,
            std::slice::from_ref(&context_grant),
        )
        .unwrap();
        destination.access = crate::execution::DirectoryAccess::ReviewBeforeApply;
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
        .with_directories(vec![context_grant, destination.clone()])
        .unwrap();
        let definition =
            crate::workflows::seeds::correctness_security_definition(settings.environment)
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
        let mut run = WorkflowRun::create_source_free_for_conversation(
            crate::workflows::RunId::generate().unwrap(),
            1,
            crate::conversations::ConversationId::generate().unwrap(),
            PinnedWorkflowDefinition::pin(None, definition.clone()),
            crate::tests::test_environment_set(&definition),
            phases,
        );
        run.kind = RunKind::Configured;
        let capture = || {
            CandidatePayload::Set(
                CandidateCapture::capture_set(
                    &settings.directories,
                    state.local_data.root(),
                    &state.workflow_artefacts,
                )
                .unwrap(),
            )
        };
        let initial = capture();
        for grant in &settings.directories {
            std::fs::write(grant.host_path.join("file.txt"), "target\n").unwrap();
        }
        let target = capture();
        assert_eq!(commit_targets(&run, &initial, &target).unwrap().len(), 2);
        let publish = |payload: &CandidatePayload, producer: ArtefactProducer| {
            let bytes = payload.manifest_bytes().unwrap();
            ArtefactRecord {
                id: crate::workflows::ArtefactId::generate().unwrap(),
                kind: ArtefactKind::CandidateRevision,
                artefact_hash: artefact_hash_for(ArtefactKind::CandidateRevision, 1, &bytes),
                object_hash: state.workflow_artefacts.publish(&bytes).unwrap(),
                payload_bytes: bytes.len() as u64,
                created_at_ms: 1,
                provenance: ArtefactProvenance {
                    run_id: run.id,
                    producer,
                    inputs: Vec::new(),
                },
                summary: ArtefactSummary::Candidate {
                    candidate: payload.candidate_hash(),
                    entries: payload.entry_count(),
                    bytes: payload.byte_count(),
                    disposition: ProductionDisposition::RequiredOutput,
                },
            }
        };
        let initial_record = publish(&initial, ArtefactProducer::RunSourceCapture);
        let target_record = publish(
            &target,
            ArtefactProducer::StepAttempt {
                attempt_id: crate::workflows::AttemptId::generate().unwrap(),
                step: StepKey::parse("implementer").unwrap(),
                output: Some(OutputKey::parse("candidate").unwrap()),
                disposition: ProductionDisposition::RequiredOutput,
            },
        );
        run.record_initial_candidate(initial_record).unwrap();
        let candidate = ArtefactReference {
            id: target_record.id,
            kind: target_record.kind,
            artefact_hash: target_record.artefact_hash,
        };
        run.artefacts.push(target_record);
        let RunSource::Captured { source } = &mut run.source else {
            panic!("source")
        };
        source.accepted = candidate.clone();
        source.observed = ObservedCandidate::Exact {
            artefact: candidate.clone(),
        };
        let mut reviews = Vec::new();
        let mut inputs = vec![AttemptArtefactInput {
            key: InputKey::parse("candidate").unwrap(),
            artefact: candidate.clone(),
        }];
        for name in ["correctness-review", "security-review"] {
            let (bytes, object_hash, artefact_hash) = payload::encode_review(
                target.candidate_hash(),
                ReviewVerdict::Approved,
                "approved",
                None,
            )
            .unwrap();
            state.workflow_artefacts.publish(&bytes).unwrap();
            let record = ArtefactRecord {
                id: crate::workflows::ArtefactId::generate().unwrap(),
                kind: ArtefactKind::ReviewReport,
                artefact_hash,
                object_hash,
                payload_bytes: bytes.len() as u64,
                created_at_ms: 1,
                provenance: ArtefactProvenance {
                    run_id: run.id,
                    producer: ArtefactProducer::StepAttempt {
                        attempt_id: crate::workflows::AttemptId::generate().unwrap(),
                        step: StepKey::parse(name).unwrap(),
                        output: Some(OutputKey::parse("review").unwrap()),
                        disposition: ProductionDisposition::RequiredOutput,
                    },
                    inputs: vec![candidate.clone()],
                },
                summary: ArtefactSummary::Review {
                    candidate: target.candidate_hash(),
                    verdict: ReviewVerdict::Approved,
                },
            };
            let reference = ArtefactReference {
                id: record.id,
                kind: record.kind,
                artefact_hash,
            };
            inputs.push(AttemptArtefactInput {
                key: InputKey::parse(name).unwrap(),
                artefact: reference.clone(),
            });
            reviews.push(reference);
            run.artefacts.push(record);
        }
        let step = definition.steps().iter().find(|step| matches!(&step.action, StepAction::SystemCommand(action) if action.command == SystemCommandId::CommitCandidate)).unwrap();
        assert!(require_commit_approval(&run, step, &inputs, &state.workflow_artefacts).is_ok());
        run.state = RunState::Ready {
            step: step.key.clone(),
        };
        let authority =
            crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
        let attempt = crate::workflows::AttemptId::generate().unwrap();
        run.start_attempt(
            attempt,
            inputs,
            crate::workflows::capabilities::AttemptCapabilities::derive_project_free(
                step, &authority,
            )
            .unwrap(),
            AttemptSandboxRecord {
                kind: AttemptSandboxKind::IsolatedAttempt,
                snapshot_digest: run
                    .environments
                    .steps
                    .iter()
                    .find(|binding| binding.step == step.key)
                    .unwrap()
                    .snapshot_digest
                    .clone(),
            },
            2,
        )
        .unwrap();
        let mut roots = Vec::new();
        let mut indices = Vec::new();
        for (grant, reference_updated) in settings.directories.iter().zip(updated) {
            let project = &grant.host_path;
            let old = git_text(project, &["rev-parse", "HEAD"]);
            let reference = git_text(project, &["symbolic-ref", "HEAD"]);
            let original_index = std::fs::read(project.join(".git/index")).unwrap();
            let journal = state
                .commit_journals
                .create_for_directory(run.id, attempt, grant.id)
                .unwrap();
            git_text(project, &["add", "."]);
            let tree = git_text(project, &["write-tree"]);
            let commit = git_text(
                project,
                &["commit-tree", &tree, "-p", &old, "-m", "candidate"],
            );
            let target_index = std::fs::read(project.join(".git/index")).unwrap();
            std::fs::write(project.join(".git/index"), &original_index).unwrap();
            journal
                .write_index_backup("original.index", &original_index)
                .unwrap();
            journal
                .write_index_backup("target.index", &target_index)
                .unwrap();
            journal.flush().unwrap();
            if reference_updated {
                git_text(project, &["update-ref", &reference, &commit, &old]);
            }
            indices.push(if reference_updated {
                target_index
            } else {
                original_index
            });
            roots.push(CommitRoot {
                grant: grant.clone(),
                state: if reference_updated {
                    CommitTransactionState::ReferenceUpdated {
                        commit: commit.clone(),
                    }
                } else {
                    CommitTransactionState::WorktreeApplied
                },
                expected_reference: reference,
                old_object: Some(old),
                target_tree: Some(tree),
                expected_commit: Some(commit),
                timestamp: "1700000000 +0000".to_owned(),
            });
        }
        let transaction = CommitTransaction {
            candidate,
            reviews,
            approval: None,
            roots,
        };
        run.record_commit_transaction(attempt, transaction.clone())
            .unwrap();
        let id = run.id;
        state.workflow_runs.create(run).unwrap();
        super::recover_commit_transactions(&state).unwrap();
        let recovered = state.workflow_runs.get(&id).unwrap();
        assert_eq!(
            recovered.state == RunState::Completed,
            updated.iter().all(|updated| *updated)
        );
        let recovered_transaction = recovered.attempts[0].commit_transaction.as_ref().unwrap();
        for (index, (root, reference_updated)) in transaction.roots.iter().zip(updated).enumerate()
        {
            let project = &root.grant.host_path;
            let expected_head = if reference_updated {
                &root.expected_commit
            } else {
                &root.old_object
            };
            assert_eq!(
                &git_text(project, &["rev-parse", "HEAD"]),
                expected_head.as_ref().unwrap()
            );
            assert_eq!(
                std::fs::read(project.join("file.txt")).unwrap(),
                if reference_updated {
                    b"target\n".to_vec()
                } else {
                    b"initial\n".to_vec()
                }
            );
            assert_eq!(
                std::fs::read(project.join(".git/index")).unwrap(),
                indices[index]
            );
            assert_eq!(
                recovered_transaction.roots[index]
                    .verified_commit()
                    .is_some(),
                reference_updated
            );
        }
        assert!(state.commit_journals.load(id, attempt).is_err());
    }
}

#[test]
fn project_free_mounts_require_direct_write_authority_for_live_host_writes() {
    let root = tempfile::tempdir().unwrap();
    let host = root.path().join("project");
    std::fs::create_dir(&host).unwrap();
    let grant = crate::execution::DirectoryGrant::from_selected(&host, &[]).unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "test".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        vec![crate::agents::ToolId::Write],
        crate::tests::test_environment_id(),
    )
    .unwrap();
    let workspace = crate::workflows::workspace::AttemptWorkspace {
        root: root.path().join("attempt"),
        project: root.path().join("attempt/workspace"),
    };
    let mut direct = grant.clone();
    direct.access = crate::execution::DirectoryAccess::DirectWrite;
    for grants in [Vec::new(), vec![grant.clone()], vec![direct]] {
        let settings = settings.clone().with_directories(grants.clone()).unwrap();
        let authority =
            crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
        let pinned = crate::workflows::pin_project_free_quick_task_with_directories(
            &settings.tools,
            "",
            crate::tests::test_environment_id(),
            grants
                .iter()
                .map(|grant| GuestDirectoryAccess {
                    alias: grant.alias.clone(),
                    access: AccessMode::ReadOnly,
                })
                .collect(),
            false,
        )
        .unwrap();
        let capabilities =
            crate::workflows::capabilities::AttemptCapabilities::derive_project_free(
                &pinned.definition.steps()[0],
                &authority,
            )
            .unwrap();
        let spec =
            super::project_free_attempt_spec(&capabilities, &workspace, &authority, None).unwrap();
        assert_eq!(spec.mounts.len(), grants.len() + 1);
        assert_eq!(spec.mounts[0].host, workspace.project);
        assert_eq!(spec.mounts[0].guest, "/workspace");
        assert!(!spec.mounts[0].read_only);
        assert_eq!(
            authority.policy.resolve("/workspace/output").unwrap().1,
            AccessMode::ReadWrite
        );
        if !grants.is_empty() {
            assert_eq!(spec.mounts[1].host, host);
            assert_eq!(spec.mounts[1].guest, grant.guest_path());
            let direct = grants[0].access == crate::execution::DirectoryAccess::DirectWrite;
            assert_eq!(spec.mounts[1].read_only, !direct);
            assert_eq!(spec.workdir, grant.guest_path());
            let access = authority.policy.resolve("file").unwrap().1;
            assert_eq!(access.is_writable(), direct);
            crate::sandbox::confirm_host_write_access(&spec, &host, access).unwrap();
            let mut forged = capabilities.clone();
            forged.directories[0].access = AccessMode::ReadWrite;
            assert_eq!(
                super::project_free_attempt_spec(&forged, &workspace, &authority, None).is_ok(),
                direct
            );
        } else {
            assert_eq!(spec.workdir, "/workspace");
        }
    }
}

#[test]
fn mixed_quick_task_mounts_only_reviewed_roots_as_copies() {
    let root = tempfile::tempdir().unwrap();
    let mut grants = Vec::new();
    for (name, access) in [
        ("direct", crate::execution::DirectoryAccess::DirectWrite),
        (
            "reviewed",
            crate::execution::DirectoryAccess::ReviewBeforeApply,
        ),
        ("reference", crate::execution::DirectoryAccess::ReadOnly),
    ] {
        let path = root.path().join(name);
        std::fs::create_dir(&path).unwrap();
        let mut grant = crate::execution::DirectoryGrant::from_selected(&path, &grants).unwrap();
        grant.access = access;
        grants.push(grant);
    }
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "test".to_owned(),
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
    let authority = crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    let pinned = crate::workflows::pin_project_free_quick_task_with_directories(
        &settings.tools,
        "",
        settings.environment,
        grants
            .iter()
            .map(|grant| GuestDirectoryAccess {
                alias: grant.alias.clone(),
                access: AccessMode::ReadOnly,
            })
            .collect(),
        true,
    )
    .unwrap();
    let capabilities = crate::workflows::capabilities::AttemptCapabilities::derive_project_free(
        &pinned.definition.steps()[0],
        &authority,
    )
    .unwrap();
    let workspace = crate::workflows::workspace::AttemptWorkspace {
        root: root.path().join("attempt"),
        project: root.path().join("attempt/workspace"),
    };
    let spec =
        super::project_free_attempt_spec(&capabilities, &workspace, &authority, None).unwrap();
    assert_eq!(spec.mounts[1].host, grants[0].host_path);
    assert!(!spec.mounts[1].read_only);
    assert_eq!(
        spec.mounts[2].host,
        workspace.reviewed_root(&grants[1].alias).unwrap()
    );
    assert!(!spec.mounts[2].read_only);
    assert_eq!(spec.mounts[3].host, grants[2].host_path);
    assert!(spec.mounts[3].read_only);
    assert_eq!(authority.reviewed_aliases, vec![grants[1].alias.clone()]);
}

#[test]
fn read_only_review_mounts_the_pinned_copy_instead_of_live_host_files() {
    let root = tempfile::tempdir().unwrap();
    let reference = root.path().join("reference");
    let host = root.path().join("editable");
    std::fs::create_dir(&reference).unwrap();
    std::fs::create_dir(&host).unwrap();
    let reference = crate::execution::DirectoryGrant::from_selected(&reference, &[]).unwrap();
    let mut reviewed =
        crate::execution::DirectoryGrant::from_selected(&host, std::slice::from_ref(&reference))
            .unwrap();
    reviewed.access = crate::execution::DirectoryAccess::ReviewBeforeApply;
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
    .with_directories(vec![reference.clone(), reviewed.clone()])
    .unwrap();
    let authority = crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    let definition = crate::workflows::seeds::implement_and_review_definition(settings.environment)
        .with_conversation_settings(&settings)
        .unwrap();
    let workspace = crate::workflows::workspace::AttemptWorkspace {
        root: root.path().join("attempt"),
        project: root.path().join("attempt/workspace"),
    };
    for (index, writable) in [(0, true), (1, false)] {
        let capabilities =
            crate::workflows::capabilities::AttemptCapabilities::derive_project_free(
                &definition.steps()[index],
                &authority,
            )
            .unwrap();
        let spec =
            super::project_free_attempt_spec(&capabilities, &workspace, &authority, None).unwrap();
        assert_eq!(spec.workdir, reference.guest_path());
        let mount = spec
            .mounts
            .iter()
            .find(|mount| mount.guest == reviewed.guest_path())
            .unwrap();
        assert_eq!(
            mount.host,
            workspace.reviewed_root(&reviewed.alias).unwrap()
        );
        assert_ne!(mount.host, host);
        assert_eq!(mount.read_only, !writable);
        assert!(
            spec.mounts
                .iter()
                .find(|mount| mount.guest == reference.guest_path())
                .unwrap()
                .read_only
        );
    }
}

#[test]
fn sensitive_dispatch_requires_live_consent_and_the_original_directory() {
    let mut state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let data = home.join("power-plant-data");
    std::fs::create_dir_all(&data).unwrap();
    state.local_data = crate::local_data::LocalDataReset::for_test(data);
    let grant = crate::execution::DirectoryGrant::from_selected(&home, &[]).unwrap();
    let settings = crate::execution::ExecutionSettings::new(
        crate::providers::ModelSelection::new(
            crate::providers::ProviderKind::Xai,
            "model".to_owned(),
            None,
        )
        .unwrap(),
        String::new(),
        vec![crate::agents::ToolId::Read],
        crate::tests::test_environment_id(),
    )
    .unwrap()
    .with_directories(vec![grant.clone()])
    .unwrap();
    let record = state
        .conversations
        .create_saved(
            crate::conversations::ConversationId::generate().unwrap(),
            None,
            Some(crate::conversations::ConversationModelConfiguration {
                settings: settings.clone(),
                preset: None,
            }),
            Vec::new(),
        )
        .unwrap();
    let authority = crate::execution::ProjectFreeAuthority::from_settings(1, &settings).unwrap();
    let run_id = crate::workflows::RunId::generate().unwrap();
    let pinned = crate::workflows::pin_project_free_quick_task_with_directories(
        &settings.tools,
        "",
        settings.environment,
        vec![GuestDirectoryAccess {
            alias: grant.alias.clone(),
            access: AccessMode::ReadOnly,
        }],
        false,
    )
    .unwrap();
    let environments = crate::workflows::resolve::tests::test_set(&pinned.definition);
    state
        .workflow_runs
        .create(
            crate::workflows::WorkflowRun::create_source_free_for_conversation(
                run_id,
                1,
                record.id,
                pinned,
                environments,
                Vec::new(),
            ),
        )
        .unwrap();
    let session = crate::sessions::generate_session_token().unwrap().id();
    state.sessions.insert(session);
    let job = super::WorkflowJob {
        run_id,
        session_id: session,
        agent_id: None,
        agent_revision: 0,
        conversation_id: Some(record.id),
        authority: None,
        host_policy: authority.policy.clone(),
        project_free_authority: Some(authority),
        grant_alias: grant.alias.clone(),
        connection: crate::providers::ProviderConnection::with_key(
            crate::providers::ProviderKind::Xai,
            "key",
            "model",
        ),
        phase_providers: Vec::new(),
        active_connection: std::sync::Arc::new(std::sync::Mutex::new(None)),
        turns: Vec::new(),
        job: crate::sessions::Job::new(crate::sessions::JobId::generate().unwrap(), run_id, 0),
        eligible_reply: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
    };
    assert!(super::confirm_run_authority(&state, &job).is_err());
    let request = state
        .access_consent
        .request_conversation(session, record.id, &settings, &grant)
        .unwrap();
    state
        .access_consent
        .approve_conversation(&request, session, record.id, &settings, &grant)
        .unwrap();
    assert!(super::confirm_run_authority(&state, &job).is_ok());

    state
        .conversations
        .rename(&record.id, record.revision, "Renamed".to_owned())
        .unwrap();
    assert!(super::confirm_run_authority(&state, &job).is_ok());
    let old = root.path().join("old-home");
    std::fs::rename(&home, &old).unwrap();
    std::fs::create_dir(&home).unwrap();
    assert!(super::confirm_run_authority(&state, &job).is_err());
    std::fs::remove_dir(&home).unwrap();
    std::fs::rename(&old, &home).unwrap();
    assert!(super::confirm_run_authority(&state, &job).is_ok());
    state
        .sessions
        .advance_clock(crate::sessions::SESSION_LIFETIME + std::time::Duration::from_secs(1));
    assert!(super::confirm_run_authority(&state, &job).is_err());
}

#[test]
fn repository_status_uses_the_fixed_guest_command() {
    let exec = guest_command(SystemCommandId::RepositoryStatus);
    assert_eq!(exec.program, "git");
    assert_eq!(
        exec.args,
        ["status".to_owned(), "--porcelain=v1".to_owned()]
    );
    assert_eq!(exec.cwd, GUEST_PROJECT);
    assert!(exec.stdin.is_none());
}

#[test]
fn candidate_authority_keeps_the_project_mount_read_only() {
    let host = DirectoryPolicy::from_grants(
        vec![
            PolicyGrant {
                alias: "project".to_owned(),
                guest_path: GUEST_PROJECT.to_owned(),
                host_path: PathBuf::from("/host/project"),
                access: AccessMode::ReadWrite,
            },
            PolicyGrant {
                alias: "docs".to_owned(),
                guest_path: "/access/docs".to_owned(),
                host_path: PathBuf::from("/host/docs"),
                access: AccessMode::ReadOnly,
            },
        ],
        "project".to_owned(),
    );
    let authority = AgentAuthority::new(
        Vec::new(),
        vec![GuestDirectoryAccess {
            alias: "docs".to_owned(),
            access: AccessMode::ReadOnly,
        }],
    )
    .expect("authority");
    let policy =
        intersect_authority(CandidateAuthority::ReadOnly, &authority, &host).expect("intersection");
    assert_eq!(policy.primary_guest(), GUEST_PROJECT);
    assert_eq!(
        policy.resolve(""),
        Ok((GUEST_PROJECT.to_owned(), AccessMode::ReadOnly))
    );
    assert_eq!(
        policy.resolve("/access/docs"),
        Ok(("/access/docs".to_owned(), AccessMode::ReadOnly))
    );
}

#[test]
fn selected_primary_cannot_also_be_secondary_context() {
    let host = DirectoryPolicy::from_grants(
        vec![PolicyGrant {
            alias: "docs".to_owned(),
            guest_path: GUEST_PROJECT.to_owned(),
            host_path: PathBuf::from("/host/docs"),
            access: AccessMode::ReadWrite,
        }],
        "docs".to_owned(),
    );
    let authority = AgentAuthority::new(
        Vec::new(),
        vec![GuestDirectoryAccess {
            alias: "docs".to_owned(),
            access: AccessMode::ReadOnly,
        }],
    )
    .expect("authority");

    assert!(intersect_authority(CandidateAuthority::ReadOnly, &authority, &host).is_err());
}

#[test]
fn fixing_review_publication_is_atomic_across_failures() {
    enum Failure {
        CandidatePublication,
        ReportPublication,
        RunMutation,
    }

    for failure in [
        Failure::CandidatePublication,
        Failure::ReportPublication,
        Failure::RunMutation,
    ] {
        let (state, job, step, attempt, inputs, captured, drafts) = fixing_publication_fixture();
        match failure {
            Failure::CandidatePublication => state.workflow_artefacts.fail_publish_after(0),
            Failure::ReportPublication => state.workflow_artefacts.fail_publish_after(1),
            Failure::RunMutation => state.workflow_runs.fail_next_mutation(),
        }

        let captured = crate::workflows::artefacts::CandidatePayload::Revision(captured);
        assert!(
            publish_success(
                &state,
                &job,
                &step,
                SuccessAttempt {
                    id: attempt,
                    complete: true,
                },
                &inputs,
                &drafts,
                Some(&captured),
            )
            .is_err()
        );
        let run = state.workflow_runs.get(&job.run_id).expect("run");
        assert!(run.attempts[0].outputs.is_empty());
        assert_eq!(run.artefacts.len(), 1);
    }
}

#[tokio::test]
async fn configured_dispatch_excludes_history_but_quick_tasks_keep_it() {
    for kind in [
        crate::workflows::RunKind::Configured,
        crate::workflows::RunKind::QuickTask,
    ] {
        let (mut state, mut job, step, attempt, _, _, drafts) = fixing_publication_fixture();
        job.connection = crate::providers::ProviderConnection::with_key(
            crate::providers::ProviderKind::Xai,
            "test-private-context-credential",
            "model",
        );
        let backend = crate::tests::ScriptedBackend::accept();
        state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend.clone()));
        state
            .workflow_runs
            .mutate(&job.run_id, |run| {
                run.launch_brief = "Inspect only the assigned candidate.".to_owned();
                run.kind = kind;
                Ok(())
            })
            .expect("brief");
        job.conversation_id =
            Some(crate::conversations::ConversationId::generate().expect("conversation"));
        job.turns = vec![crate::providers::ChatTurn::user(
            "PRIVATE DISCUSSION".to_owned(),
        )];
        let sandbox = state.sandboxes.attempt_handle(job.run_id, attempt);
        let directory = tempfile::tempdir().expect("project");
        job.host_policy = DirectoryPolicy::from_grants(
            vec![PolicyGrant {
                alias: "project".to_owned(),
                guest_path: GUEST_PROJECT.to_owned(),
                host_path: directory.path().to_path_buf(),
                access: AccessMode::ReadWrite,
            }],
            "project".to_owned(),
        );
        sandbox
            .start_from_snapshot(
                std::path::Path::new("snapshot"),
                "sha256:deadbeef",
                crate::sandbox::SandboxSpec {
                    mounts: vec![crate::sandbox::MountSpec {
                        guest: GUEST_PROJECT.to_owned(),
                        host: directory.path().to_path_buf(),
                        read_only: true,
                    }],
                    workdir: GUEST_PROJECT.to_owned(),
                    network: crate::agents::NetworkAccess::None,
                },
            )
            .await
            .expect("sandbox");
        let crate::workflows::definition::StepAction::Agent(action) = &step.action else {
            panic!("agent phase")
        };
        let outcome = super::run_agent_step(
            &state,
            &job,
            action,
            Some(&sandbox),
            std::sync::Arc::new(drafts),
        )
        .await;
        if backend.last_preamble().is_none()
            && let StepOutcome::Failed { error, .. } = outcome
        {
            panic!("dispatch failed: {error:?}");
        }
        let evidence = state
            .workflow_evidence
            .get(&job.run_id, &attempt)
            .expect("attempt evidence");
        let terminal = evidence.terminal.expect("terminal response");
        assert!(!terminal.text.is_empty());
        if kind == crate::workflows::RunKind::Configured {
            assert!(job.job.snapshot().output.text.is_empty());
        } else {
            assert_eq!(job.job.snapshot().output.text, terminal.text);
        }
        let preamble = backend.last_preamble().expect("provider request");
        let snapshot = state
            .workflow_runs
            .get(&job.run_id)
            .and_then(|run| {
                run.attempts
                    .last()
                    .and_then(|attempt| attempt.initial_context.clone())
            })
            .expect("initial context snapshot");
        assert_eq!(snapshot.prompt, preamble);
        assert_eq!(snapshot.request_messages(), backend.last_history());
        assert!(
            !backend.last_history().is_empty(),
            "Rig requires a request message"
        );
        assert_eq!(
            snapshot
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            backend
                .last_tools()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert!(preamble.contains("Inspect only the assigned candidate."));
        assert!(preamble.contains("# Project instructions"));
        assert!(!preamble.contains("PRIVATE DISCUSSION"));
        assert_eq!(
            backend
                .last_history()
                .iter()
                .any(|turn| turn.text.contains("PRIVATE DISCUSSION")),
            kind == crate::workflows::RunKind::QuickTask
        );
        sandbox.stop().await.expect("stop");
        sandbox.remove().await.expect("remove");
    }
}

fn fixing_publication_fixture() -> (
    crate::state::AppState,
    crate::workflows::WorkflowJob,
    crate::workflows::definition::StepDefinition,
    crate::workflows::AttemptId,
    Vec<crate::workflows::run::AttemptArtefactInput>,
    crate::workflows::artefacts::candidate::CandidateRevisionArtefact,
    std::sync::Mutex<crate::workflows::artefacts::output::OutputDrafts>,
) {
    use crate::workflows::definition::{
        AgentStep, ArtefactKind, OutputKey, OutputKind, RequiredOutput, RoleDefinition, RoleKey,
        StepAction, StepDefinition, StepEnvironment, StepKey, WorkflowDefinition,
        candidate_revision_output, initial_candidate_input,
    };

    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let project = tempfile::tempdir().expect("project");
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(project.path())
            .status()
            .expect("init")
            .success()
    );
    std::fs::write(project.path().join("file.txt"), b"candidate\n").expect("source");
    let captured = crate::workflows::artefacts::CandidateCapture::capture_host(
        project.path(),
        &state.workflow_artefacts,
    )
    .expect("capture");
    let role_key = RoleKey::parse("reviewer").expect("role key");
    let step = StepDefinition {
        key: StepKey::parse("fixing-reviewer").expect("step key"),
        name: "Fixing reviewer".to_owned(),
        inputs: vec![initial_candidate_input()],
        action: StepAction::Agent(AgentStep {
            environment: StepEnvironment::WorkflowDefault,
            role: role_key.clone(),
            candidate_authority: CandidateAuthority::Edit,
            authority: AgentAuthority::new(vec![crate::agents::ToolId::List], Vec::new())
                .expect("authority"),
            settings: crate::workflows::definition::ModelStepSettings::SameAsRunDefaults,
            required_outputs: vec![
                RequiredOutput {
                    key: OutputKey::parse("assistant-reply").expect("reply"),
                    kind: OutputKind::AssistantReply,
                },
                candidate_revision_output(),
                RequiredOutput {
                    key: OutputKey::parse("review").expect("review"),
                    kind: OutputKind::ReviewReport,
                },
            ],
        }),
        review: None,
    };
    let definition = WorkflowDefinition::from_parts(
        "Fixing".to_owned(),
        crate::tests::test_environment_id(),
        vec![
            RoleDefinition::new(
                role_key,
                "Reviewer".to_owned(),
                String::new(),
                String::new(),
            )
            .expect("role"),
        ],
        vec![step.clone()],
    )
    .expect("definition");
    let environments = crate::tests::test_environment_set(&definition);
    let mut run = crate::workflows::WorkflowRun::configured(
        crate::workflows::RunId::generate().expect("run"),
        1,
        AgentId::generate().expect("agent"),
        crate::workflows::definition::PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    let initial = candidate_record(&run, &captured, &state.workflow_artefacts, true);
    let initial_reference = crate::workflows::artefacts::ArtefactReference {
        id: initial.id,
        kind: ArtefactKind::CandidateRevision,
        artefact_hash: initial.artefact_hash,
    };
    run.record_initial_candidate(initial)
        .expect("initial candidate");
    let inputs = vec![crate::workflows::run::AttemptArtefactInput {
        key: crate::workflows::definition::InputKey::parse("candidate").expect("input"),
        artefact: initial_reference,
    }];
    let attempt = crate::workflows::AttemptId::generate().expect("attempt");
    run.start_attempt(
        attempt,
        inputs.clone(),
        crate::tests::test_agent_capabilities(),
        crate::workflows::run::AttemptSandboxRecord {
            kind: crate::workflows::run::AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: run.environments.steps[0].snapshot_digest.clone(),
        },
        2,
    )
    .expect("start");
    run.record_cleanup(
        attempt,
        crate::workflows::run::AttemptCleanupRecord::Complete,
    )
    .expect("cleanup");
    let run_id = run.id;
    state.workflow_runs.create(run).expect("store run");
    let mut output_drafts = crate::workflows::artefacts::output::OutputDrafts::default();
    output_drafts
        .submit(
            step.required_outputs(),
            "review",
            OutputKind::ReviewReport,
            Some("Approved".to_owned()),
            Some("approved"),
            None,
            false,
            false,
        )
        .expect("review draft");
    let token = crate::sessions::generate_session_token().expect("session");
    let job = crate::workflows::WorkflowJob {
        run_id,
        session_id: token.id(),
        agent_id: Some(AgentId::generate().expect("agent")),
        agent_revision: 1,
        conversation_id: None,
        authority: None,
        project_free_authority: None,
        grant_alias: "project".to_owned(),
        connection: crate::providers::ProviderConnection::with_key(
            crate::providers::ProviderKind::Xai,
            "key",
            "model",
        ),
        phase_providers: Vec::new(),
        active_connection: std::sync::Arc::new(std::sync::Mutex::new(None)),
        host_policy: DirectoryPolicy::from_grants(Vec::new(), "project".to_owned()),
        turns: Vec::new(),
        job: crate::sessions::Job::new(crate::sessions::JobId::generate().expect("job"), run_id, 0),
        eligible_reply: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
    };
    (
        state,
        job,
        step,
        attempt,
        inputs,
        captured,
        std::sync::Mutex::new(output_drafts),
    )
}

#[tokio::test]
async fn pause_retains_the_exact_candidate_without_replacing_the_application_baseline() {
    let (state, job, step, attempt, inputs, baseline, _) = fixing_publication_fixture();
    let directory = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(directory.path())
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(directory.path().join("file.txt"), "paused edits\n").unwrap();
    let candidate = crate::workflows::artefacts::CandidateCapture::capture_host(
        directory.path(),
        &state.workflow_artefacts,
    )
    .unwrap();
    assert_ne!(candidate.candidate_hash, baseline.candidate_hash);
    let candidate = crate::workflows::artefacts::CandidatePayload::Revision(candidate);
    let budget = crate::execution::BudgetSnapshot {
        model_requests: 1,
        model_request_limit: 1,
        tool_dispatches: 1,
        tool_dispatch_limit: 10,
        elapsed_ms: 1,
        elapsed_limit_ms: 60_000,
        reason: crate::execution::BudgetReason::ModelRequests,
    };
    super::finalise_attempt(
        &state,
        &job,
        &step,
        attempt,
        &inputs,
        Some(&candidate),
        &StepOutcome::Paused {
            budget,
            reply: Box::default(),
        },
        false,
    )
    .await
    .unwrap();
    let checkpoint = crate::conversations::CheckpointId::generate().unwrap();
    state
        .workflow_runs
        .mutate(&job.run_id, |run| {
            run.pause_attempt(attempt, checkpoint, Vec::new(), 3)?;
            run.resume_paused(attempt, checkpoint, 4)
        })
        .unwrap();
    let resumed = super::load_candidate_input(&state, &job, &inputs).unwrap();
    assert_eq!(resumed.artefact, candidate);
    let run = state.workflow_runs.get(&job.run_id).unwrap();
    assert_eq!(run.attempts[0].inputs, inputs);
    assert!(run.attempts[0].outputs.is_empty());
    assert!(run.attempts[0].result.is_none());
}

#[tokio::test]
async fn a_resumed_phase_keeps_its_tool_results_and_excludes_conversation_history() {
    let (mut state, mut job, step, attempt, _, _, drafts) = fixing_publication_fixture();
    job.connection = crate::providers::ProviderConnection::with_key(
        crate::providers::ProviderKind::Xai,
        "test-private-context-credential",
        "model",
    );
    let backend = crate::tests::ScriptedBackend::accept();
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend.clone()));
    let retained = crate::providers::ChatTurn {
        role: crate::providers::Role::Assistant,
        text: "The command already ran.".to_owned(),
        images: Vec::new(),
        thinking: String::new(),
        tools: Vec::new(),
        activity: Vec::new(),
        usage: Vec::new(),
        calls: vec![crate::providers::ChatToolCall {
            id: "completed-call".to_owned(),
            name: "list".to_owned(),
            arguments: serde_json::json!({"path": "/workspace/project"}),
            result: Some(crate::providers::ToolOutput {
                resource: None,
                label: "list".to_owned(),
                output: "file.txt".to_owned(),
                command: None,
            }),
        }],
        continuation: Vec::new(),
    };
    crate::workflows::AttemptEvidenceContext::new(
        state.workflow_evidence.clone(),
        job.run_id,
        attempt,
        step.key.as_str(),
    )
    .turn(&retained)
    .unwrap();
    let checkpoint = crate::conversations::CheckpointId::generate().unwrap();
    state
        .workflow_runs
        .mutate(&job.run_id, |run| {
            run.launch_brief = "Continue the assigned phase.".to_owned();
            run.pause_attempt(attempt, checkpoint, Vec::new(), 3)?;
            run.resume_paused(attempt, checkpoint, 4)
        })
        .unwrap();
    job.turns = vec![crate::providers::ChatTurn::user(
        "Excluded conversation text".to_owned(),
    )];
    let sandbox = state.sandboxes.attempt_handle(job.run_id, attempt);
    let directory = tempfile::tempdir().unwrap();
    job.host_policy = DirectoryPolicy::from_grants(
        vec![PolicyGrant {
            alias: "project".to_owned(),
            guest_path: GUEST_PROJECT.to_owned(),
            host_path: directory.path().to_owned(),
            access: AccessMode::ReadWrite,
        }],
        "project".to_owned(),
    );
    sandbox
        .start_from_snapshot(
            std::path::Path::new("snapshot"),
            "sha256:deadbeef",
            crate::sandbox::SandboxSpec {
                mounts: vec![crate::sandbox::MountSpec {
                    guest: GUEST_PROJECT.to_owned(),
                    host: directory.path().to_owned(),
                    read_only: true,
                }],
                workdir: GUEST_PROJECT.to_owned(),
                network: crate::agents::NetworkAccess::None,
            },
        )
        .await
        .unwrap();
    let crate::workflows::definition::StepAction::Agent(action) = &step.action else {
        panic!("agent phase")
    };
    let outcome = super::run_agent_step(
        &state,
        &job,
        action,
        Some(&sandbox),
        std::sync::Arc::new(drafts),
    )
    .await;
    if let StepOutcome::Failed { error, .. } = &outcome {
        panic!("resume failed: {error:?}");
    }
    assert!(matches!(outcome, StepOutcome::Completed));
    let history = backend.last_history();
    assert_eq!(
        history
            .iter()
            .filter(|turn| turn.calls.iter().any(|call| call.id == "completed-call"))
            .count(),
        1
    );
    assert_eq!(history.last(), Some(&retained));
    assert!(
        !history
            .iter()
            .any(|turn| turn.text.contains("Excluded conversation text"))
    );
    sandbox.stop().await.unwrap();
    sandbox.remove().await.unwrap();
}

#[tokio::test]
async fn a_configured_phase_uses_its_pinned_provider_model() {
    let (mut state, mut job, step, attempt, _, _, drafts) = fixing_publication_fixture();
    state
        .vault
        .put(crate::providers::ProviderConnection::with_key(
            crate::providers::ProviderKind::Xai,
            "phase-key",
            "grok-4.6",
        ))
        .expect("provider");
    job.connection = crate::providers::ProviderConnection::with_key(
        crate::providers::ProviderKind::Deepseek,
        "normal-key",
        "deepseek-chat",
    );
    let backend = crate::tests::ScriptedBackend::chunks([
        Ok("The credential is phase-".to_owned()),
        Ok("key.".to_owned()),
    ]);
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend.clone()));
    let selection = crate::providers::ModelSelection::new(
        crate::providers::ProviderKind::Xai,
        "grok-4.6".to_owned(),
        state
            .models_dev
            .effective_effort(crate::providers::ProviderKind::Xai, "grok-4.6", None),
    )
    .expect("selection");
    state
        .workflow_runs
        .mutate(&job.run_id, |run| {
            run.launch_brief = "Inspect this candidate.".to_owned();
            run.phase_models = vec![crate::workflows::PhaseModelSelection {
                step: step.key.clone(),
                selection,
                instructions: "Pinned phase instructions".to_owned(),
                preset: None,
                settings: None,
            }];
            Ok(())
        })
        .expect("phase model");
    let directory = tempfile::tempdir().expect("project");
    job.host_policy = DirectoryPolicy::from_grants(
        vec![PolicyGrant {
            alias: "project".to_owned(),
            guest_path: GUEST_PROJECT.to_owned(),
            host_path: directory.path().to_path_buf(),
            access: AccessMode::ReadWrite,
        }],
        "project".to_owned(),
    );
    let sandbox = state.sandboxes.attempt_handle(job.run_id, attempt);
    sandbox
        .start_from_snapshot(
            std::path::Path::new("snapshot"),
            "sha256:deadbeef",
            crate::sandbox::SandboxSpec {
                mounts: vec![crate::sandbox::MountSpec {
                    guest: GUEST_PROJECT.to_owned(),
                    host: directory.path().to_path_buf(),
                    read_only: true,
                }],
                workdir: GUEST_PROJECT.to_owned(),
                network: crate::agents::NetworkAccess::None,
            },
        )
        .await
        .expect("sandbox");
    let crate::workflows::definition::StepAction::Agent(action) = &step.action else {
        panic!("agent phase")
    };
    let drafts = std::sync::Arc::new(drafts);
    let outcome = super::run_agent_step(&state, &job, action, Some(&sandbox), drafts.clone()).await;
    if let StepOutcome::Failed { error, .. } = &outcome {
        panic!("phase failed: {error:?}");
    }
    assert!(matches!(outcome, StepOutcome::Completed));
    assert_eq!(
        backend.last_connection(),
        Some((
            crate::providers::ProviderKind::Xai,
            "grok-4.6".to_owned(),
            state.models_dev.effective_effort(
                crate::providers::ProviderKind::Xai,
                "grok-4.6",
                None
            )
        ))
    );
    assert!(
        backend
            .last_preamble()
            .expect("preamble")
            .contains("Pinned phase instructions")
    );
    assert_eq!(
        job.job.snapshot().output.text,
        "The credential is [redacted]."
    );
    assert_eq!(
        job.connection.kind,
        crate::providers::ProviderKind::Deepseek
    );
    super::set_active_connection(&job, None);
    assert_eq!(
        job.active_connection().kind,
        crate::providers::ProviderKind::Xai
    );

    state
        .vault
        .forget(crate::providers::ProviderKind::Xai)
        .expect("forget");
    let unavailable_backend = crate::tests::ScriptedBackend::accept();
    state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(
        unavailable_backend.clone(),
    ));
    let outcome = super::run_agent_step(&state, &job, action, Some(&sandbox), drafts).await;
    assert!(matches!(
        outcome,
        StepOutcome::Failed {
            category: super::FailureCategory::Provider,
            ..
        }
    ));
    assert!(unavailable_backend.last_connection().is_none());
    sandbox.stop().await.expect("stop");
    sandbox.remove().await.expect("remove");
}

#[test]
fn interruption_failure_restores_current_and_unprocessed_jobs() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let provider = crate::providers::ProviderKind::Xai;
    let session = crate::sessions::generate_session_token()
        .expect("session")
        .id();
    let mut runs = (0..3)
        .map(|_| {
            let definition = crate::tests::test_named_definition("Interrupt");
            let environments = crate::tests::test_environment_set(&definition);
            crate::workflows::WorkflowRun::configured(
                crate::workflows::RunId::generate().expect("run"),
                1,
                AgentId::generate().expect("agent"),
                crate::workflows::definition::PinnedWorkflowDefinition::pin(None, definition),
                environments,
            )
        })
        .collect::<Vec<_>>();
    runs.sort_by_key(|run| run.id);
    let mut all_runs = Vec::new();
    for (index, mut run) in runs.into_iter().enumerate() {
        run.state = if index == 1 {
            crate::workflows::run::RunState::Completed
        } else {
            crate::workflows::run::RunState::InitialisingSource
        };
        let run_id = run.id;
        all_runs.push(run_id);
        state.workflow_runs.create(run).expect("store run");
        let job = crate::workflows::WorkflowJob {
            run_id,
            session_id: session,
            agent_id: Some(AgentId::generate().expect("agent")),
            agent_revision: 1,
            conversation_id: None,
            authority: None,
            project_free_authority: None,
            grant_alias: "project".to_owned(),
            connection: crate::providers::ProviderConnection::with_key(provider, "key", "model"),
            phase_providers: Vec::new(),
            active_connection: std::sync::Arc::new(std::sync::Mutex::new(None)),
            host_policy: DirectoryPolicy::from_grants(Vec::new(), "project".to_owned()),
            turns: Vec::new(),
            job: crate::sessions::Job::new(
                crate::sessions::JobId::generate().expect("job"),
                run_id,
                0,
            ),
            eligible_reply: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
        };
        assert!(state.gate_continuations.insert(job));
    }
    assert!(super::interrupt_provider_continuations(&state, provider).is_err());

    assert!(!state.gate_continuations.available(&all_runs[0], &session));
    assert!(state.gate_continuations.available(&all_runs[1], &session));
    assert!(state.gate_continuations.available(&all_runs[2], &session));
}

#[test]
fn attempt_spec_mounts_isolated_source_and_read_only_git() {
    for access in [AccessMode::ReadOnly, AccessMode::ReadWrite] {
        let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
        let run = crate::workflows::RunId::generate().expect("run");
        let attempt = crate::workflows::AttemptId::generate().expect("attempt");
        let workspace = state
            .workflow_workspaces
            .create_attempt(run, attempt)
            .expect("workspace");
        let project = tempfile::tempdir().expect("project");
        std::fs::create_dir(project.path().join(".git")).expect("git");
        let secondary = tempfile::tempdir().expect("secondary");
        let host = DirectoryPolicy::from_grants(
            vec![
                PolicyGrant {
                    alias: "project".to_owned(),
                    guest_path: GUEST_PROJECT.to_owned(),
                    host_path: project.path().to_path_buf(),
                    access: AccessMode::ReadWrite,
                },
                PolicyGrant {
                    alias: "docs".to_owned(),
                    guest_path: "/access/docs".to_owned(),
                    host_path: secondary.path().to_path_buf(),
                    access: AccessMode::ReadOnly,
                },
            ],
            "project".to_owned(),
        );
        let mut capabilities = crate::tests::test_agent_capabilities();
        capabilities.directories[0].access = access;
        capabilities.directories.push(CapabilityDirectory {
            alias: "docs".to_owned(),
            guest_path: "/access/docs".to_owned(),
            access: AccessMode::ReadOnly,
            role: DirectoryRole::SecondaryContext,
        });

        assert!(!workspace.project.join(".git").exists());
        let spec =
            attempt_spec(&capabilities, &workspace, project.path(), &host, None).expect("spec");

        assert!(workspace.project.join(".git").is_dir());
        assert_eq!(
            std::fs::read_dir(workspace.project.join(".git"))
                .expect("mount directory")
                .count(),
            0
        );
        assert_eq!(spec.workdir, GUEST_PROJECT);
        assert_eq!(spec.mounts[0].host, workspace.project);
        assert_eq!(spec.mounts[0].read_only, !access.is_writable());
        assert_eq!(spec.mounts[1].guest, "/project/.git");
        assert_eq!(spec.mounts[1].host, project.path().join(".git"));
        assert!(spec.mounts[1].read_only);
        assert_eq!(spec.mounts[2].guest, "/access/docs");
        assert_eq!(spec.mounts[2].host, secondary.path());
        assert!(spec.mounts[2].read_only);
        std::fs::remove_dir(workspace.project.join(".git")).expect("remove placeholder");
        std::os::unix::fs::symlink(project.path().join(".git"), workspace.project.join(".git"))
            .expect("symlink");
        assert!(attempt_spec(&capabilities, &workspace, project.path(), &host, None).is_err());
        workspace.destroy().expect("destroy");
    }
}

#[test]
fn failed_conversation_workflow_retains_a_secret_safe_error() {
    let (state, mut workflow, _, _) = gate_ready_fixture(
        crate::workflows::RunKind::QuickTask,
        GateCandidate::Unchanged,
    );
    let conversation = state
        .conversations
        .create("Failure".to_owned())
        .expect("conversation");
    let session = crate::sessions::generate_session_token()
        .expect("session")
        .id();
    state.sessions.insert(session);
    workflow.session_id = session;
    workflow.job = state
        .sessions
        .begin_conversation_job(&session, conversation.id)
        .expect("job");
    let job_id = workflow.job.id();
    state
        .conversations
        .begin_message_with_model(
            &conversation.id,
            conversation.revision,
            None,
            job_id,
            "Read the project".to_owned(),
        )
        .expect("message");
    workflow.conversation_id = Some(conversation.id);
    super::set_active_connection(
        &workflow,
        Some(crate::providers::ProviderConnection::with_key(
            crate::providers::ProviderKind::Xai,
            "phase-secret",
            "model",
        )),
    );
    super::settle_job(
        &state,
        &workflow,
        JobStatus::Failed,
        Some("Sandbox failed: phase-secret\0"),
    );

    let saved = state
        .conversations
        .get(&conversation.id)
        .expect("saved conversation");
    let reply = saved.messages.last().expect("reply");
    assert_eq!(reply.status, crate::conversations::MessageStatus::Failed);
    assert_eq!(reply.error.as_deref(), Some("Sandbox failed: [redacted]"));
    assert_eq!(saved.active_job, None);
    assert!(!state.sessions.busy(&session));
}

#[tokio::test]
async fn partial_start_cleanup_retains_resources_until_the_guest_is_gone() {
    enum Failure {
        None,
        Stop,
        Remove,
    }

    for failure in [Failure::None, Failure::Stop, Failure::Remove] {
        let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
        let run = crate::workflows::RunId::generate().expect("run");
        let attempt = crate::workflows::AttemptId::generate().expect("attempt");
        let workspace = state
            .workflow_workspaces
            .create_attempt(run, attempt)
            .expect("workspace");
        let workspace_path = workspace.root.clone();
        let sandbox = state.sandboxes.attempt_handle(run, attempt);
        let spec = crate::sandbox::SandboxSpec {
            mounts: vec![crate::sandbox::MountSpec {
                guest: GUEST_PROJECT.to_owned(),
                host: workspace.project.clone(),
                read_only: false,
            }],
            workdir: GUEST_PROJECT.to_owned(),
            network: crate::agents::NetworkAccess::None,
        };
        sandbox
            .start_from_snapshot(std::path::Path::new("snapshot"), "sha256:deadbeef", spec)
            .await
            .expect("start");
        match failure {
            Failure::None => {}
            Failure::Stop => sandbox.fail_next_stop(),
            Failure::Remove => sandbox.fail_next_remove(),
        }

        let (outcome, cleanup) = cleanup_after_start_failure(
            &state,
            attempt,
            sandbox,
            workspace,
            StepOutcome::Failed {
                category: crate::workflows::run::FailureCategory::Operational,
                error: None,
            },
        )
        .await;

        match failure {
            Failure::None => {
                assert!(matches!(
                    cleanup,
                    crate::workflows::run::AttemptCleanupRecord::Complete
                ));
                assert!(matches!(
                    outcome,
                    StepOutcome::Failed {
                        category: crate::workflows::run::FailureCategory::Operational,
                        ..
                    }
                ));
                assert!(!workspace_path.exists());
                assert!(!state.sandboxes.guest_named(attempt));
            }
            Failure::Stop | Failure::Remove => {
                assert!(matches!(
                    cleanup,
                    crate::workflows::run::AttemptCleanupRecord::Orphaned {
                        sandbox: true,
                        workspace: true,
                        journal: false,
                    }
                ));
                assert!(matches!(
                    outcome,
                    StepOutcome::Failed {
                        category: crate::workflows::run::FailureCategory::Cleanup,
                        ..
                    }
                ));
                assert!(workspace_path.exists());
                assert!(
                    state
                        .sandboxes
                        .orphans()
                        .iter()
                        .any(|orphan| orphan.name.starts_with("pp-attempt-"))
                );
            }
        }
    }
}

#[test]
fn failed_capture_records_unknown_observed_source() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let definition = crate::tests::test_named_definition("Work");
    let environments = crate::tests::test_environment_set(&definition);
    let run_id = crate::workflows::RunId::generate().expect("run");
    let mut run = crate::workflows::run::WorkflowRun::configured(
        run_id,
        1,
        crate::agents::AgentId::generate().expect("agent"),
        crate::workflows::definition::PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    let artefact_id = crate::workflows::ArtefactId::generate().expect("artefact");
    run.record_initial_candidate(crate::workflows::artefacts::ArtefactRecord {
        id: artefact_id,
        kind: crate::workflows::definition::ArtefactKind::CandidateRevision,
        artefact_hash: crate::workflows::artefacts::ArtefactHash::of(b"test", b"payload"),
        object_hash: crate::workflows::artefacts::ObjectHash::of(b"payload"),
        payload_bytes: 7,
        created_at_ms: 1,
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id,
            producer: crate::workflows::artefacts::ArtefactProducer::RunSourceCapture,
            inputs: Vec::new(),
        },
        summary: crate::workflows::artefacts::ArtefactSummary::Candidate {
            candidate: crate::workflows::artefacts::CandidateHash::of(b"tree"),
            entries: 0,
            bytes: 0,
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
    })
    .expect("source");
    let attempt = crate::workflows::AttemptId::generate().expect("attempt");
    let sandbox = crate::workflows::run::AttemptSandboxRecord {
        kind: crate::workflows::run::AttemptSandboxKind::IsolatedAttempt,
        snapshot_digest: run.environments.steps[0].snapshot_digest.clone(),
    };
    run.start_attempt(
        attempt,
        Vec::new(),
        crate::tests::test_agent_capabilities(),
        sandbox,
        2,
    )
    .expect("start");
    state.workflow_runs.create(run).expect("store");

    record_unknown_observed(&state, &run_id, attempt).expect("unknown");

    let loaded = state.workflow_runs.get(&run_id).expect("run");
    let crate::workflows::run::RunSource::Captured { source } = loaded.source else {
        panic!("expected captured source");
    };
    assert_eq!(
        source.observed,
        crate::workflows::run::ObservedCandidate::Unknown
    );
}

#[test]
fn unregistered_command_text_cannot_become_a_system_command() {
    assert!(SystemCommandId::parse("rm -rf /").is_none());
    assert!(SystemCommandId::parse("git status --porcelain=v1").is_none());
    assert_eq!(
        SystemCommandId::parse("repository-status"),
        Some(SystemCommandId::RepositoryStatus)
    );
}

fn resolver_reference(
    kind: crate::workflows::definition::ArtefactKind,
    marker: &[u8],
) -> crate::workflows::artefacts::ArtefactReference {
    crate::workflows::artefacts::ArtefactReference {
        id: crate::workflows::ArtefactId::generate().expect("artefact"),
        kind,
        artefact_hash: crate::workflows::artefacts::ArtefactHash::of(
            marker,
            kind.as_str().as_bytes(),
        ),
    }
}

fn completed_output_attempt(
    step: &str,
    ordinal: u32,
    output: &str,
    artefact: crate::workflows::artefacts::ArtefactReference,
) -> crate::workflows::run::AttemptRecord {
    crate::workflows::run::AttemptRecord {
        id: crate::workflows::AttemptId::generate().expect("attempt"),
        step: crate::workflows::definition::StepKey::parse(step).expect("step"),
        ordinal,
        action_kind: crate::workflows::run::ActionKind::Agent,
        started_at_ms: u64::from(ordinal),
        finished_at_ms: Some(u64::from(ordinal) + 1),
        state: crate::workflows::run::AttemptState::Completed,
        result: Some(crate::workflows::run::AttemptResult::Completed {
            outputs: vec![output.to_owned()],
        }),
        review_route: None,
        inputs: Vec::new(),
        outputs: vec![crate::workflows::run::AttemptArtefactOutput {
            key: crate::workflows::definition::OutputKey::parse(output).expect("output"),
            artefact,
        }],
        capabilities: crate::tests::test_agent_capabilities(),
        sandbox: crate::workflows::run::AttemptSandboxRecord {
            kind: crate::workflows::run::AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: crate::environments::SnapshotDigest::parse(&format!(
                "sha256:{}",
                "a".repeat(64)
            ))
            .expect("digest"),
        },
        cleanup: crate::workflows::run::AttemptCleanupRecord::Complete,
        initial_context: None,
        apply_transaction: None,
        commit_transaction: None,
        direct_changes: None,
        continuation: None,
        paused_drafts: Vec::new(),
        paused_candidate: None,
    }
}

#[test]
fn step_output_resolution_uses_the_latest_completed_producer_attempt() {
    use crate::workflows::definition::ArtefactKind;

    let definition =
        crate::workflows::seeds::sequential_team_definition(crate::tests::test_environment_id());
    let environments = crate::tests::test_environment_set(&definition);
    let mut run = crate::workflows::WorkflowRun::configured(
        crate::workflows::RunId::generate().expect("run"),
        1,
        AgentId::generate().expect("agent"),
        crate::workflows::definition::PinnedWorkflowDefinition::pin(None, definition),
        environments,
    );
    let old_candidate = resolver_reference(ArtefactKind::CandidateRevision, b"old candidate");
    let current_candidate =
        resolver_reference(ArtefactKind::CandidateRevision, b"current candidate");
    let incomplete_candidate =
        resolver_reference(ArtefactKind::CandidateRevision, b"incomplete candidate");
    let old_review = resolver_reference(ArtefactKind::ReviewReport, b"old review");
    let current_review = resolver_reference(ArtefactKind::ReviewReport, b"current review");
    run.attempts = vec![
        completed_output_attempt("implementer", 1, "candidate", old_candidate),
        completed_output_attempt("reviewer", 1, "review", old_review),
        completed_output_attempt("implementer", 2, "candidate", current_candidate.clone()),
        completed_output_attempt("reviewer", 2, "review", current_review.clone()),
    ];
    let mut incomplete =
        completed_output_attempt("implementer", 3, "candidate", incomplete_candidate);
    incomplete.state = crate::workflows::run::AttemptState::Active;
    incomplete.finished_at_ms = None;
    incomplete.result = None;
    run.attempts.push(incomplete);
    let commit = run
        .pinned
        .definition
        .step(&crate::workflows::definition::StepKey::parse("commit").expect("step"))
        .expect("commit");

    let inputs = super::resolve_inputs(&run, commit).expect("resolve inputs");
    assert_eq!(inputs[0].artefact, current_candidate);
    assert_eq!(inputs[1].artefact, current_review);
}

fn candidate_record(
    run: &crate::workflows::WorkflowRun,
    candidate: &crate::workflows::artefacts::candidate::CandidateRevisionArtefact,
    store: &crate::workflows::WorkflowArtefactRepository,
    initial: bool,
) -> crate::workflows::artefacts::ArtefactRecord {
    let bytes = candidate.manifest_bytes().expect("manifest");
    let object = store.publish(&bytes).expect("publish candidate");
    let id = crate::workflows::ArtefactId::generate().expect("artefact");
    crate::workflows::artefacts::ArtefactRecord {
        id,
        kind: crate::workflows::definition::ArtefactKind::CandidateRevision,
        artefact_hash: crate::workflows::artefacts::artefact_hash_for(
            crate::workflows::definition::ArtefactKind::CandidateRevision,
            candidate.format_version,
            &bytes,
        ),
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: 1,
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id: run.id,
            producer: if initial {
                crate::workflows::artefacts::ArtefactProducer::RunSourceCapture
            } else {
                crate::workflows::artefacts::ArtefactProducer::StepAttempt {
                    attempt_id: crate::workflows::AttemptId::generate().expect("producer"),
                    step: crate::workflows::definition::StepKey::parse("implementer")
                        .expect("step"),
                    output: Some(
                        crate::workflows::definition::OutputKey::parse("candidate")
                            .expect("output"),
                    ),
                    disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
                }
            },
            inputs: Vec::new(),
        },
        summary: crate::workflows::artefacts::ArtefactSummary::Candidate {
            candidate: candidate.candidate_hash,
            entries: candidate.entries.len() as u64,
            bytes: 0,
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
    }
}

fn git_worktree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("dir");
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .expect("git")
            .success()
    );
    dir
}

fn published_gate_candidate(
    run: &crate::workflows::WorkflowRun,
    captured: &crate::workflows::artefacts::candidate::CandidateRevisionArtefact,
    store: &crate::workflows::WorkflowArtefactRepository,
    producer: crate::workflows::artefacts::ArtefactProducer,
    inputs: Vec<crate::workflows::artefacts::ArtefactReference>,
) -> crate::workflows::artefacts::ArtefactRecord {
    let bytes = captured.manifest_bytes().expect("manifest");
    let object = store.publish(&bytes).expect("publish");
    crate::workflows::artefacts::ArtefactRecord {
        id: crate::workflows::ArtefactId::generate().expect("artefact"),
        kind: crate::workflows::definition::ArtefactKind::CandidateRevision,
        artefact_hash: crate::workflows::artefacts::artefact_hash_for(
            crate::workflows::definition::ArtefactKind::CandidateRevision,
            captured.format_version,
            &bytes,
        ),
        object_hash: object,
        payload_bytes: bytes.len() as u64,
        created_at_ms: 1,
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id: run.id,
            producer,
            inputs,
        },
        summary: crate::workflows::artefacts::ArtefactSummary::Candidate {
            candidate: captured.candidate_hash,
            entries: captured.entries.len() as u64,
            bytes: 0,
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
    }
}

enum GateCandidate {
    Unchanged,
    Changed,
}

fn gate_ready_fixture(
    kind: crate::workflows::RunKind,
    candidate: GateCandidate,
) -> (
    crate::state::AppState,
    crate::workflows::WorkflowJob,
    crate::sessions::SessionId,
    crate::sessions::ConversationKey,
) {
    use crate::workflows::definition::{InputKey, OutputKey, StepKey};

    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let project = git_worktree();
    std::fs::write(project.path().join("file.txt"), b"candidate\n").expect("source");
    let initial_capture = crate::workflows::artefacts::CandidateCapture::capture_host(
        project.path(),
        &state.workflow_artefacts,
    )
    .expect("initial capture");
    let produced_capture = match candidate {
        GateCandidate::Unchanged => initial_capture.clone(),
        GateCandidate::Changed => {
            std::fs::write(project.path().join("file.txt"), b"changed\n").expect("change");
            crate::workflows::artefacts::CandidateCapture::capture_host(
                project.path(),
                &state.workflow_artefacts,
            )
            .expect("changed capture")
        }
    };
    let pinned = crate::workflows::pin_quick_task(
        AccessMode::ReadWrite,
        &[crate::agents::ToolId::List],
        "Do the work.",
        crate::tests::test_environment_id(),
    )
    .expect("quick task");
    let environments = crate::tests::test_environment_set(&pinned.definition);
    let mut run = crate::workflows::WorkflowRun::create(
        crate::workflows::RunId::generate().expect("run"),
        1,
        Some(AgentId::generate().expect("agent")),
        kind,
        pinned,
        environments,
    );
    let initial = published_gate_candidate(
        &run,
        &initial_capture,
        &state.workflow_artefacts,
        crate::workflows::artefacts::ArtefactProducer::RunSourceCapture,
        Vec::new(),
    );
    let initial_ref = crate::workflows::artefacts::ArtefactReference {
        id: initial.id,
        kind: initial.kind,
        artefact_hash: initial.artefact_hash,
    };
    run.record_initial_candidate(initial).expect("initial");
    let work = StepKey::parse("work").expect("work");
    let attempt = crate::workflows::AttemptId::generate().expect("attempt");
    run.start_attempt(
        attempt,
        vec![crate::workflows::run::AttemptArtefactInput {
            key: InputKey::parse("candidate").expect("input"),
            artefact: initial_ref.clone(),
        }],
        crate::tests::test_agent_capabilities(),
        crate::workflows::run::AttemptSandboxRecord {
            kind: crate::workflows::run::AttemptSandboxKind::IsolatedAttempt,
            snapshot_digest: run
                .environments
                .steps
                .iter()
                .find(|binding| binding.step == work)
                .expect("work environment")
                .snapshot_digest
                .clone(),
        },
        2,
    )
    .expect("start");
    let produced = published_gate_candidate(
        &run,
        &produced_capture,
        &state.workflow_artefacts,
        crate::workflows::artefacts::ArtefactProducer::StepAttempt {
            attempt_id: attempt,
            step: work.clone(),
            output: Some(OutputKey::parse("candidate").expect("output")),
            disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
        },
        vec![initial_ref],
    );
    let produced_ref = crate::workflows::artefacts::ArtefactReference {
        id: produced.id,
        kind: produced.kind,
        artefact_hash: produced.artefact_hash,
    };
    run.record_attempt_outputs(
        attempt,
        vec![produced],
        vec![crate::workflows::run::AttemptArtefactOutput {
            key: OutputKey::parse("candidate").expect("output"),
            artefact: produced_ref.clone(),
        }],
        Some(produced_ref.clone()),
        crate::workflows::run::ObservedCandidate::Exact {
            artefact: produced_ref,
        },
    )
    .expect("outputs");
    run.record_cleanup(
        attempt,
        crate::workflows::run::AttemptCleanupRecord::Complete,
    )
    .expect("cleanup");
    run.complete_attempt(attempt, 3).expect("complete work");
    let run_id = run.id;
    let agent_id = run.agent_id.expect("agent");
    state.workflow_runs.create(run).expect("store run");
    state.keep_temp_dir(project);
    let token = crate::sessions::generate_session_token().expect("token");
    let session_id = token.id();
    let key = crate::sessions::ConversationKey { agent_id };
    state.sessions.insert(session_id);
    let begun = state
        .sessions
        .begin_turn(&session_id, key, run_id, "Hello".to_owned())
        .expect("turn");
    let job = crate::workflows::WorkflowJob {
        run_id,
        session_id,
        agent_id: Some(agent_id),
        agent_revision: 1,
        conversation_id: None,
        authority: None,
        project_free_authority: None,
        grant_alias: "project".to_owned(),
        connection: crate::providers::ProviderConnection::with_key(
            crate::providers::ProviderKind::Xai,
            "key",
            "model",
        ),
        phase_providers: Vec::new(),
        active_connection: std::sync::Arc::new(std::sync::Mutex::new(None)),
        host_policy: DirectoryPolicy::from_grants(Vec::new(), "project".to_owned()),
        turns: begun.turns,
        job: begun.job,
        eligible_reply: std::sync::Arc::new(std::sync::Mutex::new("No files changed.".to_owned())),
    };
    (state, job, session_id, key)
}

async fn execute_gate_run(state: crate::state::AppState, job: crate::workflows::WorkflowJob) {
    let lease = state
        .agent_leases
        .acquire(job.agent_id.expect("agent"))
        .expect("lease");
    let execution = state.workflow_execution.acquire().expect("execution");
    super::execute_run(state, job, Some(lease), execution).await;
}

#[test]
fn uncertain_commit_protection_is_not_a_resumable_gate() {
    let (state, workflow, session, _) =
        gate_ready_fixture(crate::workflows::RunKind::QuickTask, GateCandidate::Changed);
    let agent = state
        .agent_leases
        .acquire(workflow.agent_id.expect("agent"))
        .expect("agent lease");
    let execution = state.workflow_execution.acquire().expect("execution lease");
    let second_execution = state
        .workflow_execution
        .acquire()
        .expect("second execution");
    let third_execution = state.workflow_execution.acquire().expect("third execution");
    let second_agent_id = crate::agents::AgentId::generate().expect("second agent");
    let second_agent = state
        .agent_leases
        .acquire(second_agent_id)
        .expect("second agent lease");
    state
        .gate_continuations
        .protect_commit_recovery(&workflow.job, Some(agent), execution);
    state.gate_continuations.protect_apply_recovery(
        &workflow.job,
        Some(second_agent),
        second_execution,
    );
    state
        .gate_continuations
        .protect_cleanup_failure(&workflow.job, None, third_execution);

    assert!(state.workflow_execution.acquire().is_err());
    assert!(state.agent_leases.acquire(second_agent_id).is_err());
    assert!(state.workflow_execution.acquire_exclusive().is_err());
    assert!(
        state
            .agent_leases
            .acquire(workflow.agent_id.expect("agent"))
            .is_err()
    );
    assert!(state.sessions.busy(&session));
    assert!(
        !state
            .gate_continuations
            .available(&workflow.run_id, &session)
    );
    super::interrupt_provider_continuations(&state, workflow.connection.kind)
        .expect("forget provider");
    super::interrupt_session_continuations(&state, session).expect("expire session");
    assert!(state.workflow_execution.acquire().is_err());
    assert!(state.agent_leases.acquire(second_agent_id).is_err());
    assert!(state.workflow_execution.acquire_exclusive().is_err());
    assert!(
        state
            .agent_leases
            .acquire(workflow.agent_id.expect("agent"))
            .is_err()
    );
}

#[tokio::test]
async fn execute_run_completes_an_unchanged_quick_task_without_a_gate() {
    let (state, job, session_id, key) = gate_ready_fixture(
        crate::workflows::RunKind::QuickTask,
        GateCandidate::Unchanged,
    );
    let run_id = job.run_id;
    let job_id = job.job.id();
    execute_gate_run(state.clone(), job).await;
    let run = state.workflow_runs.get(&run_id).expect("run");
    assert_eq!(run.state, crate::workflows::run::RunState::Completed);
    assert!(run.gates.is_empty());
    assert!(
        run.attempts
            .iter()
            .all(|attempt| attempt.step.as_str() != "commit")
    );
    assert!(run.artefacts.iter().all(|artefact| {
        artefact.kind != crate::workflows::definition::ArtefactKind::HumanDecision
    }));
    assert!(!state.gate_continuations.available(&run_id, &session_id));
    let snapshot = state.sessions.snapshot(&session_id, &key).expect("session");
    assert!(!snapshot.session_busy);
    assert_eq!(
        snapshot.turns.last().map(|turn| turn.text.as_str()),
        Some("No files changed.")
    );
    let job = state.sessions.job(&session_id, &key, &job_id).expect("job");
    assert_eq!(job.snapshot().status, JobStatus::Completed);
}

#[tokio::test]
async fn a_store_failure_does_not_open_a_gate_for_an_unchanged_quick_task() {
    let (state, job, session_id, key) = gate_ready_fixture(
        crate::workflows::RunKind::QuickTask,
        GateCandidate::Unchanged,
    );
    let run_id = job.run_id;
    let job_id = job.job.id();
    state.workflow_runs.fail_next_mutation();
    execute_gate_run(state.clone(), job).await;
    let run = state.workflow_runs.get(&run_id).expect("run");
    assert!(matches!(
        run.state,
        crate::workflows::run::RunState::Ready { .. }
    ));
    assert!(run.gates.is_empty());
    assert!(!state.gate_continuations.available(&run_id, &session_id));
    let snapshot = state.sessions.snapshot(&session_id, &key).expect("session");
    assert!(!snapshot.session_busy);
    let job = state.sessions.job(&session_id, &key, &job_id).expect("job");
    assert_eq!(job.snapshot().status, JobStatus::Failed);
}

#[tokio::test]
async fn execute_run_opens_a_gate_for_a_changed_quick_task() {
    let (state, job, session_id, key) =
        gate_ready_fixture(crate::workflows::RunKind::QuickTask, GateCandidate::Changed);
    let run_id = job.run_id;
    execute_gate_run(state.clone(), job).await;
    let run = state.workflow_runs.get(&run_id).expect("run");
    assert!(matches!(
        run.state,
        crate::workflows::run::RunState::AwaitingHuman { .. }
    ));
    assert_eq!(run.gates.len(), 1);
    assert!(state.gate_continuations.available(&run_id, &session_id));
    let snapshot = state.sessions.snapshot(&session_id, &key).expect("session");
    assert!(snapshot.session_busy);
    assert_eq!(
        snapshot.job.map(|job| job.status),
        Some(JobStatus::AwaitingDecision)
    );
}

#[tokio::test]
async fn execute_run_opens_a_gate_for_an_unchanged_configured_candidate() {
    let (state, job, session_id, key) = gate_ready_fixture(
        crate::workflows::RunKind::Configured,
        GateCandidate::Unchanged,
    );
    let run_id = job.run_id;
    execute_gate_run(state.clone(), job).await;
    let run = state.workflow_runs.get(&run_id).expect("run");
    assert!(matches!(
        run.state,
        crate::workflows::run::RunState::AwaitingHuman { .. }
    ));
    assert_eq!(run.gates.len(), 1);
    assert!(state.gate_continuations.available(&run_id, &session_id));
    let snapshot = state.sessions.snapshot(&session_id, &key).expect("session");
    assert!(snapshot.session_busy);
    assert_eq!(
        snapshot.job.map(|job| job.status),
        Some(JobStatus::AwaitingDecision)
    );
}
