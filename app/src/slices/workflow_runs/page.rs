use askama::Template;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::environments::EnvironmentCatalogue;
use crate::state::AppState;
use crate::workflows::summary::ProcessPhase;
use crate::workflows::{RunSummary, WorkflowCatalogue, WorkflowRun};

pub(super) const INDEX_TITLE: &str = "Runs | Power Plant";
pub(super) const DETAIL_TITLE: &str = "Run | Power Plant";
pub(super) const ARTEFACT_TITLE: &str = "Artefact | Power Plant";

#[derive(Template)]
#[template(path = "workflow_runs/templates/context.html")]
pub(super) struct InitialContextView {
    pub(super) run_href: String,
    pub(super) section: String,
    pub(super) position: String,
    pub(super) previous_href: String,
    pub(super) next_href: String,
    pub(super) prompt: String,
    pub(super) source_available: String,
    pub(super) excluded_context: String,
    pub(super) instruction_state: String,
    pub(super) instruction_candidate: String,
    pub(super) instruction_path: String,
    pub(super) instruction_hash: String,
    pub(super) instruction_text: String,
    pub(super) packet_bytes: String,
    pub(super) reserved_output_bytes: String,
    pub(super) reserved_tool_bytes: String,
    pub(super) total_bytes: String,
    pub(super) estimated_input_tokens: String,
    pub(super) estimated_total_tokens: String,
    pub(super) model_capacity: String,
}

pub(super) struct AttemptView {
    pub(super) ordinal: u32,
    pub(super) step: String,
    pub(super) action: &'static str,
    pub(super) state: &'static str,
    pub(super) started: String,
    pub(super) finished: String,
    pub(super) result: String,
    pub(super) tools: String,
    pub(super) primary_access: &'static str,
    pub(super) source_location: &'static str,
    pub(super) git_admin: &'static str,
    pub(super) network: String,
    pub(super) reports: Vec<StepArtefactView>,
    pub(super) route: String,
    pub(super) context_href: String,
    pub(super) activity_href: String,
    pub(super) changes_href: String,
    pub(super) result_href: String,
    pub(super) evidence_state: &'static str,
    pub(super) apply_roots: Vec<ApplyRootView>,
}

pub(super) struct ApplyRootView {
    pub(super) directory: String,
    pub(super) path: String,
    pub(super) outcome: &'static str,
}

pub(super) struct AttemptActivityItem {
    pub(super) sequence: u64,
    pub(super) phase: String,
    pub(super) kind: &'static str,
    pub(super) text: String,
    pub(super) label: String,
    pub(super) provider: String,
    pub(super) input_tokens: String,
    pub(super) truncated: bool,
}

#[derive(Template)]
#[template(path = "workflow_runs/templates/attempt_activity.html")]
pub(super) struct AttemptActivityView {
    pub(super) run_href: String,
    pub(super) attempt: String,
    pub(super) phase: String,
    pub(super) status: String,
    pub(super) unavailable: bool,
    pub(super) activity_truncated: bool,
    pub(super) events: Vec<AttemptActivityItem>,
}

pub(super) struct AttemptToolView {
    pub(super) label: String,
    pub(super) output: String,
    pub(super) truncated: bool,
}

#[derive(Template)]
#[template(path = "workflow_runs/templates/attempt_result.html")]
pub(super) struct AttemptResultView {
    pub(super) run_href: String,
    pub(super) attempt: String,
    pub(super) phase: String,
    pub(super) status: String,
    pub(super) unavailable: bool,
    pub(super) text: String,
    pub(super) response_html: String,
    pub(super) thinking: String,
    pub(super) error: String,
    pub(super) truncated: bool,
    pub(super) tools: Vec<AttemptToolView>,
}

#[derive(Template)]
#[template(path = "workflow_runs/templates/direct_changes.html")]
pub(crate) struct DirectChangesView {
    pub(crate) run_href: String,
    pub(crate) conversation_href: String,
    pub(crate) href: String,
    pub(crate) unknown: bool,
    pub(crate) total: usize,
    pub(crate) files: Vec<DirectFileView>,
    pub(crate) previous: Option<usize>,
    pub(crate) next: Option<usize>,
}

pub(crate) struct DirectFileView {
    pub(crate) path: String,
    pub(crate) status: &'static str,
    pub(crate) text: String,
    pub(crate) before_href: String,
    pub(crate) after_href: String,
}

impl DirectChangesView {
    pub(crate) fn new(
        run: &WorkflowRun,
        attempt: &crate::workflows::run::AttemptRecord,
        state: &crate::state::AppState,
        start: usize,
    ) -> Self {
        let href = format!(
            "/runs/{}/attempts/{}/changes",
            run.id.as_hex(),
            attempt.id.as_hex()
        );
        let diff = attempt
            .direct_changes
            .as_ref()
            .and_then(|changes| changes.diff(state));
        let page = diff
            .as_ref()
            .and_then(|diff| diff.manifest_page(start, 16).ok());
        let mut budget = 128 * 1024;
        let (total, files) = page.map(|(total, rows)| {
            let files = rows.into_iter().enumerate().map(|(index, row)| {
                let index = start + index;
                let change = diff.as_ref().and_then(|diff| diff.change(index, &state.workflow_artefacts).ok());
                let text = change.and_then(|change| change.text).map(|fragments| fragments.into_iter().map(|part| part.text).collect::<String>())
                    .filter(|text| crate::markdown::escape_plain(text).len() <= budget)
                    .inspect(|text| { budget -= crate::markdown::escape_plain(text).len(); })
                    .unwrap_or_else(|| "The text preview is unavailable or exceeds the page limit. Download the recorded file for its exact content.".to_owned());
                DirectFileView {
                    path: format!("{}/{}", row.directory, row.path), status: row.status, text,
                    before_href: if row.old.as_ref().is_some_and(|entry| entry.object.is_some()) { format!("{href}?file={index}&side=before") } else { String::new() },
                    after_href: if row.new.as_ref().is_some_and(|entry| entry.object.is_some()) { format!("{href}?file={index}&side=after") } else { String::new() },
                }
            }).collect();
            (total, files)
        }).unwrap_or_default();
        Self {
            run_href: format!("/runs/{}", run.id.as_hex()),
            conversation_href: run
                .conversation_id
                .map(|id| format!("/conversations/{}", id.as_hex()))
                .unwrap_or_default(),
            href,
            unknown: diff.is_none(),
            total,
            files,
            previous: (start > 0).then(|| start.saturating_sub(16)),
            next: (start.saturating_add(16) < total).then(|| start + 16),
        }
    }
}

