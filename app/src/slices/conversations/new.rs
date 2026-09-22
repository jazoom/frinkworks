use super::*;

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct NewForm {
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) thinking: String,
    pub(super) preset: String,
    pub(super) preset_preview: String,
    pub(super) preset_name: String,
    pub(super) instructions: String,
    pub(super) tool_list: String,
    pub(super) tool_read: String,
    pub(super) tool_edit: String,
    pub(super) tool_write: String,
    pub(super) tool_run: String,
    pub(super) location: String,
    pub(super) host_approval: String,
    pub(super) environment: String,
    pub(super) network: String,
    pub(super) network_domains: String,
    pub(super) directory_0: String,
    pub(super) directory_1: String,
    pub(super) directory_2: String,
    pub(super) directory_3: String,
    pub(super) directory_4: String,
    pub(super) directory_5: String,
    pub(super) directory_6: String,
    pub(super) directory_7: String,
    pub(super) draft_nonce: String,
    pub(super) prepared_run: String,
    /// Names the source conversation for a fork draft. It is set only by the
    /// fork flow and lets a stale token fail instead of silently losing context.
    pub(super) fork_source: String,
    pub(super) handoff_approval: String,
    pub(super) consent_reference: String,
    pub(super) pending_directory: String,
    pub(super) consent_request: String,
    pub(super) host_consent_request: String,
    pub(super) consent_existing: String,
    pub(super) title: String,
    pub(super) message: String,
    #[serde(default, rename = "command_source")]
    pub(super) command_source: String,
    #[serde(default, rename = "command_hash")]
    pub(super) command_hash: String,
    pub(super) action: String,
    pub(super) attachment_0: String,
    pub(super) attachment_1: String,
    pub(super) attachment_2: String,
    pub(super) attachment_3: String,
    pub(super) attachment_4: String,
    pub(super) attachment_5: String,
    pub(super) attachment_6: String,
    pub(super) attachment_7: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct NewQuery {
    source: String,
    prompt: String,
}

pub(super) async fn show(
    State(state): State<AppState>,
    session: RequiredSession,
    graft: GraftRequest,
    Query(query): Query<NewQuery>,
) -> AppResult<Response> {
    // Genuinely new drafts start with every tool selected. Copied drafts
    // below overwrite these fields from the source, and validation failures
    // re-render the submitted form, so an explicit empty choice survives.
    let initial = crate::slices::execution_settings::page::initial_tool_ids();
    let selected = |id: crate::agents::ToolId| {
        if initial.contains(&id) {
            id.as_str().to_owned()
        } else {
            String::new()
        }
    };
    let mut form = NewForm {
        network: "none".to_owned(),
        tool_list: selected(crate::agents::ToolId::List),
        tool_read: selected(crate::agents::ToolId::Read),
        tool_edit: selected(crate::agents::ToolId::Edit),
        tool_write: selected(crate::agents::ToolId::Write),
        tool_run: selected(crate::agents::ToolId::Run),
        environment: super::default_environment(&state)
            .map(|id| id.as_hex())
            .unwrap_or_default(),
        draft_nonce: crate::execution::draft_nonce().map_err(|_| {
            AppError::new(
                "create draft consent nonce",
                std::io::Error::other("system random source unavailable"),
            )
        })?,
        ..NewForm::default()
    };
    if let Some(provider) = state
        .preferences
        .desk_providers(&state.vault)
        .into_iter()
        .find(|provider| provider.selected)
    {
        form.provider = provider.kind.as_str().to_owned();
        form.thinking = state
            .models_dev
            .effective_effort(provider.kind, &provider.model, provider.thinking.as_ref())
            .map(|effort| effort.as_str().to_owned())
            .unwrap_or_default();
        form.model = provider.model;
    } else {
        // Execution settings retain a model selection even without a provider connection.
        // Model submission still requires credentials. Direct commands never dispatch this model.
        let provider = ProviderKind::Xai;
        form.provider = provider.as_str().to_owned();
        form.model = provider.default_model().to_owned();
    }
    if let Some(settings) = state.preferences.conversation_defaults() {
        super::settings::copy_settings_to_draft(&mut form, &settings);
    }
    let mut model_notice = String::new();
    if query.source.is_empty()
        && let Some(provider) = ProviderKind::parse(&form.provider)
        && state.vault.contains(provider)
    {
        let preferred = state
            .models_dev
            .preferred_model(provider, &form.model)
            .unwrap_or_default();
        if preferred != form.model {
            model_notice = if preferred.is_empty() {
                "This provider has no models for new conversations. Choose another provider or refresh the catalogue in Settings.".to_owned()
            } else {
                format!(
                    "{} is unavailable for new conversations. This draft uses {preferred} from the same provider. Saved conversations stay unchanged.",
                    form.model
                )
            };
            form.model = preferred;
            form.thinking = state
                .models_dev
                .effective_effort(provider, &form.model, None)
                .map(|effort| effort.as_str().to_owned())
                .unwrap_or_default();
        }
    }
    if !query.source.is_empty() {
        let Some(source) = load_conversation(&state, &query.source) else {
            return Ok(responses::request_navigation(graft, "/conversations"));
        };
        // Copy values only. Runtime consent and unfinished work belong to the source.
        form = NewForm {
            draft_nonce: form.draft_nonce,
            ..NewForm::default()
        };
        if let Some(configuration) = &source.model {
            super::settings::copy_settings_to_draft(&mut form, &configuration.settings);
        }
    }
    let mut error = "";
    if !query.prompt.is_empty() {
        match state.prompts.get(&query.prompt) {
            Ok(prompt) => match prompt.problem {
                Some(problem) => error = problem.message(),
                None => {
                    // Qualification binds the command to the global source
                    // that the user selected in the catalogue.
                    form.message = format!("/global/{} ", prompt.name);
                }
            },
            Err(problem) => error = problem.message(),
        }
    }
    let mut view = ConversationDetailView::from_new(&state, session.0, form, error);
    view.model_picker.notice = model_notice;
    render_detail(&state, session.0, graft, PatchStatus::Ok, view)
}

