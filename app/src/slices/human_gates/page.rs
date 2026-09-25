use askama::Template;

use crate::workflows::artefacts::diff::{CandidateDiff, MANIFEST_PAGE_SIZE, TEXT_PAGE_FRAGMENTS};
use crate::workflows::gates::HumanGateRecord;
use crate::workflows::{RunKind, WorkflowRun};

pub(super) const TITLE: &str = "Human gate | Frinkworks";

pub(super) struct ChangeRow {
    pub(super) directory: String,
    pub(super) path: String,
    pub(super) status: &'static str,
    pub(super) old: String,
    pub(super) new: String,
    pub(super) href: String,
    pub(super) base_download: String,
    pub(super) target_download: String,
    pub(super) additions: usize,
    pub(super) removals: usize,
    pub(super) has_counts: bool,
    pub(super) selected: bool,
}

pub(super) struct TextRow {
    pub(super) text: String,
    pub(super) range: String,
    pub(super) continued: bool,
}

#[derive(Template)]
#[template(path = "human_gates/templates/detail.html")]
pub(super) struct GatePage {
    pub(super) run_id: String,
    pub(super) plan_gate: bool,
    pub(super) plan_text: String,
    pub(super) plan_bytes: u64,
    pub(super) gate_id: String,
    pub(super) gate_name: String,
    pub(super) position: usize,
    pub(super) state: &'static str,
    pub(super) base: String,
    pub(super) target: String,
    pub(super) review_href: String,
    pub(super) revision: u64,
    pub(super) total: usize,
    pub(super) range: String,
    pub(super) changes: Vec<ChangeRow>,
    pub(super) previous: String,
    pub(super) next: String,
    pub(super) selected_path: String,
    pub(super) text: Vec<TextRow>,
    pub(super) text_previous: String,
    pub(super) text_next: String,
    pub(super) binary: bool,
    pub(super) text_too_large: bool,
    pub(super) preview_unavailable: bool,
    pub(super) awaiting: bool,
    pub(super) needs_recovery: bool,
    pub(super) error: &'static str,
    pub(super) run_kind: &'static str,
    pub(super) project_id: String,
    pub(super) back_href: String,
    pub(super) back_label: &'static str,
    pub(super) quick_task: bool,
    pub(super) can_request_revision: bool,
    pub(super) revision_target: String,
    pub(super) revision_attempt_limit: u8,
    pub(super) host_unchanged: &'static str,
    pub(super) commit_on_approval: bool,
    pub(super) apply_on_approval: bool,
    pub(super) exclusions: Vec<String>,
    pub(super) application_destination: String,
}