pub(super) struct AttemptChangeView {
    pub(super) href: String,
    pub(super) kind: String,
    pub(super) hash: String,
    pub(super) preview: String,
    pub(super) truncated: bool,
}

#[derive(Template)]
#[template(path = "workflow_runs/templates/attempt_changes.html")]
pub(super) struct AttemptChangesView {
    pub(super) run_href: String,
    pub(super) attempt: String,
    pub(super) changes: Vec<AttemptChangeView>,
}

pub(super) struct StepArtefactView {
    pub(super) href: String,
    pub(super) key: String,
    pub(super) kind: &'static str,
    pub(super) candidate_hash: String,
    pub(super) status: &'static str,
    pub(super) note: &'static str,
    pub(super) review_href: String,
}

pub(super) struct StepView {
    pub(super) name: String,
    pub(super) action: &'static str,
    pub(super) candidate_access: &'static str,
    pub(super) environment: String,
    pub(super) status: &'static str,
    pub(super) result: String,
    pub(super) artefacts: Vec<StepArtefactView>,
    pub(super) commits: Vec<RepositoryCommitView>,
    pub(super) gate_href: String,
    pub(super) review_phase: String,
    pub(super) attempt_limit: String,
    pub(super) latest_verdict: String,
    pub(super) selected_route: String,
    pub(super) role: String,
    pub(super) model: String,
    pub(super) host_approval: String,
}

pub(super) struct RepositoryCommitView {
    pub(super) path: String,
    pub(super) status: &'static str,
    pub(super) commit: String,
}

pub(super) struct PendingHostCommandView {
    pub(super) command: String,
    pub(super) directory: String,
    pub(super) explanation: String,
    pub(super) step: String,
}

pub(super) struct PinnedEnvironmentView {
    pub(super) name: String,
    pub(super) note: String,
    pub(super) preparation: String,
    pub(super) recipe: String,
    pub(super) snapshot: String,
    pub(super) image: String,
}

#[derive(Template)]
#[template(path = "workflow_runs/templates/index.html")]
pub(super) struct RunIndexView {
    pub(super) runs: Vec<IndexRow>,
    pub(super) directories: Vec<RunDirectoryOption>,
    pub(super) filter: String,
    pub(super) error: &'static str,
}

pub(super) struct RunDirectoryOption {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) selected: bool,
    available: bool,
}

pub(super) struct IndexRow {
    pub(super) id: String,
    pub(super) href: String,
    pub(super) project_name: String,
    pub(super) conversation_href: String,
    pub(super) conversation_title: String,
    pub(super) name: String,
    pub(super) state: String,
    pub(super) created: String,
    pub(super) current_step: String,
}

impl RunIndexView {
    pub(super) fn filtered(state: &AppState, filter: &str, error: &'static str) -> Self {
        let mut directories: std::collections::BTreeMap<String, RunDirectoryOption> =
            std::collections::BTreeMap::new();
        let mut note_directory = |grant: &crate::execution::DirectoryGrant| {
            let id = run_directory_key(grant);
            let available = grant.is_available();
            let option = RunDirectoryOption {
                selected: filter == id,
                id: id.clone(),
                name: format!(
                    "{}{}",
                    grant.host_path.display(),
                    if available { "" } else { " — Unavailable" }
                ),
                available,
            };
            let existing = directories.entry(id).or_insert(option);
            if available && !existing.available {
                existing.name = grant.host_path.display().to_string();
                existing.available = true;
            }
        };
        for summary in state.workflow_runs.all_summaries() {
            if let Some(run) = state.workflow_runs.get(&summary.id) {
                for grant in run_grants(&run) {
                    note_directory(grant);
                }
            }
        }
        // An empty filter can use the truncated summaries. Any run beyond the
        // newest fifty cannot enter the newest fifty combined rows. A directory
        // filter must start from every stored identity before the bound.
        let run_summaries = if filter.is_empty() {
            state.workflow_runs.summaries()
        } else {
            state.workflow_runs.all_summaries()
        };
        let mut rows: Vec<(u64, String, IndexRow)> = run_summaries
            .into_iter()
            .filter(|summary| {
                filter.is_empty()
                    || state.workflow_runs.get(&summary.id).is_some_and(|run| {
                        run_grants(&run).any(|grant| run_directory_key(grant) == filter)
                    })
            })
            .map(|summary| {
                let mut row = index_row_from_run(&summary);
                let owner = state
                    .workflow_runs
                    .get(&summary.id)
                    .and_then(|run| run.conversation_id);
                (row.conversation_href, row.conversation_title) =
                    conversation_presentation(state, owner);
                (summary.created_at_ms, summary.id.as_hex(), row)
            })
            .collect();
        rows.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.cmp(&left.1)));
        rows.truncate(50);
        let mut directories: Vec<_> = directories.into_values().collect();
        directories.sort_by(|left, right| left.name.cmp(&right.name));
        Self {
            runs: rows.into_iter().map(|(_, _, row)| row).collect(),
            directories,
            filter: filter.to_owned(),
            error,
        }
    }
}

pub(super) fn run_grants(
    run: &WorkflowRun,
) -> impl Iterator<Item = &crate::execution::DirectoryGrant> {
    run.phase_models
        .iter()
        .filter_map(|phase| phase.settings.as_ref())
        .flat_map(|settings| &settings.directories)
}

pub(super) fn run_directory_key(grant: &crate::execution::DirectoryGrant) -> String {
    format!(
        "{:016x}-{:016x}",
        grant.identity.device, grant.identity.inode
    )
}

fn conversation_presentation(
    state: &AppState,
    owner: Option<crate::conversations::ConversationId>,
) -> (String, String) {
    match owner {
        Some(id) => match state.conversations.get(&id) {
            Some(record) => (format!("/conversations/{}", id.as_hex()), record.title),
            None => (String::new(), "Conversation unavailable".to_owned()),
        },
        None => (String::new(), "No owning conversation".to_owned()),
    }
}

