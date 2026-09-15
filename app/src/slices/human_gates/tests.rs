#[tokio::test]
async fn conversation_workspace_keeps_exact_gate_fields_in_each_representation() {
    let fixture = conversation_awaiting_gate();
    let path = format!("/conversations/{}", fixture.conversation_id.unwrap());
    for graft in [None, Some("navigation"), Some("patch")] {
        let mut request = Request::builder()
            .uri(&path)
            .header(header::COOKIE, cookie(&fixture.token));
        if let Some(kind) = graft {
            request = request
                .header("Graft-Request", kind)
                .header(header::ACCEPT, "text/vnd.hypergraft.patches+html");
        }
        let response = app(&fixture.state)
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        for action in ["approve", "cancel", "request-revision"] {
            let action = format!("action=\"{}/{action}\"", fixture.gate_path());
            let form = body
                .split("<form")
                .find(|form| form.split("</form>").next().unwrap().contains(&action))
                .expect("gate command");
            let form = form.split("</form>").next().unwrap();
            assert!(form.contains("name=\"gate-revision\""));
            assert!(form.contains("value=\"1\""));
            assert!(form.contains("name=\"candidate\""));
            assert!(form.contains(&format!("value=\"{}\"", fixture.candidate)));
            assert!(form.contains("name=\"surface\" value=\"conversation\""));
        }
        assert!(body.contains(fixture.host.to_str().unwrap()));
    }
}

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    middleware::from_fn_with_state,
};
use std::sync::Arc;
use tower::ServiceExt;

use super::forms::{DecisionForm, EnvironmentSwitchDecisionForm, FormError};
use crate::{
    agents::{AccessMode, AgentDraft, DirectoryGrant, DirectoryPolicy, ToolId},
    config::RuntimeConfig,
    providers::{ProviderConnection, ProviderKind},
    sessions,
    state::AppState,
    workflows::{self, RunKind},
};

const HOST_UNCHANGED_SAFETY: &str =
    "The host project is unchanged. Review the candidate before you apply it.";

fn test_state() -> AppState {
    crate::tests::test_state(RuntimeConfig::development())
}

fn app(state: &AppState) -> axum::Router {
    crate::slices::router()
        .layer(from_fn_with_state(
            state.clone(),
            crate::sessions::resolve_session,
        ))
        .layer(axum::middleware::from_fn(hypergraft::middleware::classify))
        .with_state(state.clone())
}

#[test]
fn plan_checkpoint_rejects_code_decisions_and_stale_plan_hashes() {
    use crate::workflows::definition::{ArtefactKind, StepKey};

    let environment = crate::tests::test_environment_id();
    let definition = workflows::seeds::plan_then_implement_definition(environment);
    let pinned = workflows::definition::PinnedWorkflowDefinition::pin(None, definition.clone());
    let project = crate::projects::ProjectId::parse(&"a".repeat(32)).expect("project");
    let agent = crate::agents::AgentId::generate().expect("agent");
    let mut run = workflows::WorkflowRun::create(
        workflows::RunId::generate().expect("run"),
        1,
        project,
        Some(agent),
        RunKind::Configured,
        pinned,
        crate::tests::test_environment_set(&definition),
    );
    run.state = workflows::run::RunState::Ready {
        step: StepKey::parse("plan-acceptance").expect("checkpoint"),
    };
    let store = crate::workflows::WorkflowArtefactRepository::in_memory();
    let (plan_bytes, plan_object, plan_hash) =
        crate::workflows::artefacts::payload::encode_plan("Exact plan", None).expect("plan");
    let plan = crate::workflows::artefacts::ArtefactReference {
        id: crate::workflows::ArtefactId::generate().expect("plan id"),
        kind: ArtefactKind::Plan,
        artefact_hash: plan_hash,
    };
    run.artefacts
        .push(crate::workflows::artefacts::ArtefactRecord {
            id: plan.id,
            kind: plan.kind,
            artefact_hash: plan.artefact_hash,
            object_hash: plan_object,
            payload_bytes: plan_bytes.len() as u64,
            created_at_ms: 2,
            provenance: crate::workflows::artefacts::ArtefactProvenance {
                run_id: run.id,
                producer: crate::workflows::artefacts::ArtefactProducer::StepAttempt {
                    attempt_id: crate::workflows::AttemptId::generate().expect("attempt"),
                    step: StepKey::parse("planner").expect("planner"),
                    output: Some(
                        crate::workflows::definition::OutputKey::parse("plan").expect("output"),
                    ),
                    disposition: crate::workflows::artefacts::ProductionDisposition::RequiredOutput,
                },
                inputs: Vec::new(),
            },
            summary: crate::workflows::artefacts::ArtefactSummary::Plan { markdown_bytes: 10 },
        });
    let gate = run
        .open_plan_gate(
            workflows::GateId::generate().expect("gate"),
            plan.clone(),
            2,
        )
        .expect("plan gate");
    assert!(super::load_gate_plan(&run, &gate, &store).is_none());
    store.publish(&plan_bytes).expect("publish plan");
    assert_eq!(
        super::load_gate_plan(&run, &gate, &store).as_deref(),
        Some("Exact plan")
    );
    let (_bytes, object_hash, artefact_hash) = crate::workflows::artefacts::encode_plan_decision(
        plan.artefact_hash,
        crate::workflows::gates::PlanDecisionKind::Accepted,
        None,
        3,
        None,
    )
    .expect("plan decision");
    let decision = super::plan_decision_record(
        &run,
        &gate,
        crate::workflows::gates::PlanDecisionKind::Accepted,
        3,
        object_hash,
        artefact_hash,
        1,
    )
    .expect("plan decision record");
    let code_decision = crate::workflows::artefacts::ArtefactRecord {
        id: crate::workflows::ArtefactId::generate().expect("code decision id"),
        kind: ArtefactKind::HumanDecision,
        artefact_hash: crate::workflows::artefacts::ArtefactHash::of(b"kind", b"code"),
        object_hash: crate::workflows::artefacts::ObjectHash::of(b"code"),
        payload_bytes: 4,
        created_at_ms: 3,
        provenance: crate::workflows::artefacts::ArtefactProvenance {
            run_id: run.id,
            producer: crate::workflows::artefacts::ArtefactProducer::RunSourceCapture,
            inputs: Vec::new(),
        },
        summary: crate::workflows::artefacts::ArtefactSummary::HumanDecision {
            candidate: crate::workflows::artefacts::CandidateHash::of(b"candidate"),
            diff_base: crate::workflows::artefacts::CandidateHash::of(b"base"),
            decision: crate::workflows::gates::HumanDecisionKind::Approved,
        },
    };
    assert!(
        run.decide_gate(
            gate.id,
            gate.revision,
            code_decision,
            crate::workflows::gates::HumanDecisionKind::Approved,
            None,
            None,
            3,
        )
        .is_err()
    );
    let mut accepted = run.clone();
    accepted
        .decide_plan_gate(
            gate.id,
            gate.revision,
            decision.clone(),
            crate::workflows::gates::PlanDecisionKind::Accepted,
            None,
            None,
            4,
        )
        .expect("accept plan");
    assert!(matches!(
        accepted.state,
        workflows::run::RunState::Ready { .. }
    ));

    run.gates[0].candidate.artefact_hash =
        crate::workflows::artefacts::ArtefactHash::of(b"plan", b"changed plan");
    run.gates[0].diff_base = run.gates[0].candidate.clone();
    assert!(super::load_gate_plan(&run, &run.gates[0], &store).is_none());
    assert!(
        run.decide_plan_gate(
            gate.id,
            gate.revision,
            decision,
            crate::workflows::gates::PlanDecisionKind::Accepted,
            None,
            None,
            4,
        )
        .is_err()
    );
    assert!(matches!(
        run.state,
        workflows::run::RunState::AwaitingHuman { .. }
    ));
}

