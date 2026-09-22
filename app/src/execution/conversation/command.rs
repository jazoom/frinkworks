//! Direct command execution.
//!
//! A direct command is a durable conversation entry, not a model tool call.
//! The submission handler persists the pending entry before this module starts
//! a process, so a restart can never replay a command whose outcome is
//! unknown. Host commands run directly. Sandbox commands reuse the ordinary
//! file-attempt driver for preparation, review, application and cleanup.

use std::{path::PathBuf, sync::Arc};

#[cfg(test)]
mod tests;

use crate::{
    agents::ToolId,
    conversations::{ConversationRecord, MessageId, MessageStatus},
    execution::{
        CommandReporter, CommandResult, CommandTermination, HostCommandDecision,
        HostCommandRequest, OutputKey, OutputScope, ToolLocation,
    },
    sessions::{Job, JobStatus, SessionId},
    state::AppState,
    workflows::{
        DirectCommandWork, PhaseModelSelection, PinnedPreset, RunId, WorkflowJob, WorkflowRun,
    },
};

/// One direct command dispatch. No provider request is made.
pub(crate) struct DirectCommandRun {
    pub(crate) session: SessionId,
    pub(crate) record: ConversationRecord,
    pub(crate) job: Arc<Job>,
    /// The durable command entry identity for retained output provenance.
    pub(crate) message: MessageId,
    pub(crate) command: String,
    /// True for `!`, false for `!!`.
    pub(crate) included: bool,
    pub(crate) directory: PathBuf,
    pub(crate) secret: Option<String>,
    /// Held across the whole command so only one execution is unfinished.
    pub(crate) execution: crate::workflows::ExecutionGuard,
}

pub(crate) async fn run(state: AppState, work: DirectCommandRun) {
    let settings = work
        .record
        .model
        .as_ref()
        .map(|model| model.settings.clone());
    match settings.as_ref().map(|settings| settings.location) {
        Some(ToolLocation::Sandbox) => run_sandbox(state, work).await,
        _ => run_host(state, work).await,
    }
}

async fn run_sandbox(state: AppState, work: DirectCommandRun) {
    let conversation = work.record.id;
    let Some(model) = work.record.model.clone() else {
        settle(
            &state,
            &work,
            None,
            MessageStatus::Failed,
            Some("This conversation has no execution settings.".to_owned()),
            None,
            None,
        );
        return;
    };
    if let Err(message) = validate_sandbox(&state, &work, &model.settings) {
        settle(
            &state,
            &work,
            None,
            MessageStatus::Failed,
            Some(message),
            None,
            None,
        );
        return;
    }
    let project_free = match crate::execution::ProjectFreeAuthority::from_settings(
        work.record.revision,
        &model.settings,
    ) {
        Ok(project_free) => project_free,
        Err(error) => {
            settle(
                &state,
                &work,
                None,
                MessageStatus::Failed,
                Some(error.message().to_owned()),
                None,
                None,
            );
            return;
        }
    };
    let pinned = match crate::workflows::pin_agent_work(&model.settings) {
        Ok(pinned) => pinned,
        Err(error) => {
            settle(
                &state,
                &work,
                None,
                MessageStatus::Failed,
                Some(error.message().to_owned()),
                None,
                None,
            );
            return;
        }
    };
    let environments = match crate::workflows::resolve_environments(
        &pinned.definition,
        &state.environments,
        &state.environment_snapshots,
    )
    .await
    {
        Ok(environments) => environments,
        Err(error) => {
            settle(
                &state,
                &work,
                None,
                MessageStatus::Failed,
                Some(error.message().to_owned()),
                None,
                None,
            );
            return;
        }
    };
    let run_id = match RunId::generate() {
        Ok(run_id) => run_id,
        Err(_) => {
            settle(
                &state,
                &work,
                None,
                MessageStatus::Failed,
                Some("Power Plant could not create a command run identifier.".to_owned()),
                None,
                None,
            );
            return;
        }
    };
    let phase_models = pinned
        .definition
        .steps()
        .iter()
        .filter(|step| {
            matches!(
                &step.action,
                crate::workflows::definition::StepAction::Agent(_)
            )
        })
        .map(|step| PhaseModelSelection {
            step: step.key.clone(),
            selection: model.settings.model.clone(),
            instructions: model.settings.instructions.clone(),
            preset: model.preset.as_ref().map(|preset| PinnedPreset {
                id: preset.id,
                revision: preset.revision,
                name: preset.name.clone(),
            }),
            settings: Some(model.settings.clone()),
        })
        .collect::<Vec<_>>();
    let mut run = WorkflowRun::create_source_free_for_conversation(
        run_id,
        crate::workflows::now_ms(),
        conversation,
        pinned,
        environments,
        phase_models,
    );
    run.launch_brief = if work.included {
        work.command.clone()
    } else {
        "Direct command excluded from model context.".to_owned()
    };
    run.command_message = Some(work.message);
    if state.workflow_runs.create(run).is_err() {
        settle(
            &state,
            &work,
            None,
            MessageStatus::Failed,
            Some("Power Plant could not store the command run.".to_owned()),
            None,
            None,
        );
        return;
    }
    // A direct command needs no provider connection. The value here only feeds
    // redaction and the active-connection fallback.
    let connection = crate::providers::ProviderConnection::with_key(
        model.settings.model.provider,
        work.secret.clone().unwrap_or_default(),
        model.settings.model.model.clone(),
    );
    let job = WorkflowJob {
        run_id,
        session_id: work.session,
        agent_id: None,
        agent_revision: work.record.revision,
        conversation_id: Some(conversation),
        authority: None,
        project_free_authority: Some(project_free.clone()),
        grant_alias: String::new(),
        connection,
        phase_providers: Vec::new(),
        active_connection: std::sync::Arc::new(std::sync::Mutex::new(None)),
        host_policy: project_free.policy.clone(),
        turns: Vec::new(),
        job: work.job.clone(),
        eligible_reply: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
        command: Some(DirectCommandWork {
            message: work.message,
            command: work.command.clone(),
            included: work.included,
            settlement: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }),
    };
    crate::workflows::drive_ordinary_file_run(state, job, work.execution).await;
}