fn index_row_from_run(summary: &RunSummary) -> IndexRow {
    let (_, project_name) = project_presentation();
    IndexRow {
        id: summary.id.as_hex(),
        href: format!("/runs/{}", summary.id.as_hex()),
        conversation_href: String::new(),
        conversation_title: String::new(),
        project_name,
        name: summary.name.clone(),
        state: summary.state.clone(),
        created: format_time(summary.created_at_ms),
        current_step: active_step(&summary.state, &summary.current_step),
    }
}

fn active_step(state: &str, current_step: &str) -> String {
    if matches!(
        state,
        "Initialising source" | "Ready" | "Active" | "Awaiting decision"
    ) {
        current_step.to_owned()
    } else {
        String::new()
    }
}

#[derive(Template)]
#[template(path = "workflow_runs/templates/detail.html")]
pub(super) struct RunDetailView {
    pub(super) run_id: String,
    pub(super) conversation_href: String,
    pub(super) project_href: String,
    pub(super) project_name: String,
    pub(super) name: String,
    pub(super) name_href: String,
    pub(super) catalogue_note: String,
    pub(super) version: String,
    pub(super) state: &'static str,
    pub(super) state_note: &'static str,
    pub(super) review_href: String,
    pub(super) created: String,
    pub(super) current_step: String,
    pub(super) steps: Vec<StepView>,
    pub(super) environments: Vec<PinnedEnvironmentView>,

    pub(super) attempts: Vec<AttemptView>,
    pub(super) artefacts: Vec<ArtefactRow>,

    pub(super) context_boundaries: String,
    pub(super) process_phases: Vec<ProcessPhase>,
    pub(super) host_approval: String,
    pub(super) pending_host_command: Option<PendingHostCommandView>,
}

pub(super) struct ArtefactRow {
    pub(super) href: String,
    pub(super) kind: &'static str,
    pub(super) hash: String,
    pub(super) producer: &'static str,
    pub(super) created: String,
    pub(super) status: &'static str,
}

#[derive(Template)]
#[template(path = "workflow_runs/templates/detail.html", block = "run_detail")]
pub(super) struct RunDetailContents<'a> {
    pub(super) run_id: &'a str,
    pub(super) conversation_href: &'a str,
    pub(super) project_href: &'a str,
    pub(super) project_name: &'a str,
    pub(super) name: &'a str,
    pub(super) name_href: &'a str,
    pub(super) catalogue_note: &'a str,
    pub(super) version: &'a str,
    pub(super) state: &'static str,
    pub(super) state_note: &'static str,
    pub(super) review_href: &'a str,
    pub(super) created: &'a str,
    pub(super) current_step: &'a str,
    pub(super) steps: &'a [StepView],
    pub(super) environments: &'a [PinnedEnvironmentView],

    pub(super) attempts: &'a [AttemptView],
    pub(super) artefacts: &'a [ArtefactRow],

    pub(super) context_boundaries: &'a str,
    pub(super) process_phases: &'a [ProcessPhase],
    pub(super) host_approval: &'a str,
    pub(super) pending_host_command: &'a Option<PendingHostCommandView>,
}

