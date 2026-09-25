//! The pending entry precedes dispatch. Restart never replays a command with an unknown outcome.

use std::{path::PathBuf, sync::Arc};

use crate::{
    agents::ToolId,
    conversations::{ConversationRecord, MessageId, MessageStatus},
    execution::{
        CommandReporter, CommandResult, CommandTermination, HostCommandRequest, OutputKey,
        OutputScope, ToolLocation,
    },
    sessions::{Job, JobStatus, SessionId},
    state::AppState,
};

pub(crate) struct DirectCommandRun {
    pub(crate) session: SessionId,
    pub(crate) record: ConversationRecord,
    pub(crate) job: Arc<Job>,
    pub(crate) message: MessageId,
    pub(crate) command: String,
    pub(crate) included: bool,
    pub(crate) directory: PathBuf,
    pub(crate) secret: Option<String>,
    pub(crate) execution: crate::workflows::ExecutionGuard,
}

pub(crate) async fn run(state: AppState, work: DirectCommandRun) {
    let Some(settings) = work.record.model.as_ref().map(|model| &model.settings) else {
        settle(
            &state,
            &work,
            None,
            MessageStatus::Failed,
            Some("This conversation has no execution settings.".to_owned()),
        );
        return;
    };
    let result = execute(&state, &work, settings).await;
    match result {
        Ok(output) => settle(&state, &work, Some(output), MessageStatus::Complete, None),
        Err((output, error)) => {
            let cancelled = work.job.cancel_requested()
                || output
                    .as_ref()
                    .is_some_and(|output| output.termination == CommandTermination::Cancelled);
            settle(
                &state,
                &work,
                output,
                if cancelled {
                    MessageStatus::Interrupted
                } else {
                    MessageStatus::Failed
                },
                Some(error),
            );
        }
    }
}

async fn execute(
    state: &AppState,
    work: &DirectCommandRun,
    settings: &crate::execution::ExecutionSettings,
) -> Result<CommandResult, (Option<CommandResult>, String)> {
    validate(state, work, settings).map_err(|error| (None, error))?;
    let mut request = HostCommandRequest {
        token: String::new(),
        session: work.session,
        job: work.job.id(),
        conversation: work.record.id,
        execution_revision: work.record.revision,
        command: work.command.clone(),
        directory: work.directory.clone(),
        explanation: "Direct command from the conversation.".to_owned(),
        run: None,
        step: None,
        attempt: None,
    };
    crate::execution::approval::approve_command(
        state,
        &mut request,
        settings,
        &work.job,
        work.secret.as_deref(),
    )
    .await
    .map_err(|error| (None, error.to_owned()))?;
    validate(state, work, settings).map_err(|error| (None, error))?;
    state
        .workflow_evidence
        .host_command(&request, "dispatching", "", work.secret.as_deref(), None)
        .map_err(|error| (None, error.message().to_owned()))?;
    let visible_call = work.message.as_hex();
    let key = OutputKey {
        scope: OutputScope::conversation(work.record.id),
        job: work.job.id(),
        tool_call: visible_call.clone(),
        model_hidden: !work.included,
    };
    let result = match settings.location {
        ToolLocation::Host => {
            let reporter = CommandReporter {
                tool_call: &visible_call,
                secret: work.secret.as_deref(),
                outputs: &state.outputs,
                output_key: key,
            };
            crate::execution::run_shell_reported(
                &work.command,
                &work.directory,
                &work.job,
                crate::execution::COMMAND_TIMEOUT,
                false,
                &reporter,
            )
            .await
        }
        ToolLocation::Sandbox => {
            let guest = super::prepare_sandbox(
                state,
                work.record.id,
                work.job.id(),
                settings,
                work.secret.as_deref(),
            )
            .await
            .map_err(|error| (None, error.error))?;
            let result = match validate(state, work, settings) {
                Ok(()) => {
                    crate::execution::command::capture_sandbox_command(
                        &guest.sandbox,
                        crate::sandbox::GuestExec::shell(&work.command)
                            .in_dir(guest.policy.primary_guest()),
                        &work.job,
                        work.secret.as_deref(),
                        crate::tools::SANDBOX_COMMAND_TIMEOUT,
                        &visible_call,
                        Some((&state.outputs, &key)),
                    )
                    .await
                }
                Err(error) => {
                    let sandbox_gone =
                        super::dispose_guest(state, &guest.sandbox, guest.attempt).await;
                    let workspace_gone = sandbox_gone && guest.workspace.destroy().is_ok();
                    if state
                        .conversation_runtime
                        .finish(work.record.id, !sandbox_gone, !workspace_gone)
                        .is_err()
                        || !sandbox_gone
                        || !workspace_gone
                    {
                        work.execution.require_recovery();
                    }
                    return Err((None, error));
                }
            };
            let sandbox_gone = super::dispose_guest(state, &guest.sandbox, guest.attempt).await;
            let workspace_gone = sandbox_gone && guest.workspace.destroy().is_ok();
            if state
                .conversation_runtime
                .finish(work.record.id, !sandbox_gone, !workspace_gone)
                .is_err()
                || !sandbox_gone
                || !workspace_gone
            {
                work.execution.require_recovery();
                let output = match result {
                    Ok(output) => output,
                    Err(error) => error.result,
                };
                return Err((Some(output), "Frinkworks cannot remove the command sandbox. Restart before the next command.".to_owned()));
            }
            result
        }
    };
    let output = match &result {
        Ok(output) => output,
        Err(failure) => &failure.result,
    };
    state
        .workflow_evidence
        .host_command(
            &request,
            if output.is_success() {
                "completed"
            } else {
                "failed"
            },
            &output.report(),
            work.secret.as_deref(),
            Some(output),
        )
        .map_err(|error| {
            (
                Some(output.redacted(work.secret.as_deref())),
                error.message().to_owned(),
            )
        })?;
    match result {
        Ok(output) => Ok(output.redacted(work.secret.as_deref())),
        Err(error) => Err((
            Some(error.result.redacted(work.secret.as_deref())),
            error.message.to_owned(),
        )),
    }
}