impl NewForm {
    pub(super) fn consent_nonce(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        // Bind incomplete draft fields without a provider prerequisite for directory selection.
        for value in [
            &self.provider,
            &self.model,
            &self.thinking,
            &self.preset,
            &self.instructions,
            &self.tool_list,
            &self.tool_read,
            &self.tool_edit,
            &self.tool_write,
            &self.tool_run,
            &self.location,
            &self.host_approval,
            &self.environment,
            &self.network,
            &self.network_domains,
        ] {
            digest.update(value.len().to_le_bytes());
            digest.update(value);
        }
        format!(
            "{}:{}",
            self.draft_nonce,
            crate::hex::encode(&digest.finalize())
        )
    }

    pub(super) fn attachment_ids(
        &self,
    ) -> Result<Vec<crate::conversations::AttachmentId>, &'static str> {
        let mut ids = Vec::new();
        for value in [
            &self.attachment_0,
            &self.attachment_1,
            &self.attachment_2,
            &self.attachment_3,
            &self.attachment_4,
            &self.attachment_5,
            &self.attachment_6,
            &self.attachment_7,
        ] {
            if value.trim().is_empty() {
                continue;
            }
            let id = crate::conversations::attachments::parse_attachment_id(value)
                .ok_or("That staged image is not valid. Add it again.")?;
            if ids.contains(&id) {
                return Err("That staged image is not valid. Add it again.");
            }
            ids.push(id);
        }
        Ok(ids)
    }

    pub(super) fn tool_values(&self) -> Vec<String> {
        [
            &self.tool_list,
            &self.tool_read,
            &self.tool_edit,
            &self.tool_write,
            &self.tool_run,
        ]
        .into_iter()
        .filter(|value| !value.is_empty())
        .cloned()
        .collect()
    }

    pub(super) fn directories(
        &self,
    ) -> Result<Vec<crate::execution::DirectoryGrant>, &'static str> {
        let directories = self
            .directory_values()
            .into_iter()
            .map(|value| {
                crate::execution::DirectoryGrant::parse_form(value)
                    .ok_or("A directory grant is not valid.")
            })
            .collect::<Result<Vec<_>, _>>()?;
        crate::execution::validate_directories(&directories)
            .map_err(|_| "Choose valid non-overlapping directories.")?;
        Ok(directories)
    }

    fn directory_values(&self) -> Vec<&str> {
        [
            &self.directory_0,
            &self.directory_1,
            &self.directory_2,
            &self.directory_3,
            &self.directory_4,
            &self.directory_5,
            &self.directory_6,
            &self.directory_7,
        ]
        .into_iter()
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .collect()
    }

    pub(super) fn pending_directory(&self) -> Option<crate::execution::DirectoryGrant> {
        (!self.pending_directory.is_empty())
            .then(|| crate::execution::DirectoryGrant::parse_form(&self.pending_directory))
            .flatten()
    }

    pub(super) fn set_directories(&mut self, directories: &[crate::execution::DirectoryGrant]) {
        let mut values = directories.iter().map(|grant| grant.form_value());
        for field in [
            &mut self.directory_0,
            &mut self.directory_1,
            &mut self.directory_2,
            &mut self.directory_3,
            &mut self.directory_4,
            &mut self.directory_5,
            &mut self.directory_6,
            &mut self.directory_7,
        ] {
            *field = values.next().unwrap_or_default();
        }
    }
}