impl RunDetailView {
    pub(super) fn from_run(
        run: &WorkflowRun,
        workflows: &WorkflowCatalogue,
        environments: &EnvironmentCatalogue,
        evidence: &crate::workflows::WorkflowEvidenceStore,
    ) -> Self {
        let (name_href, catalogue_note) = catalogue_presentation(run, workflows);
        let (project_href, mut project_name) = project_presentation();
        if let Some(settings) = run.directory_settings()
            && !settings.directories.is_empty()
        {
            project_name = settings
                .directories
                .iter()
                .map(|grant| format!("{} ({})", grant.host_path.display(), grant.guest_path()))
                .collect::<Vec<_>>()
                .join(", ");
        }
        let current_step = run.current_step_name().unwrap_or("").to_owned();
        let context_boundaries = if run.kind == crate::workflows::RunKind::QuickTask {
            "Ordinary work retains conversation context. Revision attempts use the original brief, exact candidate and explicit feedback."
        } else {
            "Each model step uses the exact brief, declared inputs and authorised directory instructions. Other step transcripts stay excluded."
        }.to_owned();
        Self {
            run_id: run.id.as_hex(),
            conversation_href: run.conversation_id.map(|id| format!("/conversations/{}", id.as_hex())).unwrap_or_default(),
            project_href,
            project_name,
            name: run.pinned.definition.name().to_owned(),
            name_href,
            catalogue_note,
            version: run.pinned.version.as_hex(),
            state: run.state.as_label(),
            state_note: if run.completed_without_changes() {
                "The task completed without changes. No commit was created."
            } else {
                state_note(&run.state)
            },
            review_href: match &run.state {
                crate::workflows::run::RunState::AwaitingHuman { gate, .. } => {
                    format!("/runs/{}/gates/{}", run.id.as_hex(), gate.as_hex())
                }
                _ => String::new(),
            },
            created: format_time(run.created_at_ms),
            current_step,

            steps: run
                .pinned
                .definition
                .steps()
                .iter()
                .map(|step| {
                    let attempt = run
                        .attempts
                        .iter()
                        .rev()
                        .find(|attempt| attempt.step == step.key);
                    let gate = run.gates.iter().rev().find(|gate| gate.step == step.key);
                    StepView {
                        name: step.name.clone(),
                        action: step.action.kind_label(),
                        candidate_access: match &step.action {
                            crate::workflows::definition::StepAction::Agent(action) => {
                                action.candidate_authority.label()
                            }
                            crate::workflows::definition::StepAction::SystemCommand(_)
                            | crate::workflows::definition::StepAction::HumanGate(_) => "",
                        },
                        environment: step_environment_label(run, step),
                        status: step_status(run, attempt, gate),
                        result: attempt
                            .and_then(|attempt| attempt.result.as_ref())
                            .map(|result| result.as_label())
                            .unwrap_or_default(),
                        artefacts: attempt
                            .into_iter()
                            .flat_map(|attempt| &attempt.outputs)
                            .map(|output| {
                                let record = run.artefact(&output.artefact.id);
                                let candidate_hash = record
                                    .and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash)
                                    .map(|hash| hash.short())
                                    .unwrap_or_default();
                                let status = match output.artefact.kind {
                                    crate::workflows::definition::ArtefactKind::ReviewReport => {
                                        match (
                                            record.and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash),
                                            next_candidate_hash(run, step),
                                        ) {
                                            (Some(report), Some(next)) if report == next => "Current",
                                            (Some(_), Some(_)) => "Superseded",
                                            _ => "",
                                        }
                                    }
                                    crate::workflows::definition::ArtefactKind::CandidateRevision => {
                                        match (
                                            record.and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash),
                                            following_candidate_hash(run, step),
                                        ) {
                                            (Some(candidate), Some(following)) if candidate != following => {
                                                "Superseded"
                                            }
                                            _ => "",
                                        }
                                    }
                                    _ => "",
                                };
                                StepArtefactView {
                                    href: format!(
                                        "/runs/{}/artefacts/{}",
                                        run.id.as_hex(),
                                        output.artefact.id.as_hex()
                                    ),
                                    key: output.key.as_str().to_owned(),
                                    kind: output.artefact.kind.as_str(),
                                    candidate_hash,
                                    status,
                                    note: if output.artefact.kind
                                        == crate::workflows::definition::ArtefactKind::ReviewReport
                                        && step.inputs.iter().any(|input| {
                                            input.kind
                                                == crate::workflows::definition::ArtefactKind::ReviewReport
                                        })
                                    {
                                        "Independent review"
                                    } else {
                                        ""
                                    },
                                    review_href: candidate_review_href(run, record),
                                }
                            })
                            .collect(),
                        commits: attempt.and_then(|attempt| attempt.commit_transaction.as_ref())
                            .map(|transaction| transaction.roots.iter().map(|root| RepositoryCommitView {
                                path: root.grant.host_path.display().to_string(),
                                status: match root.state {
                                    crate::workflows::CommitTransactionState::Prepared => "Not committed",
                                    crate::workflows::CommitTransactionState::WorktreeApplied => "Files applied · commit incomplete",
                                    crate::workflows::CommitTransactionState::ReferenceUpdated { .. } => "Commit requires recovery",
                                    crate::workflows::CommitTransactionState::Verified { .. } => "Committed",
                                    crate::workflows::CommitTransactionState::Restored => "Not committed · no task changes remain",
                                },
                                commit: root.verified_commit().unwrap_or_default().to_owned(),
                            }).collect()).unwrap_or_default(),
                        gate_href: gate.map(|gate| format!("/runs/{}/gates/{}", run.id.as_hex(), gate.id.as_hex())).unwrap_or_default(),
                        review_phase: run.pinned.definition.review_phase(&step.key).map(|phase| phase.to_string()).unwrap_or_default(),
                        attempt_limit: step
                            .review
                            .as_ref()
                            .map(|policy| policy.attempt_limit.to_string())
                            .unwrap_or_default(),
                        latest_verdict: latest_review_verdict(run, attempt),
                        selected_route: review_route(attempt),
                        role: match &step.action {
                            crate::workflows::definition::StepAction::Agent(action) => run.pinned.definition.role(&action.role).map(|role| role.name.clone()).unwrap_or_default(),
                            _ => String::new(),
                        },
                        model: phase_model_label(run, step),
                        host_approval: run
                            .phase_settings(&step.key)
                            .filter(|settings| {
                                settings.location == crate::execution::ToolLocation::Host
                            })
                            .map(|settings| {
                                crate::slices::execution_settings::page::host_approval_label(
                                    settings.host_approval,
                                )
                                .to_owned()
                            })
                            .unwrap_or_default(),
                    }
                })
                .collect(),
            environments: pinned_environments(run, environments),

            attempts: run
                .attempts
                .iter()
                .map(|attempt| AttemptView {
                    ordinal: attempt.ordinal,
                    step: attempt.step.as_str().to_owned(),
                    action: attempt.action_kind.as_label(),
                    state: attempt.state.as_label(),
                    started: format_time(attempt.started_at_ms),
                    finished: attempt.finished_at_ms.map(format_time).unwrap_or_default(),
                    result: attempt
                        .result
                        .as_ref()
                        .map(|result| result.as_label())
                        .unwrap_or_default(),
                    tools: attempt.capabilities.tools_label(),
                    primary_access: attempt.capabilities.primary_access_label(),
                    source_location: attempt.capabilities.source_location.label(),
                    git_admin: attempt.capabilities.git_admin.as_str(),
                    network: attempt.capabilities.network_label(),
                    reports: attempt.outputs.iter().filter(|output| output.artefact.kind == crate::workflows::definition::ArtefactKind::ReviewReport).map(|output| {
                        let record = run.artefact(&output.artefact.id);
                        let current = match (&run.source, record.and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash)) {
                            (crate::workflows::run::RunSource::Captured { source }, Some(hash)) => run.artefact(&source.accepted.id).and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash) == Some(hash),
                            _ => false,
                        };
                        StepArtefactView {
                            href: format!("/runs/{}/artefacts/{}", run.id.as_hex(), output.artefact.id.as_hex()),
                            key: output.key.as_str().to_owned(),
                            kind: output.artefact.kind.as_str(),
                            candidate_hash: record.and_then(crate::workflows::artefacts::ArtefactRecord::candidate_hash).map(|hash| hash.short()).unwrap_or_default(),
                            status: if current { "Current" } else { "Superseded" },
                            note: "",
                            review_href: candidate_review_href(run, record),
                        }
                    }).collect(),
                    route: review_route(Some(attempt)),
                    context_href: if attempt.initial_context.is_some() {
                        format!("/runs/{}/attempts/{}/context", run.id.as_hex(), attempt.id.as_hex())
                    } else {
                        String::new()
                    },
                    activity_href: format!(
                        "/runs/{}/attempts/{}/activity",
                        run.id.as_hex(),
                        attempt.id.as_hex()
                    ),
                    changes_href: format!(
                        "/runs/{}/attempts/{}/changes",
                        run.id.as_hex(),
                        attempt.id.as_hex()
                    ),
                    result_href: format!(
                        "/runs/{}/attempts/{}/result",
                        run.id.as_hex(),
                        attempt.id.as_hex()
                    ),
                    evidence_state: if evidence.get(&run.id, &attempt.id).is_some() {
                        "Available"
                    } else {
                        "Unavailable"
                    },
                    apply_roots: attempt
                        .apply_transaction
                        .as_ref()
                        .map(|transaction| {
                            transaction
                                .roots
                                .iter()
                                .map(|root| ApplyRootView {
                                    directory: root.alias.clone(),
                                    path: root.host_path.display().to_string(),
                                    outcome: match root.outcome {
                                        crate::workflows::apply::ApplyRootOutcome::Pending => "Pending",
                                        crate::workflows::apply::ApplyRootOutcome::Unchanged => "Unchanged",
                                        crate::workflows::apply::ApplyRootOutcome::Applied => "Applied",
                                        crate::workflows::apply::ApplyRootOutcome::Conflicted => "Conflicted",
                                        crate::workflows::apply::ApplyRootOutcome::Uncertain => "Uncertain",
                                    },
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect(),
            artefacts: artefact_rows(run),
            context_boundaries,
            process_phases: if run.kind == crate::workflows::run::RunKind::Configured {
                crate::workflows::summary::process_overview(&run.pinned.definition)
            } else {
                Vec::new()
            },
            host_approval: run
                .phase_models
                .iter()
                .filter_map(|phase| phase.settings.as_ref())
                .find(|settings| settings.location == crate::execution::ToolLocation::Host)
                .map(|settings| {
                    crate::slices::execution_settings::page::host_approval_label(
                        settings.host_approval,
                    )
                    .to_owned()
                })
                .unwrap_or_default(),
            pending_host_command: None,
        }
    }

    pub(super) fn with_pending_host_command(
        mut self,
        command: Option<crate::execution::HostCommandRequest>,
    ) -> Self {
        self.pending_host_command = command
            .filter(|command| command.run.as_deref() == Some(self.run_id.as_str()))
            .map(|command| PendingHostCommandView {
                command: command.command,
                directory: command.directory.display().to_string(),
                explanation: if command.explanation.is_empty() {
                    "The model did not explain this command.".to_owned()
                } else {
                    command.explanation
                },
                step: command.step.unwrap_or_default(),
            });
        self
    }

    pub(super) fn contents(&self) -> RunDetailContents<'_> {
        RunDetailContents {
            run_id: &self.run_id,
            conversation_href: &self.conversation_href,
            project_href: &self.project_href,
            project_name: &self.project_name,
            name: &self.name,
            name_href: &self.name_href,
            catalogue_note: &self.catalogue_note,
            version: &self.version,
            state: self.state,
            state_note: self.state_note,
            review_href: &self.review_href,
            created: &self.created,
            current_step: &self.current_step,
            steps: &self.steps,
            environments: &self.environments,

            attempts: &self.attempts,
            artefacts: &self.artefacts,

            context_boundaries: &self.context_boundaries,
            process_phases: &self.process_phases,
            host_approval: &self.host_approval,
            pending_host_command: &self.pending_host_command,
        }
    }
}