impl GatePage {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        run: &WorkflowRun,
        gate: &HumanGateRecord,
        diff: Option<CandidateDiff>,
        plan_text: Option<String>,
        store: &crate::workflows::WorkflowArtefactRepository,
        query: super::forms::DiffQuery,
        error: &'static str,
        application_destination: String,
    ) -> Option<Self> {
        let plan_gate = gate.candidate.kind == crate::workflows::definition::ArtefactKind::Plan;
        let root = format!("/runs/{}/gates/{}", run.id.as_hex(), gate.id.as_hex());
        let (start, total, end, changes) = if let Some(diff) = diff.as_ref() {
            let start = query.page.checked_mul(MANIFEST_PAGE_SIZE)?;
            let (total, page) = diff.manifest_page(start, MANIFEST_PAGE_SIZE).ok()?;
            let end = start + page.len();
            let changes = page
                .iter()
                .enumerate()
                .map(|(offset, change)| {
                    let index = start + offset;
                    // Counts derive from the complete stored diff for the
                    // row. Binary or oversized changes omit counts.
                    let (additions, removals, has_counts) = diff
                        .change(index, store)
                        .ok()
                        .and_then(|full| full.text)
                        .map(|fragments| {
                            let text: String = fragments
                                .into_iter()
                                .map(|fragment| fragment.text)
                                .collect();
                            let mut additions = 0;
                            let mut removals = 0;
                            for line in text.lines() {
                                if line.starts_with('+') && !line.starts_with("+++") {
                                    additions += 1;
                                } else if line.starts_with('-') && !line.starts_with("---") {
                                    removals += 1;
                                }
                            }
                            (additions, removals, true)
                        })
                        .unwrap_or((0, 0, false));
                    ChangeRow {
                        directory: change.directory.clone(),
                        path: change.path.clone(),
                        status: change.status,
                        old: facts(change.old.as_ref()),
                        new: facts(change.new.as_ref()),
                        href: format!("{root}?page={}&change={index}", query.page),
                        base_download: download(&root, "base", index, change.old.as_ref()),
                        target_download: download(&root, "target", index, change.new.as_ref()),
                        additions,
                        removals,
                        has_counts,
                        selected: query.change.unwrap_or(start) == index,
                    }
                })
                .collect();
            (start, total, end, changes)
        } else {
            (0, 0, 0, Vec::new())
        };
        let mut selected_path = String::new();
        let mut text = Vec::new();
        let mut text_previous = String::new();
        let mut text_next = String::new();
        let mut binary = false;
        let mut text_too_large = false;
        let mut preview_unavailable = false;
        if let Some(index) = query.change.or_else(|| (total > start).then_some(start)) {
            let diff = diff.as_ref()?;
            // Validate selection against the manifest, independently of object availability.
            let (_, entries) = diff.manifest_page(index, 1).ok()?;
            let entry = entries.first()?;
            selected_path = if entry.directory.is_empty() {
                entry.path.clone()
            } else {
                format!("{}/{}", entry.directory, entry.path)
            };
            let change = diff.change(index, store).ok();
            preview_unavailable = change.is_none();
            binary = change.as_ref().is_some_and(|change| change.binary);
            text_too_large = change.as_ref().is_some_and(|change| change.text_too_large);
            if let Some(fragments) = change.as_ref().and_then(|change| change.text.as_ref()) {
                let line_start = query.line.checked_mul(TEXT_PAGE_FRAGMENTS)?;
                if line_start > fragments.len() {
                    return None;
                }
                let line_end = (line_start + TEXT_PAGE_FRAGMENTS).min(fragments.len());
                text = fragments[line_start..line_end]
                    .iter()
                    .map(|fragment| TextRow {
                        text: fragment.text.clone(),
                        range: format!("bytes {}–{}", fragment.start, fragment.end),
                        continued: fragment.continued,
                    })
                    .collect();
                if query.line > 0 {
                    text_previous = format!(
                        "{root}?page={}&change={index}&line={}",
                        query.page,
                        query.line - 1
                    );
                }
                if line_end < fragments.len() {
                    text_next = format!(
                        "{root}?page={}&change={index}&line={}",
                        query.page,
                        query.line + 1
                    );
                }
            }
        }
        let previous = if query.page > 0 {
            format!("{root}?page={}", query.page - 1)
        } else {
            String::new()
        };
        let next = if end < total {
            format!("{root}?page={}", query.page + 1)
        } else {
            String::new()
        };
        let quick_task = run.kind == RunKind::QuickTask;
        let exclusions = diff
            .as_ref()
            .map(|diff| diff.exclusions().to_vec())
            .unwrap_or_default();
        let revision_policy = run.human_revision_policy(&gate.step);
        let can_request_revision = revision_policy.is_some()
            && (run.kind != RunKind::QuickTask || run.conversation_id.is_some());
        Some(Self {
            run_id: run.id.as_hex(),
            plan_gate,
            plan_text: plan_text.unwrap_or_default(),
            plan_bytes: if plan_gate {
                run.artefact(&gate.candidate.id)
                    .map(|record| record.payload_bytes)
                    .unwrap_or(0)
            } else {
                0
            },
            gate_id: gate.id.as_hex(),
            gate_name: run.pinned.definition.step(&gate.step)?.name.clone(),
            position: run
                .pinned
                .definition
                .steps()
                .iter()
                .position(|step| step.key == gate.step)?
                + 1,
            state: state_label(gate.state),
            base: if plan_gate {
                gate.candidate.artefact_hash.as_str()
            } else {
                diff.as_ref()?.base.as_str()
            },
            target: if plan_gate {
                gate.candidate.artefact_hash.as_str()
            } else {
                diff.as_ref()?.target.as_str()
            },
            review_href: format!(
                "/conversations/candidate-review?run={}&candidate={}&diff_base={}",
                run.id.as_hex(),
                gate.candidate.id.as_hex(),
                gate.diff_base.id.as_hex()
            ),
            revision: run.decision_revision(gate).get(),
            total,
            range: if start == end {
                "No changed paths".to_owned()
            } else {
                format!("Paths {}–{}", start + 1, end)
            },
            changes,
            previous,
            next,
            selected_path,
            text,
            text_previous,
            text_next,
            binary,
            text_too_large,
            preview_unavailable,
            awaiting: gate.state == crate::workflows::gates::HumanGateState::AwaitingDecision,
            needs_recovery: false,
            error,
            run_kind: run.kind.as_str(),
            project_id: String::new(),
            back_href: run.conversation_id.map_or_else(
                || "/runs".to_owned(),
                |conversation| format!("/conversations/{}", conversation.as_hex()),
            ),
            back_label: if run.conversation_id.is_some() {
                "Back to conversation"
            } else {
                "Back to runs"
            },
            quick_task,
            can_request_revision,
            revision_target: revision_policy
                .map(|policy| policy.revision_target.as_str().to_owned())
                .unwrap_or_default(),
            revision_attempt_limit: revision_policy.map_or(0, |policy| policy.attempt_limit),
            host_unchanged: if run.has_direct_writes() {
                "This review covers only isolated changes. Direct host changes remain after discard, cancellation and environment changes."
            } else {
                crate::workflows::HOST_UNCHANGED
            },
            exclusions,
            application_destination,
            commit_on_approval: super::approval_command(run, gate)
                == Some(crate::workflows::commands::SystemCommandId::CommitCandidate),
            apply_on_approval: super::approval_command(run, gate)
                == Some(crate::workflows::commands::SystemCommandId::ApplyChanges),
        })
    }
}