#[test]
fn decision_forms_reject_duplicate_and_blank_revision_fields() {
    let duplicate = vec![
        ("gate-revision".to_owned(), "1".to_owned()),
        ("gate-revision".to_owned(), "1".to_owned()),
        ("candidate".to_owned(), "sha256:00".to_owned()),
    ];
    assert_eq!(
        DecisionForm::parse(duplicate, false).err(),
        Some(FormError::Invalid)
    );

    let blank_note = vec![
        ("gate-revision".to_owned(), "1".to_owned()),
        ("candidate".to_owned(), "sha256:00".to_owned()),
        ("note".to_owned(), "  ".to_owned()),
    ];
    assert_eq!(
        DecisionForm::parse(blank_note, true).err(),
        Some(FormError::Note)
    );
}

#[test]
fn environment_switch_decisions_reject_missing_and_duplicate_bindings() {
    let environment = crate::tests::test_environment_id();
    let fields = vec![
        ("gate-revision".to_owned(), "1".to_owned()),
        ("candidate".to_owned(), "sha256:00".to_owned()),
        ("conversation-revision".to_owned(), "2".to_owned()),
        ("environment".to_owned(), environment.as_hex()),
        ("surface".to_owned(), "conversation".to_owned()),
    ];
    for required in [
        "gate-revision",
        "candidate",
        "conversation-revision",
        "environment",
    ] {
        let missing = fields
            .iter()
            .filter(|(key, _)| key != required)
            .cloned()
            .collect();
        assert!(matches!(
            EnvironmentSwitchDecisionForm::parse(missing),
            Err(FormError::Invalid)
        ));
    }
    let mut duplicate = fields;
    duplicate.push(("candidate".to_owned(), "sha256:01".to_owned()));
    assert!(matches!(
        EnvironmentSwitchDecisionForm::parse(duplicate),
        Err(FormError::Invalid)
    ));
}

#[tokio::test]
async fn anonymous_gate_requests_redirect_to_connect() {
    let state = test_state();
    let id = "0".repeat(32);
    let detail = format!("/runs/{id}/gates/{id}");
    let approve = format!("/runs/{id}/gates/{id}/approve");
    let cases = [
        ("GET", detail.as_str(), None, false),
        ("GET", detail.as_str(), Some("navigation"), true),
        ("POST", approve.as_str(), None, false),
        ("POST", approve.as_str(), Some("patch"), true),
    ];
    for (method, uri, graft, enhanced) in cases {
        assert_connect_redirect(&state, method, uri, graft, enhanced).await;
    }
}

async fn assert_connect_redirect(
    state: &AppState,
    method: &str,
    uri: &str,
    graft: Option<&str>,
    enhanced: bool,
) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(graft) = graft {
        builder = builder
            .header(hypergraft::GRAFT_REQUEST, graft)
            .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
    }
    let response = app(state)
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("anonymous");
    if method == "POST" && graft.is_none() {
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    } else if enhanced {
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            hypergraft::MEDIA_TYPE
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains("navigate=\"/connect\""),
            "{method} {uri} {graft:?}: {text}"
        );
    } else {
        assert_eq!(response.status(), axum::http::StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/connect"
        );
    }
}

struct GateFixture {
    state: AppState,
    token: String,
    session: sessions::SessionId,
    key: sessions::ConversationKey,
    run_id: workflows::RunId,
    gate_id: workflows::GateId,
    conversation_id: Option<crate::conversations::ConversationId>,
    project_id: crate::projects::ProjectId,
    agent_id: crate::agents::AgentId,
    candidate: String,
    candidate_id: crate::workflows::ArtefactId,
    diff_base_id: crate::workflows::ArtefactId,
    host: std::path::PathBuf,
}

impl GateFixture {
    fn gate_path(&self) -> String {
        format!(
            "/runs/{}/gates/{}",
            self.run_id.as_hex(),
            self.gate_id.as_hex()
        )
    }

    fn desk_path(&self) -> String {
        crate::tests::desk_path(&self.project_id, &self.agent_id)
    }

    fn decision_body(&self, candidate: &str) -> String {
        let surface = if self.conversation_id.is_some() {
            "&surface=conversation"
        } else {
            ""
        };
        format!("gate-revision=1&candidate={candidate}{surface}")
    }
}