pub(super) fn initial_context_view(
    packet: &crate::workflows::input_context::AttemptContextPacket,
    run_href: &str,
    context_href: &str,
    part: usize,
    offset: usize,
) -> Option<InitialContextView> {
    // One bounded section keeps escaped HTML below the Hypergraft envelope limit.
    const PAGE_BYTES: usize = 16 * 1024;
    let (section, text) = if part == 0 {
        ("Initial prompt".to_owned(), packet.prompt.clone())
    } else if let Some(message) = packet.messages.get(part - 1) {
        (
            format!("Message {part} · {}", message.role()),
            message.text().to_owned(),
        )
    } else if part == packet.messages.len() + 1 {
        (
            "Tool definitions".to_owned(),
            serde_json::to_string_pretty(&packet.tools).ok()?,
        )
    } else {
        return None;
    };
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let end = text.floor_char_boundary(offset.saturating_add(PAGE_BYTES).min(text.len()));
    let next_href = if end < text.len() {
        format!("{context_href}?part={part}&offset={end}")
    } else if part <= packet.messages.len() {
        format!("{context_href}?part={}&offset=0", part + 1)
    } else {
        String::new()
    };
    let previous_href = if offset > 0 {
        format!(
            "{context_href}?part={part}&offset={}",
            text.ceil_char_boundary(offset.saturating_sub(PAGE_BYTES))
        )
    } else if part > 0 {
        format!("{context_href}?part={}&offset=0", part - 1)
    } else {
        String::new()
    };
    let (instruction_state, instruction_text) = match &packet.project_instructions.state {
        crate::workflows::input_context::ProjectInstructionState::Absent => (
            "Absent".to_owned(),
            "No root AGENTS.md file was present in this candidate.".to_owned(),
        ),
        crate::workflows::input_context::ProjectInstructionState::Present { text, .. } => {
            ("Present".to_owned(), text.clone())
        }
    };
    let instruction_candidate = packet
        .project_instructions
        .candidate
        .as_ref()
        .map(|candidate| {
            format!(
                "{} · {} · {}",
                candidate.id, candidate.kind, candidate.artefact_hash
            )
        })
        .unwrap_or_else(|| "Not available".to_owned());
    Some(InitialContextView {
        run_href: run_href.to_owned(),
        section,
        position: format!("Bytes {offset}–{end} of {}.", text.len()),
        previous_href,
        next_href,
        prompt: text[offset..end].to_owned(),
        source_available: packet.source_available.clone(),
        excluded_context: packet.excluded_context.clone(),
        instruction_state,
        instruction_candidate,
        instruction_path: packet.project_instructions.guest_path.clone(),
        instruction_hash: packet
            .project_instructions
            .content_hash()
            .unwrap_or("None")
            .to_owned(),
        instruction_text,
        packet_bytes: packet.budget.packet_bytes.to_string(),
        reserved_output_bytes: packet.budget.reserved_output_bytes.to_string(),
        reserved_tool_bytes: packet.budget.reserved_tool_bytes.to_string(),
        total_bytes: packet.budget.total_bytes.to_string(),
        estimated_input_tokens: packet.budget.estimated_input_tokens.to_string(),
        estimated_total_tokens: packet.budget.estimated_total_tokens.to_string(),
        model_capacity: packet.budget.capacity_label(),
    })
}