fn facts(value: Option<&crate::workflows::artefacts::diff::EntryFacts>) -> String {
    value
        .map(|facts| {
            let mode = if facts.executable {
                "executable"
            } else {
                "not executable"
            };
            let bytes = facts
                .bytes
                .map(|bytes| format!(" · {bytes} bytes"))
                .unwrap_or_default();
            let object = facts
                .object
                .map(|hash| format!(" · {}", hash.as_str()))
                .unwrap_or_default();
            let detail = if facts.detail.is_empty() {
                String::new()
            } else {
                format!(" · {}", facts.detail)
            };
            format!("{} · {mode}{bytes}{object}{detail}", facts.kind)
        })
        .unwrap_or_else(|| "Absent".to_owned())
}

fn download(
    root: &str,
    side: &str,
    index: usize,
    value: Option<&crate::workflows::artefacts::diff::EntryFacts>,
) -> String {
    value
        .filter(|facts| facts.object.is_some())
        .map(|_| format!("{root}/objects/{side}/{index}"))
        .unwrap_or_default()
}

fn state_label(state: crate::workflows::gates::HumanGateState) -> &'static str {
    match state {
        crate::workflows::gates::HumanGateState::AwaitingDecision => "Awaiting decision",
        crate::workflows::gates::HumanGateState::Approved => "Approved",
        crate::workflows::gates::HumanGateState::RevisionRequested => "Revision requested",
        crate::workflows::gates::HumanGateState::Cancelled => "Cancelled",
        crate::workflows::gates::HumanGateState::Interrupted => "Interrupted",
    }
}