pub(super) fn model(
    state: &AppState,
    session: crate::sessions::SessionId,
    form: &NewForm,
) -> Result<Option<ConversationModelConfiguration>, &'static str> {
    let configuration = settings_snapshot(state, session, form)?;
    if let Some(configuration) = &configuration {
        valid_new_selection(state, &configuration.settings.model)?;
    }
    Ok(configuration)
}

pub(super) fn settings_snapshot(
    state: &AppState,
    session: crate::sessions::SessionId,
    form: &NewForm,
) -> Result<Option<ConversationModelConfiguration>, &'static str> {
    if form.provider.is_empty() && form.model.is_empty() {
        return Ok(None);
    }
    let provider = ProviderKind::parse(&form.provider).ok_or("Choose a stored provider.")?;
    let thinking = if form.thinking.trim().is_empty() {
        None
    } else {
        Some(
            ThinkingEffort::new(form.thinking.clone())
                .ok_or("Choose an available thinking effort.")?,
        )
    };
    let selection = ModelSelection::new(provider, form.model.clone(), thinking)
        .ok_or("Enter a valid model name.")?;
    let environment = if form.environment.trim().is_empty() {
        super::default_environment(state).map_err(|_| "Choose an environment.")?
    } else {
        EnvironmentId::parse(form.environment.trim()).ok_or("Choose a valid environment.")?
    };
    let network_mode = if form.network.trim().is_empty() {
        "none"
    } else {
        form.network.as_str()
    };
    let network = crate::agents::NetworkAccess::parse_form(network_mode, &form.network_domains)
        .map_err(|_| "Choose valid network access. Restricted access needs 1 to 32 domains.")?;
    let location = crate::execution::ToolLocation::parse(if form.location.trim().is_empty() {
        "sandbox"
    } else {
        form.location.trim()
    })
    .ok_or("Choose where tools run.")?;
    let host_approval =
        crate::execution::HostApprovalPolicy::parse(if form.host_approval.trim().is_empty() {
            "ask-each-time"
        } else {
            form.host_approval.trim()
        })
        .ok_or("Choose host command approval.")?;
    let settings = crate::execution::ExecutionSettings::new(
        selection,
        form.instructions.clone(),
        super::settings::parse_tools(&form.tool_values())?,
        environment,
    )
    .and_then(|settings| settings.with_network(network))
    .and_then(|settings| settings.with_directories(form.directories().ok()?))
    .map(|settings| {
        settings
            .with_location(location)
            .with_host_approval(host_approval)
    })
    .ok_or("Enter valid conversation settings.")?;
    let preset = state
        .presets
        .applied_draft(session, &form.preset_preview)
        .filter(|preset| preset.id.as_hex() == form.preset && preset.settings == settings);
    Ok(Some(match preset {
        Some(preset) => ConversationModelConfiguration::from_preset(&preset),
        None => ConversationModelConfiguration {
            settings,
            preset: None,
        },
    }))
}