pub(super) fn attempt_activity_view(
    run: &WorkflowRun,
    attempt: &crate::workflows::run::AttemptRecord,
    evidence: Option<crate::workflows::evidence::AttemptEvidence>,
) -> AttemptActivityView {
    let run_href = format!("/runs/{}", run.id.as_hex());
    let attempt_label = format!("Attempt {} · {}", attempt.ordinal, attempt.step.as_str());
    let Some(evidence) = evidence else {
        return AttemptActivityView {
            run_href,
            attempt: attempt_label,
            phase: attempt.step.as_str().to_owned(),
            status: attempt.state.as_label().to_owned(),
            unavailable: true,
            activity_truncated: false,
            events: Vec::new(),
        };
    };
    AttemptActivityView {
        run_href,
        attempt: attempt_label,
        phase: evidence.phase,
        status: attempt.state.as_label().to_owned(),
        unavailable: false,
        activity_truncated: evidence.activity_truncated,
        events: evidence
            .events
            .into_iter()
            .map(|event| AttemptActivityItem {
                sequence: event.sequence,
                phase: event.phase,
                kind: event.kind.label(),
                text: event.text,
                label: event.label,
                provider: event.provider.unwrap_or_default(),
                input_tokens: event
                    .input_tokens
                    .map(|tokens| tokens.to_string())
                    .unwrap_or_default(),
                truncated: event.truncated,
            })
            .collect(),
    }
}

pub(super) fn attempt_result_view(
    run: &WorkflowRun,
    attempt: &crate::workflows::run::AttemptRecord,
    evidence: Option<crate::workflows::evidence::AttemptEvidence>,
) -> AttemptResultView {
    let run_href = format!("/runs/{}", run.id.as_hex());
    let attempt_label = format!("Attempt {} · {}", attempt.ordinal, attempt.step.as_str());
    let Some(terminal) = evidence.and_then(|evidence| evidence.terminal) else {
        return AttemptResultView {
            run_href,
            attempt: attempt_label,
            phase: attempt.step.as_str().to_owned(),
            status: attempt.state.as_label().to_owned(),
            unavailable: true,
            text: String::new(),
            response_html: String::new(),
            thinking: String::new(),
            error: String::new(),
            truncated: false,
            tools: Vec::new(),
        };
    };
    AttemptResultView {
        run_href,
        attempt: attempt_label,
        phase: terminal.phase,
        status: format!(
            "{} · Response {}",
            attempt.state.as_label(),
            terminal.state.label()
        ),
        unavailable: false,
        response_html: crate::markdown::render(&terminal.text),
        text: terminal.text,
        thinking: terminal.thinking,
        error: terminal.error.unwrap_or_default(),
        truncated: terminal.truncated,
        tools: terminal
            .tools
            .into_iter()
            .map(|tool| AttemptToolView {
                label: tool.label,
                output: tool.output,
                truncated: tool.truncated,
            })
            .collect(),
    }
}

pub(super) fn attempt_changes_view(
    run: &WorkflowRun,
    attempt: &crate::workflows::run::AttemptRecord,
    state: &crate::state::AppState,
) -> AttemptChangesView {
    let changes = attempt
        .outputs
        .iter()
        .filter(|output| {
            output.artefact.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
        })
        .filter_map(|output| {
            let record = run.artefact(&output.artefact.id)?;
            let (preview, truncated) = candidate_preview(run, record, state);
            Some(AttemptChangeView {
                href: format!(
                    "/runs/{}/artefacts/{}",
                    run.id.as_hex(),
                    output.artefact.id.as_hex()
                ),
                kind: output.artefact.kind.as_str().to_owned(),
                hash: record
                    .candidate_hash()
                    .map(|hash| hash.short())
                    .unwrap_or_default(),
                preview,
                truncated,
            })
        })
        .collect();
    AttemptChangesView {
        run_href: format!("/runs/{}", run.id.as_hex()),
        attempt: format!("Attempt {} · {}", attempt.ordinal, attempt.step.as_str()),
        changes,
    }
}

fn phase_model_label(
    run: &WorkflowRun,
    step: &crate::workflows::definition::StepDefinition,
) -> String {
    let Some(selection) = run.phase_model(&step.key) else {
        return String::new();
    };
    let effort = selection
        .selection
        .thinking
        .as_ref()
        .map(|effort| format!(" · {}", effort.label()))
        .unwrap_or_default();
    let preset = selection
        .preset
        .as_ref()
        .map(|preset| format!(" · Preset: {}", preset.name))
        .unwrap_or_default();
    format!(
        "{} · {}{}{}",
        selection.selection.provider.label(),
        selection.selection.model,
        effort,
        preset
    )
}

fn step_status(
    run: &WorkflowRun,
    attempt: Option<&crate::workflows::run::AttemptRecord>,
    gate: Option<&crate::workflows::gates::HumanGateRecord>,
) -> &'static str {
    if let Some(gate) = gate {
        return match gate.state {
            crate::workflows::gates::HumanGateState::AwaitingDecision => "Awaiting decision",
            crate::workflows::gates::HumanGateState::Approved => "Approved",
            crate::workflows::gates::HumanGateState::RevisionRequested => "Revision requested",
            crate::workflows::gates::HumanGateState::Cancelled => "Cancelled",
            crate::workflows::gates::HumanGateState::Interrupted => "Interrupted",
        };
    }
    if let Some(attempt) = attempt {
        return attempt.state.as_label();
    }
    match run.state {
        crate::workflows::run::RunState::Completed => "Not needed",
        crate::workflows::run::RunState::RevisionRequested { .. }
        | crate::workflows::run::RunState::Escalated { .. }
        | crate::workflows::run::RunState::Failed
        | crate::workflows::run::RunState::Cancelled
        | crate::workflows::run::RunState::Interrupted => "Not started",
        crate::workflows::run::RunState::InitialisingSource
        | crate::workflows::run::RunState::Ready { .. }
        | crate::workflows::run::RunState::Active { .. }
        | crate::workflows::run::RunState::AwaitingHuman { .. } => "Waiting",
    }
}