/// Revalidate consent, authority and the active job immediately before sandbox
/// preparation. A change since submission rejects the command without effects.
fn validate_sandbox(
    state: &AppState,
    work: &DirectCommandRun,
    settings: &crate::execution::ExecutionSettings,
) -> Result<(), String> {
    if settings.location != ToolLocation::Sandbox {
        return Err("Direct commands need a ready sandbox selection.".to_owned());
    }
    if !settings.tools.contains(&ToolId::Run) {
        return Err("The Run capability is not enabled for this conversation.".to_owned());
    }
    if work.job.cancel_requested() || !state.sessions.contains_live(&work.session) {
        return Err(
            "Sandbox access expired or changed. Approve the current settings again.".to_owned(),
        );
    }
    if state.sandboxes.missing().is_some() {
        return Err("The sandbox runtime is not available.".to_owned());
    }
    let record = state
        .conversations
        .get(&work.record.id)
        .ok_or_else(|| "This conversation is not available.".to_owned())?;
    if record.active_job != Some(work.job.id())
        || record
            .model
            .as_ref()
            .is_none_or(|model| model.settings != *settings)
    {
        return Err("This command is no longer active.".to_owned());
    }
    for grant in &settings.directories {
        grant
            .revalidate()
            .map_err(|_| "A work location changed or is not available.".to_owned())?;
        if grant.requires_access_consent(state.local_data.root())
            && !state.access_consent.authorised_conversation(
                work.session,
                work.record.id,
                settings,
                grant,
            )
            && !state
                .conversations
                .directory_approved(&work.record.id, settings, grant)
        {
            return Err("Directory access needs explicit approval.".to_owned());
        }
    }
    Ok(())
}