fn cookie(token: &str) -> String {
    format!("powerplant_session={token}")
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

fn git_has_head(path: &std::path::Path) -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(path)
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

async fn body_text(response: axum::http::Response<Body>) -> String {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

fn published_gate_candidate(
    run: &workflows::WorkflowRun,
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

fn awaiting_gate(kind: RunKind) -> GateFixture {
    use crate::workflows::definition::{InputKey, OutputKey, StepKey};

    let state = test_state();
    let project_dir = git_worktree();
    std::fs::write(project_dir.path().join("file.txt"), b"candidate\n").expect("source");
    let initial_capture = crate::workflows::artefacts::CandidateCapture::capture_host(
        project_dir.path(),
        &state.workflow_artefacts,
    )
    .expect("initial capture");
    std::fs::write(project_dir.path().join("file.txt"), b"changed\n").expect("change");
    let produced_capture = crate::workflows::artefacts::CandidateCapture::capture_host(
        project_dir.path(),
        &state.workflow_artefacts,
    )
    .expect("changed capture");
    std::fs::write(project_dir.path().join("file.txt"), b"candidate\n").expect("restore source");
    let agent = state
        .agents
        .create(AgentDraft {
            name: "Desk agent".to_owned(),
            instructions: "Do the work.".to_owned(),
            selection: None,
            tools: vec![ToolId::List],
            network: crate::agents::NetworkAccess::None,
            directories: vec![DirectoryGrant {
                alias: "project".to_owned(),
                host_path: project_dir.path().to_path_buf(),
                access: AccessMode::ReadWrite,
            }],
            primary_directory: "project".to_owned(),
        })
        .expect("agent");
    let project = state
        .projects
        .create(
            "Desk project".to_owned(),
            agent.directories[0].host_path.clone(),
        )
        .expect("project");
    let pinned = workflows::pin_quick_task(
        AccessMode::ReadWrite,
        &[ToolId::List],
        "Do the work.",
        crate::tests::test_environment_id(),
    )
    .expect("quick task");
    let environments = crate::tests::test_environment_set(&pinned.definition);
    let mut run = workflows::WorkflowRun::create(
        workflows::RunId::generate().expect("run"),
        1,
        project.id,
        Some(agent.id),
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
    let attempt = workflows::AttemptId::generate().expect("attempt");
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
        vec![initial_ref.clone()],
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
            artefact: produced_ref.clone(),
        },
    )
    .expect("outputs");
    run.record_cleanup(
        attempt,
        crate::workflows::run::AttemptCleanupRecord::Complete,
    )
    .expect("cleanup");
    run.complete_attempt(attempt, 3).expect("complete work");
    let gate_id = workflows::GateId::generate().expect("gate");
    run.open_gate(gate_id, produced_ref, initial_ref, 4)
        .expect("gate");
    let candidate_id = run.gates[0].candidate.id;
    let diff_base_id = run.gates[0].diff_base.id;
    let candidate = run
        .artefact(&candidate_id)
        .and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash)
        .expect("candidate")
        .as_str()
        .to_owned();
    let run_id = run.id;
    let host = project.host_path.clone();
    state.workflow_runs.create(run).expect("store run");
    state.keep_temp_dir(project_dir);
    let token = sessions::generate_session_token().expect("session token");
    let session = token.id();
    state.sessions.insert(session);
    state
        .vault
        .put(ProviderConnection::with_key(
            ProviderKind::Xai,
            "test-key",
            "grok-4.6",
        ))
        .expect("vault");
    let key = sessions::ConversationKey {
        project_id: project.id,
        agent_id: agent.id,
    };
    let begun = state
        .sessions
        .begin_turn(&session, key, run_id, "Change the file".to_owned())
        .expect("turn");
    begun.job.set_awaiting_decision();
    begun.job.set_workflow_name("Quick task".to_owned());
    begun.job.set_step_label("Awaiting decision".to_owned());
    let inserted = state.gate_continuations.insert(workflows::WorkflowJob {
        run_id,
        session_id: session,
        project_id: Some(project.id),
        agent_id: Some(agent.id),
        agent_revision: agent.revision,
        conversation_id: None,
        authority: None,
        project_free_authority: None,
        grant_alias: "project".to_owned(),
        grant_access: AccessMode::ReadWrite,
        connection: ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6"),
        phase_providers: Vec::new(),
        active_connection: Arc::new(std::sync::Mutex::new(None)),
        host_policy: DirectoryPolicy::from_record_with_primary(&agent, "project"),
        turns: begun.turns,
        job: begun.job,
        eligible_reply: std::sync::Arc::new(std::sync::Mutex::new(
            "Here is the change.".to_owned(),
        )),
        task_loop: None,
    });
    assert!(inserted);
    GateFixture {
        state,
        token: token.raw().as_str().to_owned(),
        session,
        key,
        run_id,
        gate_id,
        conversation_id: None,
        project_id: project.id,
        agent_id: agent.id,
        candidate,
        candidate_id,
        diff_base_id,
        host,
    }
}

fn conversation_awaiting_gate() -> GateFixture {
    let mut fixture = awaiting_gate(RunKind::QuickTask);
    let conversation = fixture
        .state
        .conversations
        .create("Implementation".to_owned())
        .expect("conversation");
    let attached = fixture
        .state
        .conversations
        .attach_project(&conversation.id, conversation.revision, fixture.project_id)
        .expect("attach");
    let granted = fixture
        .state
        .conversations
        .grant_writable(&conversation.id, attached.revision, fixture.project_id, 1)
        .expect("grant");
    let old = fixture
        .state
        .gate_continuations
        .take(&fixture.run_id)
        .expect("old continuation");
    drop(old);
    let session_token = sessions::generate_session_token().expect("session token");
    let session = session_token.id();
    fixture.state.sessions.insert(session);
    let selection =
        crate::providers::ModelSelection::new(ProviderKind::Xai, "grok-4.6".to_owned(), None)
            .expect("selection");
    let job = fixture
        .state
        .sessions
        .begin_conversation_job(&session, conversation.id, 1)
        .expect("job");
    fixture
        .state
        .conversations
        .begin_message(
            &conversation.id,
            granted.revision,
            selection,
            job.id(),
            "Change the file".to_owned(),
        )
        .expect("message");
    fixture
        .state
        .workflow_runs
        .mutate(&fixture.run_id, |run| {
            run.conversation_id = Some(conversation.id);
            Ok(())
        })
        .expect("conversation run");
    let run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    let authority = crate::conversations::resolve_workflow_authority(
        &granted,
        &fixture.state.projects,
        &fixture.state.agents,
    )
    .expect("authority")
    .expect("authority");
    job.set_awaiting_decision();
    assert!(fixture.state.sessions.release_job_reservation(
        &session,
        Some(conversation.id),
        job.id(),
    ));
    assert!(
        fixture
            .state
            .gate_continuations
            .insert(workflows::WorkflowJob {
                run_id: fixture.run_id,
                session_id: session,
                project_id: Some(fixture.project_id),
                agent_id: run.agent_id,
                agent_revision: authority.effective.revision,
                conversation_id: Some(conversation.id),
                authority: Some(authority.effective.clone()),
                project_free_authority: None,
                grant_alias: authority.effective.grant_alias.clone(),
                grant_access: authority.effective.grant_access,
                connection: ProviderConnection::with_key(ProviderKind::Xai, "test-key", "grok-4.6"),
                phase_providers: Vec::new(),
                active_connection: Arc::new(std::sync::Mutex::new(None)),
                host_policy: authority.effective.policy.clone(),
                turns: vec![crate::providers::ChatTurn::user(
                    "Change the file".to_owned()
                )],
                job,
                eligible_reply: std::sync::Arc::new(std::sync::Mutex::new(
                    "Here is the change.".to_owned(),
                )),
                task_loop: None,
            })
    );
    fixture.token = session_token.raw().as_str().to_owned();
    fixture.session = session;
    fixture.conversation_id = Some(conversation.id);
    fixture.key = sessions::ConversationKey {
        project_id: fixture.project_id,
        agent_id: run.agent_id.expect("agent"),
    };
    fixture.agent_id = run.agent_id.expect("agent");
    fixture
}

async fn get_gate(fixture: &GateFixture, graft: Option<&str>) -> axum::http::Response<Body> {
    let mut builder = Request::builder()
        .uri(fixture.gate_path())
        .header(header::COOKIE, cookie(&fixture.token));
    if let Some(graft) = graft {
        builder = builder
            .header(hypergraft::GRAFT_REQUEST, graft)
            .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
    }
    app(&fixture.state)
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("gate")
}

async fn post_candidate_review(fixture: &GateFixture, body: String) -> axum::http::Response<Body> {
    let builder = Request::builder()
        .method("POST")
        .uri("/conversations/candidate-review")
        .header(header::COOKIE, cookie(&fixture.token))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(hypergraft::GRAFT_REQUEST, "patch")
        .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
    app(&fixture.state)
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .expect("candidate review")
}

async fn post_decision(
    fixture: &GateFixture,
    action: &str,
    body: String,
    _graft: Option<&str>,
) -> axum::http::Response<Body> {
    let builder = Request::builder()
        .method("POST")
        .uri(format!("{}/{action}", fixture.gate_path()))
        .header(header::COOKIE, cookie(&fixture.token))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(hypergraft::GRAFT_REQUEST, "patch")
        .header(header::ACCEPT, hypergraft::MEDIA_TYPE);
    app(&fixture.state)
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .expect("decision")
}

#[tokio::test]
async fn a_conversation_gate_shows_its_candidate_and_returns_to_the_conversation() {
    let fixture = conversation_awaiting_gate();
    let conversation = fixture.conversation_id.expect("conversation");
    let response = app(&fixture.state)
        .oneshot(
            Request::builder()
                .uri(format!("/conversations/{conversation}"))
                .header(header::COOKIE, cookie(&fixture.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("conversation");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let text = body_text(response).await;
    assert!(text.contains(&format!("value=\"{}\"", fixture.candidate)));
    assert!(text.contains(&format!("action=\"{}/approve\"", fixture.gate_path())));
    assert!(text.contains(&format!("action=\"{}/cancel\"", fixture.gate_path())));

    let gate = body_text(get_gate(&fixture, None).await).await;
    assert!(gate.contains(&format!("/conversations/{conversation}")));
    assert!(!fixture.state.sessions.busy(&fixture.session));
}

#[tokio::test]
async fn a_linked_candidate_review_releases_the_session_but_not_the_source_gate() {
    let mut fixture = conversation_awaiting_gate();
    let backend = crate::providers::tests::ScriptedBackend::accept();
    fixture.state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend));
    let body = format!(
        "run={}&candidate={}&diff_base={}&brief=Review+candidate&provider=xai&model=grok-4.6&thinking=medium",
        fixture.run_id, fixture.candidate_id, fixture.diff_base_id,
    );
    let started = post_candidate_review(&fixture, body).await;
    let started_status = started.status();
    let started_body = body_text(started).await;
    assert_eq!(started_status, axum::http::StatusCode::OK, "{started_body}");
    let review = fixture
        .state
        .conversations
        .list()
        .into_iter()
        .find(|record| record.id != fixture.conversation_id.expect("source"))
        .expect("linked review");
    assert!(
        fixture
            .state
            .conversations
            .get(&fixture.conversation_id.expect("source"))
            .expect("source")
            .active_job
            .is_some()
    );
    assert!(fixture.state.sessions.busy(&fixture.session));

    let rejected = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(rejected.status(), axum::http::StatusCode::CONFLICT);
    assert!(
        fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while fixture
            .state
            .conversations
            .get(&review.id)
            .expect("review")
            .active_job
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("review settlement");
    assert!(!fixture.state.sessions.busy(&fixture.session));
    let approved = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(approved.status(), axum::http::StatusCode::OK);
}

fn reopen_revision(
    run: &workflows::WorkflowRun,
) -> Result<workflows::WorkflowRun, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir().expect("run directory");
    let store = workflows::WorkflowRunStore::open(dir.path().to_path_buf())?;
    store.create(run.clone())?;
    drop(store);
    Ok(workflows::WorkflowRunStore::open(dir.path().to_path_buf())?
        .get(&run.id)
        .expect("run"))
}

fn reserve_revision(run: &mut workflows::WorkflowRun, state: &AppState, at: u64) {
    use workflows::gates::HumanDecisionKind;
    let gate = run.gates.last().expect("gate").clone();
    let diff = workflows::artefacts::CandidateDiff::load(
        run,
        &gate.diff_base,
        &gate.candidate,
        &state.workflow_artefacts,
    )
    .expect("diff");
    let (bytes, object, hash) = workflows::artefacts::encode_human_decision(
        diff.target,
        diff.base,
        HumanDecisionKind::RevisionRequested,
        Some("Fix the candidate"),
        at,
        None,
    )
    .expect("decision");
    state.workflow_artefacts.publish(&bytes).expect("publish");
    let record = super::decision_record(
        run,
        &gate,
        HumanDecisionKind::RevisionRequested,
        at,
        object,
        hash,
        bytes.len() as u64,
    )
    .expect("record");
    run.decide_gate(
        gate.id,
        gate.revision,
        record,
        HumanDecisionKind::RevisionRequested,
        Some("Fix the candidate".to_owned()),
        Some(workflows::AttemptId::generate().expect("attempt")),
        at,
    )
    .expect("reserve revision");
}

#[test]
fn human_revisions_preserve_feedback_identity_and_exhaust_the_gate_step_limit() {
    use workflows::run::{
        AttemptArtefactInput, AttemptArtefactOutput, AttemptCleanupRecord, ObservedCandidate,
        RunState,
    };
    let fixture = conversation_awaiting_gate();
    let mut run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    run.launch_brief = "Original task direction".to_owned();
    for iteration in 0..2 {
        let at = 10 + iteration * 10;
        reserve_revision(&mut run, &fixture.state, at);
        let reservation = run.revision_reservation.clone().expect("reservation");
        reopen_revision(&run).expect("durable reservation");
        let mut interrupted = run.clone();
        interrupted.interrupt(at).expect("interrupt reservation");
        reopen_revision(&interrupted).expect("recover reservation");
        let step = run
            .pinned
            .definition
            .step(&reservation.target)
            .expect("work")
            .clone();
        let inputs = vec![AttemptArtefactInput {
            key: step.inputs[0].key.clone(),
            artefact: reservation.candidate.clone(),
        }];
        let packet = |run: &workflows::WorkflowRun| {
            workflows::input_context::build_attempt_packet_for_request(
                run,
                &step,
                &inputs,
                &fixture.state.workflow_artefacts,
                workflows::input_context::ProjectInstructions::Absent,
                &[crate::providers::ChatTurn::user(
                    "EXCLUDED TRANSCRIPT".to_owned(),
                )],
                "",
                &[],
                None,
                None,
            )
        };
        let context = packet(&run).expect("revision packet");
        let encoded = serde_json::to_string(&context).expect("packet");
        assert!(encoded.contains("Original task direction"));
        assert!(encoded.contains("Fix the candidate"));
        assert!(!encoded.contains("EXCLUDED TRANSCRIPT"));
        let mut substituted = run.clone();
        substituted
            .revision_reservation
            .as_mut()
            .expect("reservation")
            .feedback = "Forged feedback".to_owned();
        assert!(packet(&substituted).is_err());
        let mut substituted = run.clone();
        substituted
            .revision_reservation
            .as_mut()
            .expect("reservation")
            .gate = workflows::GateId::generate().expect("other gate");
        assert!(packet(&substituted).is_err());
        let sandbox = run.attempts[0].sandbox.clone();
        run.start_attempt(
            reservation.attempt,
            inputs,
            crate::tests::test_agent_capabilities(),
            sandbox,
            at + 1,
        )
        .expect("start revision");
        let mut interrupted = run.clone();
        interrupted.interrupt(at + 1).expect("interrupt attempt");
        reopen_revision(&interrupted).expect("recover attempt");
        let mut candidate = run
            .artefact(&reservation.candidate.id)
            .expect("candidate")
            .clone();
        candidate.id = workflows::ArtefactId::generate().expect("new candidate");
        candidate.provenance.producer = workflows::artefacts::ArtefactProducer::StepAttempt {
            attempt_id: reservation.attempt,
            step: step.key.clone(),
            output: Some(workflows::definition::OutputKey::parse("candidate").expect("output")),
            disposition: workflows::artefacts::ProductionDisposition::RequiredOutput,
        };
        candidate.provenance.inputs = vec![reservation.candidate.clone()];
        let reference = workflows::artefacts::ArtefactReference {
            id: candidate.id,
            kind: candidate.kind,
            artefact_hash: candidate.artefact_hash,
        };
        run.record_attempt_outputs(
            reservation.attempt,
            vec![candidate],
            vec![AttemptArtefactOutput {
                key: workflows::definition::OutputKey::parse("candidate").expect("output"),
                artefact: reference.clone(),
            }],
            Some(reference.clone()),
            ObservedCandidate::Exact {
                artefact: reference.clone(),
            },
        )
        .expect("outputs");
        run.record_cleanup(reservation.attempt, AttemptCleanupRecord::Complete)
            .expect("cleanup");
        run.complete_attempt(reservation.attempt, at + 2)
            .expect("complete revision");
        assert!(matches!(run.state, RunState::Ready { .. }));
        run.open_gate(
            workflows::GateId::generate().expect("gate"),
            reference,
            reservation.diff_base,
            at + 3,
        )
        .expect("new approval gate");
        reopen_revision(&run).expect("reopened gate");
    }
    reserve_revision(&mut run, &fixture.state, 40);
    assert!(matches!(
        run.state,
        RunState::Escalated {
            reason: workflows::run::EscalationReason::AttemptLimit,
            ..
        }
    ));
    assert!(run.revision_reservation.is_none());
    assert_eq!(run.attempts.len(), 3);
    reopen_revision(&run).expect("blocked evidence");
}

#[tokio::test]
async fn a_human_revision_dispatches_the_reserved_attempt_and_rejects_a_duplicate() {
    let fixture = conversation_awaiting_gate();
    let body = format!(
        "{}&note=Fix+the+candidate",
        fixture.decision_body(&fixture.candidate)
    );
    let response = post_decision(&fixture, "request-revision", body.clone(), None).await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let run = fixture
                .state
                .workflow_runs
                .get(&fixture.run_id)
                .expect("run");
            if run.attempts.len() == 2 {
                assert_eq!(run.attempts[1].inputs[0].artefact.id, fixture.candidate_id);
                break;
            }
            assert!(
                !run.is_terminal(),
                "revision stopped before dispatch: {:?}",
                run.state
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("revision dispatch");
    let duplicate = post_decision(&fixture, "request-revision", body, None).await;
    assert_eq!(duplicate.status(), axum::http::StatusCode::CONFLICT);
    assert_eq!(
        std::fs::read(fixture.host.join("file.txt")).expect("host"),
        b"candidate\n"
    );
}

#[tokio::test]
async fn another_conversation_keeps_a_gate_open_until_the_original_session_is_free() {
    let fixture = conversation_awaiting_gate();
    let other = fixture
        .state
        .conversations
        .create("Other conversation".to_owned())
        .expect("other");
    let other_job = fixture
        .state
        .sessions
        .begin_conversation_job(&fixture.session, other.id, 1)
        .expect("other job");
    let revision = post_decision(
        &fixture,
        "request-revision",
        format!(
            "{}&note=Fix+the+candidate",
            fixture.decision_body(&fixture.candidate)
        ),
        None,
    )
    .await;
    assert_eq!(revision.status(), axum::http::StatusCode::CONFLICT);
    assert!(
        fixture
            .state
            .workflow_runs
            .get(&fixture.run_id)
            .expect("run")
            .revision_reservation
            .is_none()
    );
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    assert!(
        fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );
    assert!(matches!(
        fixture
            .state
            .workflow_runs
            .get(&fixture.run_id)
            .expect("run")
            .state,
        crate::workflows::run::RunState::AwaitingHuman { .. }
    ));
    assert!(fixture.state.sessions.finish_conversation_job(
        &fixture.session,
        other.id,
        other_job.id()
    ));
    let approved = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(approved.status(), axum::http::StatusCode::OK);
    assert!(body_text(approved).await.contains(&format!(
        "navigate=\"/conversations/{}\"",
        fixture.conversation_id.unwrap()
    )));
    let run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    let decision = run.gates[0].decision.as_ref().expect("approved decision");
    let record = run.artefact(&decision.id).expect("decision record");
    let bytes = fixture
        .state
        .workflow_artefacts
        .get(&record.object_hash)
        .expect("decision bytes");
    let crate::workflows::artefacts::TypedPayload::HumanDecision(payload) =
        crate::workflows::artefacts::parse_typed_payload(record.kind, &bytes).expect("decision")
    else {
        panic!("human decision")
    };
    assert_eq!(payload.candidate, fixture.candidate);
    assert_eq!(
        payload.decision,
        crate::workflows::gates::HumanDecisionKind::Approved
    );
    let duplicate = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(duplicate.status(), axum::http::StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_busy_executor_returns_the_conversation_gate_without_its_session_reservation() {
    let fixture = conversation_awaiting_gate();
    let execution = fixture
        .state
        .workflow_execution
        .acquire()
        .expect("other execution");
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    assert!(!fixture.state.sessions.busy(&fixture.session));
    assert!(
        fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );
    let run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    assert_eq!(
        run.gates[0].state,
        crate::workflows::gates::HumanGateState::AwaitingDecision
    );
    assert!(run.gates[0].decision.is_none());
    assert!(
        fixture
            .state
            .conversations
            .get(&fixture.conversation_id.unwrap())
            .expect("conversation")
            .active_job
            .is_some()
    );
    assert!(fixture.state.workflow_execution.acquire().is_err());
    drop(execution);
}

#[tokio::test]
async fn malformed_conversation_decisions_keep_the_error_on_the_conversation_surface() {
    let fixture = conversation_awaiting_gate();
    let response = post_decision(
        &fixture,
        "approve",
        "gate-revision=invalid&surface=conversation".to_owned(),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    let body = body_text(response).await;
    assert!(body.contains("target=\"conversation-candidate\""));
    assert!(!body.contains("target=\"gate-detail\""));
    assert!(
        fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );
    assert!(!fixture.state.sessions.busy(&fixture.session));
}

#[tokio::test]
async fn a_conversation_discard_settles_only_that_conversation_and_rejects_duplicate_decisions() {
    let fixture = conversation_awaiting_gate();
    let conversation = fixture.conversation_id.expect("conversation");
    let wrong = post_decision(
        &fixture,
        "approve",
        fixture.decision_body("sha256:00"),
        None,
    )
    .await;
    assert_eq!(wrong.status(), axum::http::StatusCode::CONFLICT);
    assert!(
        fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );

    let response = post_decision(
        &fixture,
        "cancel",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let text = body_text(response).await;
    assert!(text.contains(&format!("navigate=\"/conversations/{conversation}\"")));
    assert!(
        fixture
            .state
            .conversations
            .get(&conversation)
            .expect("conversation")
            .active_job
            .is_none()
    );
    assert!(!fixture.state.sessions.busy(&fixture.session));
    assert!(
        !fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );

    let duplicate = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(duplicate.status(), axum::http::StatusCode::CONFLICT);
}

#[tokio::test]
async fn environment_switch_discards_only_the_current_candidate_after_readiness() {
    let fixture = conversation_awaiting_gate();
    let conversation = fixture.conversation_id.unwrap();
    let before = fixture.state.conversations.get(&conversation).unwrap();
    let run_before = fixture.state.workflow_runs.get(&fixture.run_id).unwrap();
    let host_before = std::fs::read(fixture.host.join("file.txt")).unwrap();
    let (replacement, _) = fixture
        .state
        .environments
        .create(crate::environments::EnvironmentDraft {
            name: "Replacement".to_owned(),
            oci_image: "docker.io/library/alpine:3.20".to_owned(),
            setup_script: String::new(),
        })
        .unwrap();
    let body = format!(
        "{}&conversation-revision={}&environment={}",
        fixture.decision_body(&fixture.candidate),
        before.revision,
        replacement.id,
    );
    let response = post_decision(&fixture, "discard-and-switch", body.clone(), None).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        fixture.state.conversations.get(&conversation).unwrap(),
        before
    );
    assert!(
        fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );

    while let Some(preparation) = fixture.state.environments.claim_oldest_queued().unwrap() {
        let snapshot = crate::tests::sample_snapshot(preparation.id);
        fixture.state.environment_snapshots.mark(
            snapshot.artifact_key.clone(),
            crate::environments::snapshot::SnapshotAvailability::Available,
        );
        fixture
            .state
            .environments
            .finish_ready(
                &preparation.id,
                snapshot,
                crate::environments::PreparationLogRecord::empty(),
            )
            .unwrap();
    }
    for invalid in [
        body.replace(&fixture.candidate, "sha256:00"),
        body.replace(
            &format!("conversation-revision={}", before.revision),
            "conversation-revision=0",
        ),
    ] {
        let response = post_decision(&fixture, "discard-and-switch", invalid, None).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            fixture.state.conversations.get(&conversation).unwrap(),
            before
        );
    }
    let response = post_decision(&fixture, "discard-and-switch", body.clone(), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let settled = fixture.state.conversations.get(&conversation).unwrap();
    assert!(settled.active_job.is_none());
    assert_eq!(settled.model.unwrap().settings.environment, replacement.id);
    let run_after = fixture.state.workflow_runs.get(&fixture.run_id).unwrap();
    assert_eq!(run_after.environments, run_before.environments);
    assert!(run_after.artefact(&fixture.candidate_id).is_some());
    assert_eq!(
        std::fs::read(fixture.host.join("file.txt")).unwrap(),
        host_before
    );
    let replay = post_decision(&fixture, "discard-and-switch", body, None).await;
    assert_eq!(replay.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn source_drift_interrupts_a_conversation_before_commit() {
    let fixture = conversation_awaiting_gate();
    std::fs::write(fixture.host.join("file.txt"), b"outside change\n").expect("drift");
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert!(matches!(
        fixture
            .state
            .workflow_runs
            .get(&fixture.run_id)
            .expect("run")
            .state,
        crate::workflows::run::RunState::Interrupted
    ));
    assert!(
        !fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );
    assert_eq!(
        std::fs::read(fixture.host.join("file.txt")).expect("file"),
        b"outside change\n"
    );
}

#[tokio::test]
async fn a_quick_task_gate_uses_apply_and_discard_labels() {
    let fixture = awaiting_gate(RunKind::QuickTask);
    let response = get_gate(&fixture, None).await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let text = body_text(response).await;
    assert!(text.contains("Apply changes"));
    assert!(text.contains("Discard"));
    assert!(!text.contains("Discard changes"));
    assert!(!text.contains("Request revision"));
    assert!(!text.contains("/request-revision"));
    let safety_at = text.find(HOST_UNCHANGED_SAFETY).expect("safety");
    let apply_at = text.find("Apply changes").expect("apply");
    assert!(safety_at < apply_at);
    assert!(text.contains("data-run-kind=\"quick-task\""));
    assert!(text.contains(&format!("data-project=\"{}\"", fixture.project_id.as_hex())));
    assert!(text.contains(&format!("/projects/{}", fixture.project_id.as_hex())));

    let navigation = get_gate(&fixture, Some("navigation")).await;
    assert_eq!(navigation.status(), axum::http::StatusCode::OK);
    let text = body_text(navigation).await;
    assert!(text.contains("target=\"chat-main\""));
    assert!(text.contains("Apply changes"));
    assert!(!text.contains("Request revision"));
}

#[tokio::test]
async fn a_configured_gate_keeps_revision_controls() {
    let fixture = awaiting_gate(RunKind::Configured);
    let text = body_text(get_gate(&fixture, None).await).await;
    assert!(text.contains("/request-revision"));
    assert!(!text.contains(HOST_UNCHANGED_SAFETY));
}

#[tokio::test]
async fn a_quick_task_approval_returns_to_project_detail() {
    let fixture = awaiting_gate(RunKind::QuickTask);
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let text = body_text(response).await;
    assert!(text.contains(&format!(
        "navigate=\"/projects/{}\"",
        fixture.project_id.as_hex()
    )));
    let run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    let decision = run.gates[0].decision.as_ref().expect("decision");
    let record = run.artefact(&decision.id).expect("decision record");
    let bytes = fixture
        .state
        .workflow_artefacts
        .get(&record.object_hash)
        .expect("decision object");
    let payload = crate::workflows::artefacts::parse_typed_payload(record.kind, &bytes)
        .expect("decision payload");
    let crate::workflows::artefacts::TypedPayload::HumanDecision(payload) = payload else {
        panic!("human decision");
    };
    assert_eq!(payload.candidate, fixture.candidate);
    assert_eq!(
        payload.decision,
        crate::workflows::gates::HumanDecisionKind::Approved
    );
}

#[tokio::test]
async fn an_enhanced_quick_task_approval_navigates_to_project_detail() {
    let fixture = awaiting_gate(RunKind::QuickTask);
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        Some("patch"),
    )
    .await;
    let text = body_text(response).await;
    assert!(text.contains(&format!(
        "navigate=\"/projects/{}\"",
        fixture.project_id.as_hex()
    )));
}

#[tokio::test]
async fn a_configured_decision_redirects_to_run_detail() {
    let fixture = awaiting_gate(RunKind::Configured);
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let text = body_text(response).await;
    assert!(text.contains(&format!("navigate=\"/runs/{}\"", fixture.run_id.as_hex())));
}

#[tokio::test]
async fn a_quick_task_discard_settles_the_transcript() {
    let fixture = awaiting_gate(RunKind::QuickTask);
    let response = post_decision(
        &fixture,
        "cancel",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let text = body_text(response).await;
    assert!(text.contains(&format!(
        "navigate=\"/projects/{}\"",
        fixture.project_id.as_hex()
    )));
    let run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    assert!(matches!(
        run.state,
        crate::workflows::run::RunState::Cancelled
    ));
    let snapshot = fixture
        .state
        .sessions
        .snapshot(&fixture.session, &fixture.key)
        .expect("session");
    assert!(!snapshot.session_busy);
    assert_eq!(
        snapshot.turns.last().map(|turn| turn.text.as_str()),
        Some("Here is the change.")
    );
    assert!(!git_has_head(&fixture.host));
    let desk = app(&fixture.state)
        .oneshot(
            Request::builder()
                .uri(fixture.desk_path())
                .header(header::COOKIE, cookie(&fixture.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("desk");
    let desk_text = body_text(desk).await;
    assert!(!desk_text.contains("Task finished."));
    let begun = fixture.state.sessions.begin_turn(
        &fixture.session,
        fixture.key,
        workflows::RunId::generate().expect("next"),
        "Next task".to_owned(),
    );
    assert!(begun.is_ok());
}

#[tokio::test]
async fn a_quick_task_revision_request_is_rejected() {
    let fixture = awaiting_gate(RunKind::QuickTask);
    let response = post_decision(
        &fixture,
        "request-revision",
        format!(
            "gate-revision=1&candidate={}&note=Please+change+it",
            fixture.candidate
        ),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    assert!(
        fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );
    let run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    assert!(matches!(
        run.state,
        crate::workflows::run::RunState::AwaitingHuman { .. }
    ));
}

#[tokio::test]
async fn a_wrong_candidate_decision_is_rejected() {
    let fixture = awaiting_gate(RunKind::QuickTask);
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body("sha256:00"),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    assert!(
        fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );
    assert!(matches!(
        fixture
            .state
            .workflow_runs
            .get(&fixture.run_id)
            .expect("run")
            .state,
        crate::workflows::run::RunState::AwaitingHuman { .. }
    ));
    assert!(!git_has_head(&fixture.host));
}

#[tokio::test]
async fn a_pinned_phase_snapshot_does_not_require_a_live_preset() {
    let fixture = conversation_awaiting_gate();
    let agent = fixture.state.agents.get(&fixture.agent_id).expect("preset");
    fixture
        .state
        .workflow_runs
        .mutate(&fixture.run_id, |run| {
            let step = run
                .pinned
                .definition
                .steps()
                .iter()
                .find(|step| matches!(step.action, workflows::definition::StepAction::Agent(_)))
                .expect("model phase");
            run.phase_models = vec![workflows::PhaseModelSelection {
                step: step.key.clone(),
                selection: crate::providers::ModelSelection::new(
                    ProviderKind::Xai,
                    "grok-4.6".to_owned(),
                    None,
                )
                .expect("model"),
                instructions: agent.instructions.clone(),
                preset: None,
                settings: None,
            }];
            Ok(())
        })
        .expect("pin phase");
    fixture
        .state
        .agents
        .update(
            &agent.id,
            agent.revision,
            AgentDraft {
                name: agent.name,
                instructions: agent.instructions,
                selection: agent.selection,
                tools: Vec::new(),
                network: agent.network,
                directories: agent.directories,
                primary_directory: agent.primary_directory,
            },
        )
        .expect("revoke tools");
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    assert!(!matches!(run.state, workflows::run::RunState::Interrupted));
}

#[tokio::test]
async fn a_stale_agent_revision_interrupts_without_host_mutation() {
    let fixture = awaiting_gate(RunKind::QuickTask);
    let agent = fixture.state.agents.get(&fixture.agent_id).expect("agent");
    fixture
        .state
        .agents
        .update(
            &agent.id,
            agent.revision,
            AgentDraft {
                name: "Renamed agent".to_owned(),
                instructions: agent.instructions.clone(),
                selection: None,
                tools: agent.tools.clone(),
                network: agent.network.clone(),
                directories: agent.directories.clone(),
                primary_directory: agent.primary_directory.clone(),
            },
        )
        .expect("rename");
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let text = body_text(response).await;
    assert!(text.contains(&format!(
        "navigate=\"/projects/{}\"",
        fixture.project_id.as_hex()
    )));
    let run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    assert!(matches!(
        run.state,
        crate::workflows::run::RunState::Interrupted
    ));
    assert!(
        !fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );
    assert!(!git_has_head(&fixture.host));
    let snapshot = fixture
        .state
        .sessions
        .snapshot(&fixture.session, &fixture.key)
        .expect("session");
    assert!(!snapshot.session_busy);
}

#[tokio::test]
async fn a_changed_grant_interrupts_without_host_mutation() {
    let fixture = awaiting_gate(RunKind::QuickTask);
    let other = git_worktree();
    let agent = fixture.state.agents.get(&fixture.agent_id).expect("agent");
    fixture
        .state
        .agents
        .update(
            &agent.id,
            agent.revision,
            AgentDraft {
                name: agent.name.clone(),
                instructions: agent.instructions.clone(),
                selection: None,
                tools: agent.tools.clone(),
                network: agent.network.clone(),
                directories: vec![DirectoryGrant {
                    alias: "project".to_owned(),
                    host_path: other.path().to_path_buf(),
                    access: AccessMode::ReadWrite,
                }],
                primary_directory: "project".to_owned(),
            },
        )
        .expect("change grant");
    fixture.state.keep_temp_dir(other);
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    assert!(matches!(
        run.state,
        crate::workflows::run::RunState::Interrupted
    ));
    assert!(!git_has_head(&fixture.host));
}

#[tokio::test]
async fn an_unavailable_path_keeps_the_continuation() {
    let fixture = awaiting_gate(RunKind::QuickTask);
    std::fs::remove_dir_all(&fixture.host).expect("remove path");
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    let text = body_text(response).await;
    assert!(text.contains("A granted directory is no longer at the saved path."));
    assert!(
        fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );
    let run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    assert!(matches!(
        run.state,
        crate::workflows::run::RunState::AwaitingHuman { .. }
    ));
    let snapshot = fixture
        .state
        .sessions
        .snapshot(&fixture.session, &fixture.key)
        .expect("session");
    assert!(snapshot.session_busy);
}

#[tokio::test]
async fn a_gate_object_download_stays_available() {
    let fixture = awaiting_gate(RunKind::QuickTask);
    let response = app(&fixture.state)
        .oneshot(
            Request::builder()
                .uri(format!("{}/objects/target/0", fixture.gate_path()))
                .header(header::COOKIE, cookie(&fixture.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("object");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/octet-stream"
    );
}

pub(crate) fn loop_at_gate() -> (
    AppState,
    String,
    sessions::SessionId,
    crate::workflows::TaskLoop,
) {
    let fixture = conversation_awaiting_gate();
    let id = attach_parent_loop(&fixture);
    let record = fixture.state.task_loops.get(&id).expect("parent");
    (fixture.state, fixture.token, fixture.session, record)
}

fn attach_parent_loop(fixture: &GateFixture) -> crate::workflows::TaskLoopId {
    use crate::workflows::definition::PinnedWorkflowDefinition;
    use crate::workflows::task_loop::{TaskListSnapshot, TaskLoopItem, TaskOutcome};
    let markdown = "# Tasks\n\n- [ ] First task\n- [ ] Second task\n".to_owned();
    let definition =
        crate::workflows::seeds::task_loop_definition(crate::tests::test_environment_id());
    let loop_id = crate::workflows::TaskLoopId::generate().expect("loop");
    let record = crate::workflows::TaskLoop::create(
        loop_id,
        1,
        fixture.conversation_id.expect("conversation"),
        Some(fixture.project_id),
        Some(fixture.agent_id),
        "Implement each remaining task.".to_owned(),
        PinnedWorkflowDefinition::pin(None, definition.clone()),
        Vec::new(),
        crate::tests::test_environment_set(&definition),
        TaskListSnapshot {
            document_id: crate::conversations::DocumentId::generate().expect("document"),
            revision: 1,
            content_hash: crate::workflows::artefacts::ObjectHash::of(markdown.as_bytes()).as_str(),
            markdown: markdown.clone(),
        },
        vec![
            TaskLoopItem {
                index: 0,
                markdown: "- [ ] First task\n".to_owned(),
                child_id: None,
                previous_child_ids: Vec::new(),
                outcome: TaskOutcome::Pending,
            },
            TaskLoopItem {
                index: 1,
                markdown: "- [ ] Second task\n".to_owned(),
                child_id: None,
                previous_child_ids: Vec::new(),
                outcome: TaskOutcome::Pending,
            },
        ],
    )
    .expect("loop");
    fixture.state.task_loops.create(record).expect("store loop");
    fixture
        .state
        .task_loops
        .mutate(&loop_id, |record| {
            record.tasks[0].child_id = Some(fixture.run_id);
            record.tasks[0].outcome = TaskOutcome::Dispatched;
            record.state = crate::workflows::task_loop::TaskLoopState::AwaitingChild {
                task_index: 0,
                child: fixture.run_id,
            };
            Ok(())
        })
        .expect("awaiting");
    fixture
        .state
        .workflow_runs
        .mutate(&fixture.run_id, |run| {
            run.parent_loop = Some(loop_id);
            Ok(())
        })
        .expect("parent");
    let continuation = fixture
        .state
        .gate_continuations
        .take(&fixture.run_id)
        .expect("continuation");
    let mut continuation = continuation;
    continuation.task_loop = Some(loop_id);
    assert!(fixture.state.gate_continuations.insert(continuation));
    loop_id
}

#[tokio::test]
async fn a_child_gate_keeps_the_parent_conversation_and_transfers_the_execution_lease() {
    let fixture = conversation_awaiting_gate();
    let loop_id = attach_parent_loop(&fixture);
    let conversation = fixture.conversation_id.expect("conversation");
    let response = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert!(
        fixture
            .state
            .conversations
            .get(&conversation)
            .expect("conversation")
            .active_job
            .is_some()
    );
    assert!(
        !fixture
            .state
            .task_loops
            .get(&loop_id)
            .expect("loop")
            .state
            .is_terminal()
    );
    assert!(fixture.state.workflow_execution.acquire().is_err());
}

#[tokio::test]
async fn a_linked_review_can_use_the_session_while_a_parent_loop_awaits_a_child_gate() {
    let mut fixture = conversation_awaiting_gate();
    attach_parent_loop(&fixture);
    let backend = crate::providers::tests::ScriptedBackend::accept();
    fixture.state.chat = std::sync::Arc::new(crate::providers::ChatBackend::Scripted(backend));
    let body = format!(
        "run={}&candidate={}&diff_base={}&brief=Review+candidate&provider=xai&model=grok-4.6&thinking=medium",
        fixture.run_id, fixture.candidate_id, fixture.diff_base_id,
    );
    let started = post_candidate_review(&fixture, body).await;
    assert_eq!(started.status(), axum::http::StatusCode::OK);
    assert!(
        fixture
            .state
            .conversations
            .get(&fixture.conversation_id.expect("source"))
            .expect("source")
            .active_job
            .is_some()
    );
    assert!(fixture.state.sessions.busy(&fixture.session));
    let rejected = post_decision(
        &fixture,
        "approve",
        fixture.decision_body(&fixture.candidate),
        None,
    )
    .await;
    assert_eq!(rejected.status(), axum::http::StatusCode::CONFLICT);
    assert!(
        fixture
            .state
            .gate_continuations
            .available(&fixture.run_id, &fixture.session)
    );
}

#[test]
fn gate_diff_counts_derive_from_complete_evidence_and_reject_out_of_range_navigation() {
    let fixture = awaiting_gate(RunKind::QuickTask);
    let run = fixture
        .state
        .workflow_runs
        .get(&fixture.run_id)
        .expect("run");
    let gate = run
        .gates
        .iter()
        .find(|gate| gate.id == fixture.gate_id)
        .expect("gate");
    let diff = crate::workflows::artefacts::CandidateDiff::load(
        &run,
        &gate.diff_base,
        &gate.candidate,
        &fixture.state.workflow_artefacts,
    )
    .expect("diff");
    let destination = super::application_destination(&fixture.state, &run);
    let query = super::forms::DiffQuery::parse(None, None, None).expect("query");
    let view = super::page::GatePage::new(
        &run,
        gate,
        Some(diff.clone()),
        None,
        &fixture.state.workflow_artefacts,
        query,
        "",
        destination.clone(),
    )
    .expect("gate page");
    assert_eq!(view.total, 1);
    assert_eq!(view.changes.len(), 1);
    assert!(view.changes[0].has_counts);
    assert_eq!(
        (view.changes[0].additions, view.changes[0].removals),
        (1, 1)
    );
    for query in [
        super::forms::DiffQuery::parse(Some("9999"), None, None),
        super::forms::DiffQuery::parse(None, Some("9999"), None),
        super::forms::DiffQuery::parse(None, Some("0"), Some("9999")),
    ] {
        assert!(
            super::page::GatePage::new(
                &run,
                gate,
                Some(diff.clone()),
                None,
                &fixture.state.workflow_artefacts,
                query.expect("query"),
                "",
                destination.clone(),
            )
            .is_none(),
            "out-of-range diff navigation cannot render"
        );
    }
    assert!(super::forms::DiffQuery::parse(Some("00"), None, None).is_none());
    assert!(super::forms::DiffQuery::parse(Some("42949672967"), None, None).is_none());
}