fn validate(
    state: &AppState,
    work: &DirectCommandRun,
    settings: &crate::execution::ExecutionSettings,
) -> Result<(), String> {
    if !settings.tools.contains(&ToolId::Run) {
        return Err("The Run tool is not enabled for this conversation.".to_owned());
    }
    if work.job.cancel_requested() || !state.sessions.contains_live(&work.session) {
        return Err("Command access expired or changed.".to_owned());
    }
    let record = state
        .conversations
        .get(&work.record.id)
        .ok_or("This conversation is not available.")?;
    if record.active_job != Some(work.job.id())
        || record
            .model
            .as_ref()
            .is_none_or(|model| model.settings != *settings)
    {
        return Err("This command is no longer active.".to_owned());
    }
    if settings.location == ToolLocation::Host {
        if !settings.host_access_allowed()
            || !state.access_consent.authorised_host_conversation(
                work.session,
                work.record.id,
                settings,
            )
        {
            return Err(
                "Host access needs explicit approval. Read access requires a sandbox.".to_owned(),
            );
        }
    } else if state.sandboxes.missing().is_some() {
        return Err("The sandbox runtime is not available.".to_owned());
    }
    for grant in &settings.directories {
        grant
            .revalidate()
            .map_err(|error| error.message().to_owned())?;
        if settings.location == ToolLocation::Sandbox
            && grant.requires_access_consent(state.local_data.root())
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

fn settle(
    state: &AppState,
    work: &DirectCommandRun,
    output: Option<CommandResult>,
    status: MessageStatus,
    error: Option<String>,
) {
    if state.conversation_runtime.unsettled(work.record.id) {
        work.execution.require_recovery();
    }
    // An absent result reaches settlement only before command dispatch.
    let output =
        output.unwrap_or_else(|| CommandResult::new(Vec::new(), CommandTermination::NotDispatched));
    if state
        .conversations
        .settle_command(
            &work.record.id,
            work.job.id(),
            Some(output),
            status,
            error.clone().filter(|_| status == MessageStatus::Failed),
        )
        .is_err()
    {
        work.execution.require_recovery();
        let _ = work.job.finish(
            JobStatus::Failed,
            Some("Frinkworks cannot store the command outcome. Restart before the next command."),
        );
        return;
    }
    let status = match status {
        MessageStatus::Complete => JobStatus::Completed,
        MessageStatus::Interrupted => JobStatus::Cancelled,
        MessageStatus::Failed | MessageStatus::Pending => JobStatus::Failed,
    };
    work.job.finish(status, error.as_deref());
    state
        .sessions
        .finish_conversation_job(&work.session, work.record.id, work.job.id());
}

#[cfg(test)]
mod tests;