async fn run_host(state: AppState, work: DirectCommandRun) {
    let conversation = work.record.id;
    let Some(settings) = work
        .record
        .model
        .as_ref()
        .map(|model| model.settings.clone())
    else {
        settle(
            &state,
            &work,
            None,
            MessageStatus::Failed,
            Some("This conversation has no execution settings.".to_owned()),
            None,
            None,
        );
        return;
    };
    if let Err(message) = validate(&state, &work, &settings) {
        settle(
            &state,
            &work,
            None,
            MessageStatus::Failed,
            Some(message),
            None,
            None,
        );
        return;
    }
    // Capture the baseline before the process can write anything. A capture
    // failure blocks dispatch so no unreviewable change can occur.
    let before = match crate::workflows::capture_ordinary_command_evidence(&state, &settings) {
        Ok(before) => before,
        Err(message) => {
            settle(
                &state,
                &work,
                None,
                MessageStatus::Failed,
                Some(message.to_owned()),
                None,
                None,
            );
            return;
        }
    };
    if let Some(baseline) = &before
        && state
            .conversations
            .record_command_baseline(&conversation, work.job.id(), baseline.clone())
            .is_err()
    {
        let _ = work.job.finish(
            JobStatus::Failed,
            Some("Power Plant could not store the command baseline. No command ran."),
        );
        return;
    }
    let secret = work.secret.as_deref();
    let mut request = HostCommandRequest {
        token: String::new(),
        session: work.session,
        job: work.job.id(),
        conversation,
        execution_revision: work.record.revision,
        command: work.command.clone(),
        directory: work.directory.clone(),
        explanation: "Direct command from the conversation.".to_owned(),
        run: None,
        step: None,
        attempt: None,
    };
    if settings.automatic_host_commands() {
        request.token = match crate::execution::command_token() {
            Ok(token) => token,
            Err(error) => {
                settle(
                    &state,
                    &work,
                    None,
                    MessageStatus::Failed,
                    Some(error.message().to_owned()),
                    before,
                    None,
                );
                return;
            }
        };
    } else {
        // Typed command syntax is not approval. Ask each time uses the same
        // token store and decision route as a model tool command.
        let approvals = &state.host_approvals;
        let token = match approvals.submit(request.clone()) {
            Ok(token) => token,
            Err(error) => {
                settle(
                    &state,
                    &work,
                    None,
                    MessageStatus::Failed,
                    Some(error.message().to_owned()),
                    before,
                    None,
                );
                return;
            }
        };
        request.token = token.clone();
        if work.job.set_awaiting_decision().is_none() && work.job.cancel_requested() {
            approvals.invalidate_job(work.job.id());
            settle(
                &state,
                &work,
                None,
                MessageStatus::Interrupted,
                None,
                before,
                None,
            );
            return;
        }
        let decision = approvals.wait(&token, &work.job).await;
        let _ = work.job.resume();
        match decision {
            Ok(HostCommandDecision::Approved) => {}
            Ok(HostCommandDecision::Rejected) => {
                settle(
                    &state,
                    &work,
                    None,
                    MessageStatus::Failed,
                    Some("The user rejected this command.".to_owned()),
                    before,
                    final_evidence(&state, &settings),
                );
                return;
            }
            Err(error) => {
                let status = if work.job.cancel_requested()
                    || error == crate::execution::ApprovalError::Cancelled
                {
                    MessageStatus::Interrupted
                } else {
                    MessageStatus::Failed
                };
                settle(
                    &state,
                    &work,
                    None,
                    status,
                    (status == MessageStatus::Failed).then(|| error.message().to_owned()),
                    before,
                    final_evidence(&state, &settings),
                );
                return;
            }
        }
    }
    if let Err(error) = validate(&state, &work, &settings) {
        settle(
            &state,
            &work,
            None,
            MessageStatus::Failed,
            Some(error),
            before,
            None,
        );
        return;
    }
    let visible_call = work.message.as_hex();
    let output_key = OutputKey {
        scope: OutputScope::conversation(conversation),
        job: work.job.id(),
        tool_call: visible_call.clone(),
        // `!!` output is never model-visible, even through `read_output`.
        model_hidden: !work.included,
    };
    let reporter = CommandReporter {
        tool_call: &visible_call,
        secret,
        outputs: &state.outputs,
        output_key,
    };
    let result = crate::execution::run_shell_reported(
        &work.command,
        &work.directory,
        &work.job,
        crate::execution::COMMAND_TIMEOUT,
        false,
        &reporter,
    )
    .await;
    let after = match crate::workflows::capture_ordinary_command_evidence(&state, &settings) {
        Ok(after) => after,
        Err(error) => {
            let output = match result {
                Ok(output) => output,
                Err(failure) => failure.result,
            }
            .redacted(secret);
            settle(
                &state,
                &work,
                Some(output),
                MessageStatus::Failed,
                Some(error.to_owned()),
                before,
                None,
            );
            return;
        }
    };
    match result {
        Ok(command) => {
            let command = command.redacted(secret);
            settle(
                &state,
                &work,
                Some(command),
                MessageStatus::Complete,
                None,
                before,
                after,
            );
        }
        Err(failure) => {
            let command = failure.result.redacted(secret);
            let status = if command.termination == CommandTermination::Cancelled {
                MessageStatus::Interrupted
            } else {
                MessageStatus::Failed
            };
            settle(
                &state,
                &work,
                Some(command),
                status,
                (status == MessageStatus::Failed).then(|| failure.message.to_owned()),
                before,
                after,
            );
        }
    }
}