fn state_note(state: &crate::workflows::run::RunState) -> &'static str {
    match state {
        crate::workflows::run::RunState::InitialisingSource => {
            "Power Plant captures the authorised reviewed directories before the first step starts."
        }
        crate::workflows::run::RunState::Ready { .. } => {
            "The next step is queued. Apply changes updates host files. An explicit Git commit creates a commit."
        }
        crate::workflows::run::RunState::Active { .. } => {
            "The current step uses an isolated attempt. File application and explicit Git commits update host files."
        }
        crate::workflows::run::RunState::AwaitingHuman { .. } => {
            "The decision covers the exact candidate set. Approval permits the configured file application or explicit Git commit."
        }
        crate::workflows::run::RunState::RevisionRequested { .. } => {
            "The revision request ended this run. Start a new task to continue the work."
        }
        crate::workflows::run::RunState::Escalated { .. } => {
            "Automatic work stopped. Inspect the latest review report before a new task."
        }
        crate::workflows::run::RunState::Completed => "The task is complete.",
        crate::workflows::run::RunState::Failed => {
            "A step failed. Inspect the latest attempt for the failure category."
        }
        crate::workflows::run::RunState::Cancelled => {
            "The run was cancelled. No later steps will start."
        }
        crate::workflows::run::RunState::Interrupted => {
            "The process restarted before this run finished. This run cannot continue. Inspect the recorded attempts. Start new work from a conversation."
        }
    }
}

fn latest_review_verdict(
    run: &WorkflowRun,
    attempt: Option<&crate::workflows::run::AttemptRecord>,
) -> String {
    review_verdict_label(
        attempt
            .into_iter()
            .flat_map(|attempt| &attempt.outputs)
            .filter_map(|output| run.artefact(&output.artefact.id))
            .map(|record| &record.summary),
    )
}

