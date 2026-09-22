use crate::{
    conversations::ConversationRecord,
    sessions::{JobStatus, SessionId},
    state::AppState,
    workflows::{WorkflowJob, WorkflowRun},
};

pub(in crate::slices::conversations) fn source_run(
    state: &AppState,
    source: &ConversationRecord,
) -> Option<WorkflowRun> {
    state
        .workflow_runs
        .for_conversation(&source.id)
        .into_iter()
        .find(WorkflowRun::transferable)
}

pub(in crate::slices::conversations) fn draft_run(
    state: &AppState,
    session: SessionId,
    form: &super::super::new::NewForm,
) -> Result<Option<WorkflowRun>, &'static str> {
    let binding = state.handoff_drafts.get(session, &form.draft_nonce);
    if form.prepared_run.is_empty() && binding.is_none() {
        return Ok(None);
    }
    let binding = binding.ok_or(
        "The prepared-change draft expired. Open Handoff in the current owner conversation again.",
    )?;
    let source = state
        .conversations
        .get(&binding.source)
        .ok_or("The source conversation is unavailable.")?;
    let run = state
        .workflow_runs
        .get(&binding.run)
        .ok_or("The prepared changes are unavailable.")?;
    if form.prepared_run != run.id.as_hex()
        || !state.sessions.contains_live(&session)
        || source.revision != binding.source_revision
        || !run.transferable()
        || run.conversation_id != Some(source.id)
        || run.handoff_fingerprint() != binding.fingerprint
    {
        return Err("The source or prepared changes changed. Open Handoff again.");
    }
    if state.sessions.conversation_reserved(source.id)
        && !state.gate_continuations.available(&run.id, &session)
    {
        return Err("Another operation controls the prepared changes. Wait before handoff.");
    }
    Ok(Some(run))
}

pub(in crate::slices::conversations) fn settings_text(run: &WorkflowRun) -> String {
    let phases = run.model_phases().filter_map(|phase| phase.settings.as_ref().map(|settings| {
        let directories = settings.directories.iter().map(|grant| format!("{}: {} ({})", grant.alias, grant.host_path.display(), grant.access.as_str())).collect::<Vec<_>>().join("\n");
        let environment = run.environments.steps.iter().find(|binding| binding.step == phase.step).map(|binding| format!("{} / {}", binding.environment_id.as_hex(), binding.snapshot_digest.as_str())).unwrap_or_else(|| "Host execution".to_owned());
        format!("Step: {}\nModel: {} / {}\nThinking: {}\nTools: {}\nLocation: {}\nHost command policy: {}\nNetwork: {:?}\nEnvironment: {}\nDirectories:\n{}\nInstructions:\n{}", phase.step.as_str(), phase.selection.provider.as_str(), phase.selection.model, phase.selection.thinking.as_ref().map_or("Not available", |effort| effort.as_str()), settings.tools.iter().map(|tool| tool.as_str()).collect::<Vec<_>>().join(", "), settings.location.as_str(), settings.host_approval.as_str(), settings.network, environment, directories, phase.instructions)
    })).collect::<Vec<_>>().join("\n\n");
    let steps = run
        .pinned
        .definition
        .steps()
        .iter()
        .map(|step| {
            format!(
                "{}: {}",
                step.name,
                match &step.action {
                    crate::workflows::definition::StepAction::SystemCommand(action) =>
                        action.command.consequence(),
                    crate::workflows::definition::StepAction::HumanGate(_) =>
                        "An explicit human decision remains separate.",
                    crate::workflows::definition::StepAction::Agent(_) =>
                        "A model call uses its pinned inputs and settings.",
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{phases}\n\nPinned workflow sequence:\n{steps}")
}

pub(in crate::slices::conversations) fn validate_pinned(
    state: &AppState,
    run: &WorkflowRun,
) -> Result<(), &'static str> {
    for phase in run.model_phases() {
        if run.command_message.is_none() {
            crate::workflows::validate_phase_selection(state, &phase.selection)
                .map_err(|_| "Connect each pinned provider before continuation.")?;
        }
        if let Some(settings) = &phase.settings {
            crate::execution::ProjectFreeAuthority::from_settings(1, settings).map_err(|_| "A pinned directory changed identity. The prepared changes remain with their current owner.")?;
        }
    }
    Ok(())
}

pub(in crate::slices::conversations) fn preflight(
    state: &AppState,
    session: SessionId,
    form: &super::super::new::NewForm,
) -> Result<Option<WorkflowRun>, &'static str> {
    let Some(run) = draft_run(state, session, form)? else {
        return Ok(None);
    };
    if form.handoff_approval != "continue-prepared" {
        return Err(
            "Approve the pinned settings for this execution only before you send the handoff.",
        );
    }
    validate_pinned(state, &run)?;
    Ok(Some(run))
}

pub(in crate::slices::conversations) fn finish(
    state: &AppState,
    session: SessionId,
    destination: ConversationRecord,
    form: &super::super::new::NewForm,
    expected: WorkflowRun,
) -> Result<ConversationRecord, &'static str> {
    let source_id = expected
        .conversation_id
        .ok_or("The source owner is unavailable.")?;
    let source = state
        .conversations
        .get(&source_id)
        .ok_or("The source owner is unavailable.")?;
    let binding = state
        .handoff_drafts
        .get(session, &form.draft_nonce)
        .ok_or("The handoff draft expired.")?;
    if binding.source_revision != source.revision
        || binding.fingerprint != expected.handoff_fingerprint()
    {
        return Err("The source changed. Open Handoff again.");
    }
    validate_pinned(state, &expected)?;
    let previous = state.gate_continuations.take(&expected.id);
    let restore = |previous: Option<WorkflowJob>| {
        if let Some(previous) = previous {
            state.gate_continuations.put_back(previous);
        }
    };
    if previous
        .as_ref()
        .is_some_and(|job| job.session_id != session || job.conversation_id != Some(source_id))
        || (previous.is_none() && state.sessions.conversation_reserved(source_id))
    {
        restore(previous);
        return Err("Another operation controls the prepared changes.");
    }
    let job = match state
        .sessions
        .begin_conversation_job(&session, destination.id)
    {
        Ok(job) => job,
        Err(_) => {
            restore(previous);
            return Err(crate::conversations::ConversationError::Active.message());
        }
    };
    let started = match state.conversations.transfer_prepared(
        &state.workflow_runs,
        &expected,
        binding.source_revision,
        &destination,
        job.id(),
        &form.message,
    ) {
        Ok(record) => record,
        Err(_) => {
            state
                .sessions
                .finish_conversation_job(&session, destination.id, job.id());
            restore(previous);
            return Err("The transfer did not commit. The source retains the prepared changes.");
        }
    };
    if let Some(previous) = previous {
        previous.job.finish(JobStatus::Completed, None);
        state
            .sessions
            .finish_conversation_job(&previous.session_id, source_id, previous.job.id());
    }
    let transferred = state
        .workflow_runs
        .get(&expected.id)
        .expect("committed ownership");
    let attached = if transferred.pending_handoff.is_some() {
        Err("Prepared-change ownership needs recovery.")
    } else {
        super::recovery::attach(state, session, &transferred, &started, job.clone())
    };
    if let Err(error) = attached {
        job.finish(JobStatus::Failed, Some(error));
        state
            .sessions
            .finish_conversation_job(&session, destination.id, job.id());
    }
    state.handoff_drafts.remove(session, &form.draft_nonce);
    Ok(started)
}
