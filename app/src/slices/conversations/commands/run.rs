//! Direct command submission for saved conversations and unsaved drafts.
//!
//! The pending entry precedes process creation. No provider connection is required.

use std::path::PathBuf;

use hypergraft::PatchStatus;

use crate::{
    agents::ToolId,
    conversations::{
        AttachmentId, ConversationError, ConversationModelConfiguration, ConversationRecord,
        DirectCommand, MessageRole, normalise_command,
    },
    execution::ToolLocation,
    sessions::SessionId,
    state::AppState,
};

use super::super::{
    StartMessageError, preflight_execution, provider_secret, spawn_conversation_work, status_for,
};

/// Bound and normalise one typed command body. An empty command leaves the
/// draft untouched.
pub(in crate::slices::conversations) fn command_body(
    command: &str,
) -> Result<String, StartMessageError> {
    normalise_command(command).map_err(|_| {
        StartMessageError::User(
            PatchStatus::UnprocessableEntity,
            "Enter a command after ! or !!.",
        )
    })
}

pub(in crate::slices::conversations) fn reject_attachments(
    attachments: &[AttachmentId],
) -> Result<(), StartMessageError> {
    if attachments.is_empty() {
        return Ok(());
    }
    Err(StartMessageError::User(
        PatchStatus::UnprocessableEntity,
        "Direct commands do not accept images. Remove the images or send a model message.",
    ))
}

pub(in crate::slices::conversations) fn command_directory(
    model: &ConversationModelConfiguration,
) -> Result<(PathBuf, String), StartMessageError> {
    let settings = &model.settings;
    if settings.location != ToolLocation::Host {
        return Err(StartMessageError::User(
            PatchStatus::UnprocessableEntity,
            "Direct commands need host selection. Choose Host in Settings and approve access.",
        ));
    }
    if !settings.host_tools() || !settings.tools.contains(&ToolId::Run) {
        return Err(StartMessageError::User(
            PatchStatus::UnprocessableEntity,
            "Enable host tools and the Run capability before a direct command.",
        ));
    }
    let directory = crate::execution::command_directory(&settings.directories);
    let text = directory.to_string_lossy().into_owned();
    Ok((directory, text))
}

/// Start one direct command on an existing conversation. The pending entry is
/// durable before the process starts.
pub(crate) async fn start_saved(
    state: &AppState,
    session: SessionId,
    record: &ConversationRecord,
    revision: u32,
    model: ConversationModelConfiguration,
    direct: DirectCommand,
    attachments: Vec<AttachmentId>,
) -> Result<ConversationRecord, StartMessageError> {
    reject_attachments(&attachments)?;
    let command = command_body(&direct.command)?;
    let (directory, directory_text) = command_directory(&model)?;
    if record.revision != revision {
        return Err(StartMessageError::User(
            PatchStatus::Conflict,
            "Reload the conversation and try again.",
        ));
    }
    if record.candidate_review_context.is_some()
        || record
            .forked_from
            .as_ref()
            .is_some_and(|source| source.candidate_review)
    {
        return Err(StartMessageError::User(
            PatchStatus::Conflict,
            "Candidate reviews use immutable evidence only. Use a separate conversation for file access.",
        ));
    }
    if record.active_job.is_some()
        || record.continuation.is_some()
        || !record.queue.items.is_empty()
    {
        return Err(StartMessageError::User(
            PatchStatus::Conflict,
            ConversationError::Active.message(),
        ));
    }
    preflight_execution(state, session, Some(record.id), &model).await?;
    let execution = state
        .workflow_execution
        .acquire()
        .map_err(|error| StartMessageError::User(PatchStatus::Conflict, error))?;
    let job = state
        .sessions
        .begin_conversation_job(&session, record.id)
        .map_err(|_| {
            StartMessageError::User(PatchStatus::Conflict, ConversationError::Active.message())
        })?;
    let persisted_model = model.clone();
    let started = match state.conversations.begin_command(
        &record.id,
        revision,
        Some(persisted_model),
        job.id(),
        command.clone(),
        !direct.excluded,
        directory_text,
    ) {
        Ok(started) => started,
        Err(error) => {
            state
                .sessions
                .finish_conversation_job(&session, record.id, job.id());
            return Err(StartMessageError::User(status_for(error), error.message()));
        }
    };
    let Some(message) = started
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::Command && message.request == Some(job.id()))
        .map(|message| message.id)
    else {
        state
            .sessions
            .finish_conversation_job(&session, record.id, job.id());
        return Err(StartMessageError::User(
            PatchStatus::UnprocessableEntity,
            "The command entry could not be recorded.",
        ));
    };
    let secret = provider_secret(state, &model.settings.model);
    let result = started.clone();
    let state = state.clone();
    spawn_conversation_work(state.clone(), session, started.id, async move {
        let _execution = execution;
        crate::execution::conversation::command::run(
            state.clone(),
            crate::execution::conversation::command::DirectCommandRun {
                session,
                record: started.clone(),
                job,
                message,
                command,
                included: !direct.excluded,
                directory,
                secret,
            },
        )
        .await;
    });
    Ok(result)
}