pub(super) fn review_verdict_label<'a>(
    mut summaries: impl Iterator<Item = &'a crate::workflows::artefacts::ArtefactSummary>,
) -> String {
    summaries
        .find_map(|summary| match summary {
            crate::workflows::artefacts::ArtefactSummary::Review { verdict, .. } => {
                Some(verdict.as_label().to_owned())
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn review_route(attempt: Option<&crate::workflows::run::AttemptRecord>) -> String {
    attempt
        .and_then(|attempt| attempt.review_route)
        .map(crate::workflows::run::ReviewRoute::as_label)
        .unwrap_or("")
        .to_owned()
}

fn following_candidate_hash(
    run: &WorkflowRun,
    step: &crate::workflows::definition::StepDefinition,
) -> Option<crate::workflows::artefacts::CandidateHash> {
    let mut next = run.pinned.definition.next_step(&step.key);
    while let Some(key) = next {
        if let Some(candidate) = run
            .attempts
            .iter()
            .rev()
            .find(|attempt| attempt.step == *key)
            .and_then(|attempt| {
                attempt.outputs.iter().find_map(|output| {
                    (output.artefact.kind
                        == crate::workflows::definition::ArtefactKind::CandidateRevision)
                        .then(|| run.artefact(&output.artefact.id)?.candidate_hash())
                        .flatten()
                })
            })
        {
            return Some(candidate);
        }
        next = run.pinned.definition.next_step(key);
    }
    None
}

fn next_candidate_hash(
    run: &WorkflowRun,
    step: &crate::workflows::definition::StepDefinition,
) -> Option<crate::workflows::artefacts::CandidateHash> {
    let next = run.pinned.definition.next_step(&step.key)?;
    let next = run.pinned.definition.step(next)?;
    let candidate = next.inputs.iter().find(|input| {
        input.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
    })?;
    let reference = match &candidate.source {
        crate::workflows::definition::ArtefactSource::RunInitialCandidate => match &run.source {
            crate::workflows::run::RunSource::Captured { source } => &source.initial,
            crate::workflows::run::RunSource::None | crate::workflows::run::RunSource::Pending => {
                return None;
            }
        },
        crate::workflows::definition::ArtefactSource::RunCurrentCandidate => match &run.source {
            crate::workflows::run::RunSource::Captured { source } => &source.accepted,
            crate::workflows::run::RunSource::None | crate::workflows::run::RunSource::Pending => {
                return None;
            }
        },
        crate::workflows::definition::ArtefactSource::RunCurrentPlan => return None,

        crate::workflows::definition::ArtefactSource::StepOutput { step, output } => {
            &run.attempts
                .iter()
                .rev()
                .find(|attempt| attempt.step == *step)?
                .outputs
                .iter()
                .find(|item| item.key == *output)?
                .artefact
        }
    };
    run.artefact(&reference.id)?.candidate_hash()
}

fn step_environment_label(
    run: &WorkflowRun,
    step: &crate::workflows::definition::StepDefinition,
) -> String {
    let set = &run.environments;
    let Some(binding) = set.steps.iter().find(|item| item.step == step.key) else {
        return String::new();
    };
    let Some(environment) = set
        .environments
        .iter()
        .find(|item| item.environment_id == binding.environment_id)
    else {
        return String::new();
    };
    format!(
        "{} · {}",
        environment.name,
        environment.snapshot.snapshot_digest.short_hex()
    )
}

fn pinned_environments(
    run: &WorkflowRun,
    catalogue: &EnvironmentCatalogue,
) -> Vec<PinnedEnvironmentView> {
    run.environments
        .environments
        .iter()
        .map(|environment| {
            let note = match catalogue.get(&environment.environment_id) {
                None => "Environment deleted from catalogue",
                Some(record) if record.ready_preparation != Some(environment.preparation_id) => {
                    "Earlier pinned preparation"
                }
                Some(_) => "",
            };
            PinnedEnvironmentView {
                name: environment.name.clone(),
                note: note.to_owned(),
                preparation: environment.preparation_id.as_hex(),
                recipe: environment.recipe_version.as_hex(),
                snapshot: environment.snapshot.snapshot_digest.as_str().to_owned(),
                image: environment
                    .snapshot
                    .image_manifest_digest
                    .as_str()
                    .to_owned(),
            }
        })
        .collect()
}

fn project_presentation() -> (String, String) {
    (String::new(), "Private workspace".to_owned())
}

fn catalogue_presentation(run: &WorkflowRun, catalogue: &WorkflowCatalogue) -> (String, String) {
    let Some(id) = run.pinned.workflow_id else {
        return (String::new(), String::new());
    };
    match catalogue.get(&id) {
        None => (String::new(), "Workflow deleted from catalogue".to_owned()),
        Some(record) if record.definition_version != run.pinned.version => (
            format!("/workflows/{}/configuration", id.as_hex()),
            "Pinned configuration differs from the saved workflow".to_owned(),
        ),
        Some(_) => (
            format!("/workflows/{}/configuration", id.as_hex()),
            String::new(),
        ),
    }
}

fn artefact_rows(run: &WorkflowRun) -> Vec<ArtefactRow> {
    let observed = run.observed_candidate_hash();
    run.artefacts
        .iter()
        .map(|record| ArtefactRow {
            href: format!("/runs/{}/artefacts/{}", run.id.as_hex(), record.id.as_hex()),
            kind: record.kind.as_str(),
            hash: record.artefact_hash.short(),
            producer: record.provenance.producer.as_label(),
            created: format_time(record.created_at_ms),
            status: record.assurance_label(observed),
        })
        .collect()
}

#[derive(askama::Template)]
#[template(path = "workflow_runs/templates/artefact.html")]
pub(super) struct ArtefactView {
    pub(super) run_href: String,
    pub(super) kind: &'static str,
    pub(super) hash: String,
    pub(super) producer: &'static str,
    pub(super) created: String,
    pub(super) body: String,
    pub(super) content_unavailable: bool,
    pub(super) constraint: String,
    pub(super) preview: String,
    pub(super) truncated: bool,
}

impl ArtefactView {
    pub(super) fn from_record(
        run: &WorkflowRun,
        record: &crate::workflows::artefacts::ArtefactRecord,
        state: &crate::state::AppState,
    ) -> Self {
        let (body, content_unavailable) = match state.workflow_artefacts.get(&record.object_hash) {
            Ok(bytes) => (artefact_body(record.kind, bytes), false),
            Err(_) => (String::new(), true),
        };
        let constraint = record.constraint_label();
        let (preview, truncated) = candidate_preview(run, record, state);
        Self {
            run_href: format!("/runs/{}", run.id.as_hex()),
            kind: record.kind.as_str(),
            hash: record.artefact_hash.as_str(),
            producer: record.provenance.producer.as_label(),
            created: format_time(record.created_at_ms),
            body,
            content_unavailable,
            constraint,
            preview,
            truncated,
        }
    }
}

fn artefact_body(kind: crate::workflows::definition::ArtefactKind, bytes: Vec<u8>) -> String {
    match crate::workflows::artefacts::parse_typed_payload(kind, &bytes) {
        Ok(crate::workflows::artefacts::TypedPayload::Plan(plan)) => {
            crate::markdown::render(&plan.markdown)
        }
        Ok(crate::workflows::artefacts::TypedPayload::Review(report)) => {
            crate::markdown::render(&report.markdown)
        }
        Ok(crate::workflows::artefacts::TypedPayload::Test(report)) => {
            crate::markdown::render(&report.markdown)
        }
        Ok(crate::workflows::artefacts::TypedPayload::HumanDecision(decision)) => {
            let note = decision.note.unwrap_or_default();
            crate::markdown::escape_plain(&format!("{}\n{}", decision.decision.as_label(), note))
        }
        Ok(crate::workflows::artefacts::TypedPayload::PlanDecision(decision)) => {
            let note = decision.note.unwrap_or_default();
            crate::markdown::escape_plain(&format!(
                "{}\n{}\nPlan: {}",
                decision.decision.as_label(),
                note,
                decision.plan
            ))
        }
        Err(_) => match String::from_utf8(bytes) {
            Ok(text) => crate::markdown::escape_plain(&text),
            Err(_) => "Binary candidate manifest".to_owned(),
        },
    }
}

fn candidate_preview(
    run: &WorkflowRun,
    record: &crate::workflows::artefacts::ArtefactRecord,
    state: &crate::state::AppState,
) -> (String, bool) {
    if record.kind != crate::workflows::definition::ArtefactKind::CandidateRevision {
        return (String::new(), false);
    }
    let Ok(after_bytes) = state.workflow_artefacts.get(&record.object_hash) else {
        return (String::new(), false);
    };
    let Some(after) =
        crate::workflows::artefacts::CandidatePayload::from_manifest_bytes(&after_bytes)
    else {
        return (String::new(), false);
    };
    let before = record.provenance.inputs.iter().find_map(|input| {
        if input.kind != crate::workflows::definition::ArtefactKind::CandidateRevision {
            return None;
        }
        let parent = run.artefact(&input.id)?;
        let bytes = state.workflow_artefacts.get(&parent.object_hash).ok()?;
        crate::workflows::artefacts::CandidatePayload::from_manifest_bytes(&bytes)
    });
    let mut text = String::new();
    let mut truncated = false;
    match (before.as_ref(), &after) {
        (
            Some(crate::workflows::artefacts::CandidatePayload::Revision(before)),
            crate::workflows::artefacts::CandidatePayload::Revision(after),
        ) => {
            (text, truncated) = crate::workflows::artefacts::candidate::preview_plain(
                &state.workflow_artefacts,
                &before.entries,
                &after.entries,
            );
        }
        (
            Some(crate::workflows::artefacts::CandidatePayload::Set(before)),
            crate::workflows::artefacts::CandidatePayload::Set(after),
        ) => {
            for (left, right) in before.roots.iter().zip(&after.roots) {
                let (root_text, root_truncated) =
                    crate::workflows::artefacts::candidate::preview_plain(
                        &state.workflow_artefacts,
                        &left.candidate.entries,
                        &right.candidate.entries,
                    );
                if !root_text.is_empty() {
                    text.push_str(&format!("Directory {}\n{}", right.alias, root_text));
                }
                truncated |= root_truncated;
            }
        }
        _ => return (String::new(), false),
    }
    (crate::markdown::escape_plain(&text), truncated)
}

fn candidate_review_href(
    run: &WorkflowRun,
    record: Option<&crate::workflows::artefacts::ArtefactRecord>,
) -> String {
    let Some(record) = record.filter(|record| {
        record.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
    }) else {
        return String::new();
    };
    let Some(base) =
        record.provenance.inputs.iter().find(|input| {
            input.kind == crate::workflows::definition::ArtefactKind::CandidateRevision
        })
    else {
        return String::new();
    };
    format!(
        "/conversations/candidate-review?run={}&candidate={}&diff_base={}",
        run.id.as_hex(),
        record.id.as_hex(),
        base.id.as_hex()
    )
}

fn format_time(ms: u64) -> String {
    let seconds = i64::try_from(ms / 1000).unwrap_or(0);
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
        .unwrap_or_else(|| "unknown".to_owned())
}