pub(super) async fn save(
    State(state): State<AppState>,
    session: RequiredSession,
    _graft: PatchGraft,
    Form(form): Form<NewForm>,
) -> AppResult<Response> {
    let reject = |status, error, form| {
        render_detail(
            &state,
            session.0,
            GraftRequest::Patch,
            status,
            ConversationDetailView::from_new(&state, session.0, form, error),
        )
    };
    let reject_settings = |status, error, form| {
        render_detail(
            &state,
            session.0,
            GraftRequest::Patch,
            status,
            ConversationDetailView::from_new(&state, session.0, form, error).open_settings(),
        )
    };
    if form.action == "settings" {
        return reject_settings(PatchStatus::Ok, "", form);
    }
    if form.action != "send" {
        return reject(
            PatchStatus::UnprocessableEntity,
            "Send a message to start the conversation.",
            form,
        );
    }
    let direct = crate::conversations::input::direct_command(&form.message);
    let configuration = if direct.is_some() {
        settings_snapshot(&state, session.0, &form)
    } else {
        model(&state, session.0, &form)
    };
    let model = match configuration {
        Ok(model) => model,
        Err(error) => {
            return reject_settings(PatchStatus::UnprocessableEntity, error, form);
        }
    };
    if direct.is_none()
        && model
            .as_ref()
            .and_then(|model| state.vault.connection_for(&model.settings.model))
            .is_none()
    {
        return reject_settings(
            PatchStatus::UnprocessableEntity,
            "Choose a stored provider.",
            form,
        );
    };
    // No identity, reservation or store mutation precedes input validation.
    let attachments = match form.attachment_ids() {
        Ok(ids) => ids,
        Err(error) => return reject(PatchStatus::UnprocessableEntity, error, form),
    };
    if form.message.len() > crate::conversations::MAXIMUM_MESSAGE_BYTES
        || ((!form.message.trim().is_empty() || attachments.is_empty())
            && crate::conversations::normalise_message(&form.message).is_err())
    {
        return reject(
            PatchStatus::UnprocessableEntity,
            ConversationError::Message.message(),
            form,
        );
    }
    if !form.title.is_empty() && crate::conversations::normalise_title(&form.title).is_err() {
        return reject(
            PatchStatus::UnprocessableEntity,
            ConversationError::Title.message(),
            form,
        );
    }
    let Some(model) = model else {
        return reject_settings(
            PatchStatus::UnprocessableEntity,
            "Choose a stored provider.",
            form,
        );
    };
    let catalogue = super::commands::catalogue_for_draft(
        &state,
        session.0,
        &model.settings.directories,
        &form.consent_reference,
        &form.draft_nonce,
        model.settings.location,
    );
    if let Some(direct) = &direct {
        let validation = super::commands::run::reject_attachments(&attachments)
            .and_then(|()| super::commands::run::command_body(&direct.command).map(|_| ()))
            .and_then(|()| super::commands::run::command_directory(&model).map(|_| ()));
        if let Err(super::StartMessageError::User(status, error)) = validation {
            return reject(status, error, form);
        }
        if !form.handoff_approval.is_empty() {
            return reject(
                PatchStatus::Conflict,
                "Finish the handoff before a direct command.",
                form,
            );
        }
    }
    let secret = super::provider_secret(&state, &model.settings.model);
    let expansion = if direct.is_some() {
        crate::conversations::InputExpansion {
            typed: form.message.clone(),
            expanded: String::new(),
            provenance: None,
        }
    } else {
        match super::commands::expand(
            &form.message,
            &catalogue,
            secret.as_deref(),
            super::commands::preview_binding(&form.command_source, &form.command_hash),
        ) {
            Ok(expansion) => expansion,
            Err(error) => return reject(PatchStatus::UnprocessableEntity, error.message(), form),
        }
    };
    if form.pending_directory().is_some() {
        return reject(
            PatchStatus::UnprocessableEntity,
            "Approve or remove the pending sensitive directory.",
            form,
        );
    }
    let consent_grants = model
        .settings
        .directories
        .iter()
        .filter(|grant| {
            model.settings.location == crate::execution::ToolLocation::Sandbox
                && grant.requires_access_consent(state.local_data.root())
        })
        .cloned()
        .collect::<Vec<_>>();
    if consent_grants.iter().any(|grant| {
        !state.sessions.contains_live(&session.0)
            || !state.access_consent.authorised_draft(
                &form.consent_reference,
                session.0,
                &form.consent_nonce(),
                &model.settings.directories,
                grant,
            )
    }) {
        return reject(
            PatchStatus::UnprocessableEntity,
            "Directory access needs explicit approval.",
            form,
        );
    }
    if model.settings.host_tools()
        && (!state.sessions.contains_live(&session.0)
            || !state.access_consent.authorised_host_draft(
                &form.consent_reference,
                session.0,
                &form.consent_nonce(),
                &model.settings,
            ))
    {
        return reject(
            PatchStatus::UnprocessableEntity,
            "Unrestricted host access needs explicit approval.",
            form,
        );
    }
    // A fork draft carries copied context but no approval or execution state.
    // A present marker with no binding is a stale or foreign draft, not a new
    // conversation. Reject it so copied context cannot silently disappear.
    let fork = state.forks.get(session.0, &form.draft_nonce);
    if !form.fork_source.is_empty() && fork.is_none() {
        return reject(
            PatchStatus::Conflict,
            "The fork draft expired. Open Fork again from the source conversation.",
            form,
        );
    }
    if let Some(snapshot) = &fork
        && snapshot.candidate_review
        && (!model.settings.tools.is_empty() || !model.settings.directories.is_empty())
    {
        return reject_settings(
            PatchStatus::Conflict,
            "Candidate reviews use immutable evidence only. Use a separate conversation for file access.",
            form,
        );
    }
    let handoff_run = match super::handoff::transfer::preflight(&state, session.0, &form) {
        Ok(run) => run,
        Err(error) => return reject(PatchStatus::Conflict, error, form),
    };
    if handoff_run.is_none()
        && let Err(error) = super::preflight_execution(&state, session.0, None, &model).await
    {
        let super::StartMessageError::User(status, message) = error else {
            return Err(AppError::new(
                "preflight first conversation message",
                std::io::Error::other("unexpected internal preflight error"),
            ));
        };
        return reject_settings(status, message, form);
    }
    let id = ConversationId::generate()
        .map_err(|error| AppError::new("create conversation identifier", error))?;
    let Ok(permit) = state.local_data.begin_host_path_mutation().await else {
        return reject(
            PatchStatus::Conflict,
            crate::local_data::HOST_PATH_RESET_PENDING,
            form,
        );
    };
    let fork_claim = if fork.is_some() {
        let Some(claim) = state.forks.claim(session.0, &form.draft_nonce) else {
            return reject(
                PatchStatus::Conflict,
                "The fork draft is no longer available.",
                form,
            );
        };
        Some(claim)
    } else {
        None
    };
    if (!consent_grants.is_empty() || model.settings.host_tools())
        && state
            .access_consent
            .consume_draft(
                &form.consent_reference,
                session.0,
                &form.consent_nonce(),
                &model.settings,
                id,
                &consent_grants,
            )
            .is_err()
    {
        return reject(
            PatchStatus::UnprocessableEntity,
            "Directory access needs explicit approval.",
            form,
        );
    }
    let approvals = consent_grants
        .iter()
        .map(|grant| crate::conversations::DirectoryApproval::for_grant(&model.settings, grant))
        .collect();
    let created = match &fork {
        Some(snapshot) => {
            let messages =
                crate::conversations::forks::materialise(snapshot, id, &state.outputs)
                    .map_err(|error| AppError::new("prepare fork conversation entries", error))?;
            state.conversations.create_fork(
                id,
                (!form.title.is_empty()).then(|| form.title.clone()),
                Some(model.clone()),
                approvals,
                messages,
                snapshot,
            )
        }
        None => state.conversations.create_saved(
            id,
            (!form.title.is_empty()).then(|| form.title.clone()),
            Some(model.clone()),
            approvals,
        ),
    };
    let record = match created {
        Ok(record) => record,
        Err(error) => {
            state.access_consent.invalidate_conversation(id);
            if error == ConversationError::Unsettled {
                if let Some(claim) = fork_claim {
                    claim.commit();
                }
                return Err(AppError::new("settle fork creation", error));
            }
            if matches!(
                error,
                ConversationError::Persist | ConversationError::Corrupt
            ) {
                return Err(AppError::new(
                    "store first conversation and directory approvals",
                    error,
                ));
            }
            return reject(status_for(error), error.message(), form);
        }
    };
    // A durable destination consumes the token even if later startup fails.
    if let Some(claim) = fork_claim {
        claim.commit();
    }
    let start = if let Some(run) = handoff_run {
        let result = super::handoff::transfer::finish(&state, session.0, record, &form, run);
        drop(permit);
        result.map_err(|error| super::StartMessageError::User(PatchStatus::Conflict, error))
    } else if let Some(direct) = direct {
        drop(permit);
        let revision = record.revision;
        super::commands::run::start_saved(
            &state,
            session.0,
            &record,
            revision,
            model.clone(),
            direct,
            attachments,
        )
        .await
    } else {
        drop(permit);
        super::start_message(
            &state,
            session.0,
            record,
            1,
            model,
            expansion.expanded,
            expansion.provenance,
            attachments,
            super::attachments::page::draft_scope(&form.draft_nonce),
        )
        .await
    };
    let record = match start {
        Ok(record) => record,
        Err(super::StartMessageError::Internal(error)) => return Err(error),
        Err(super::StartMessageError::User(status, error)) => {
            state
                .conversations
                .delete(&id, 1)
                .map_err(|error| AppError::new("remove unstarted conversation", error))?;
            state.access_consent.invalidate_conversation(id);
            return reject(status, error, form);
        }
    };
    let view = detail_view(&state, session.0, &record, &record.title, "");
    let mut patches = hypergraft::PatchSet::new();
    patches
        .children("conversation-detail", &view.contents())
        .map_err(|error| AppError::new("render first conversation save", error))?;
    patches
        .replace_location(conversation_path(&record))
        .map_err(|error| AppError::new("locate first conversation save", error))?;
    patches = patches.title(&view.document_title);
    patches
        .respond(PatchStatus::Ok)
        .map_err(|error| AppError::new("respond to first conversation save", error))
}

#[cfg(test)]
mod tests;