fn final_evidence(
    state: &AppState,
    settings: &crate::execution::ExecutionSettings,
) -> Option<String> {
    crate::workflows::capture_ordinary_command_evidence(state, settings)
        .ok()
        .flatten()
}

/// Revalidate consent, authority and the active job immediately before
/// dispatch. A change since submission rejects the command without effects.
fn validate(
    state: &AppState,
    work: &DirectCommandRun,
    settings: &crate::execution::ExecutionSettings,
) -> Result<(), String> {
    if settings.location != ToolLocation::Host {
        return Err(
            "Direct commands need host selection. Choose Host in Settings and approve access."
                .to_owned(),
        );
    }
    if !settings.host_tools() {
        return Err("Enable host tools and run capability before a direct command.".to_owned());
    }
    if !settings.tools.contains(&ToolId::Run) {
        return Err("The Run capability is not enabled for this conversation.".to_owned());
    }
    if work.job.cancel_requested() || !state.sessions.contains_live(&work.session) {
        return Err(
            "Host access expired or changed. Approve the current settings again.".to_owned(),
        );
    }
    if !state
        .access_consent
        .authorised_host_conversation(work.session, work.record.id, settings)
    {
        return Err(
            "Unrestricted host access needs explicit approval. Open Settings to approve it."
                .to_owned(),
        );
    }
    let record = state
        .conversations
        .get(&work.record.id)
        .ok_or_else(|| "This conversation is not available.".to_owned())?;
    if record.active_job != Some(work.job.id())
        || record
            .model
            .as_ref()
            .is_none_or(|model| model.settings != *settings)
    {
        return Err("This command is no longer active.".to_owned());
    }
    for grant in &settings.directories {
        grant
            .revalidate()
            .map_err(|_| "A work location changed or is not available.".to_owned())?;
    }
    Ok(())
}

/// Persist the settled command entry before the job ends. A failed write keeps
/// the pending entry, so restart never repeats the process.
pub(crate) fn settle(
    state: &AppState,
    work: &DirectCommandRun,
    output: Option<CommandResult>,
    status: MessageStatus,
    error: Option<String>,
    before: Option<String>,
    after: Option<String>,
) {
    let conversation = work.record.id;
    if state
        .conversations
        .settle_command(
            &conversation,
            work.job.id(),
            output,
            status,
            error.clone(),
            before,
            after,
        )
        .is_err()
    {
        // The process already ran. Keep the pending entry and the job failed so
        // a restart reports an uncertain outcome instead of replaying it.
        let _ = work.job.finish(
            JobStatus::Failed,
            Some("Power Plant could not store the command outcome. Restart before you continue."),
        );
        return;
    }
    let job_status = match status {
        MessageStatus::Complete => JobStatus::Completed,
        MessageStatus::Interrupted => JobStatus::Cancelled,
        MessageStatus::Failed | MessageStatus::Pending => JobStatus::Failed,
    };
    work.job.finish(job_status, error.as_deref());
    state
        .sessions
        .finish_conversation_job(&work.session, conversation, work.job.id());
}
