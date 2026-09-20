use std::time::{Duration, Instant};

use rig_core::completion::ToolDefinition;
use serde::{Deserialize, Serialize};

use crate::{
    agents::ToolId,
    sandbox::{CommandEvent, GUEST_PROJECT, GuestExec, GuestSandbox},
};

use super::artefacts::{
    ArtefactHash, ArtefactProducer, ArtefactSummary, CandidateHash, ObjectHash, TypedPayload,
    parse_typed_payload,
};
use super::definition::{
    ArtefactKind, ArtefactSource, InputKey, OutputKey, RequiredInput, StepAction, StepDefinition,
    StepKey,
};
use super::run::{AttemptArtefactInput, WorkflowRun};

pub(crate) const MAXIMUM_IMPORTED_TEXT_BYTES: usize = 1024 * 1024;
pub(crate) const MAXIMUM_PROJECT_INSTRUCTION_BYTES: usize = 32 * 1024;
pub(crate) const MAXIMUM_LAUNCH_BRIEF_BYTES: usize = 32 * 1024;
pub(crate) const MAXIMUM_ATTEMPT_PACKET_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const RESERVED_MODEL_OUTPUT_BYTES: usize = 128 * 1024;
pub(crate) const RESERVED_TOOL_WORK_BYTES: usize = 12 * 64 * 1024;
pub(crate) const MAXIMUM_INITIAL_CONTEXT_BYTES: usize =
    MAXIMUM_ATTEMPT_PACKET_BYTES - RESERVED_MODEL_OUTPUT_BYTES - RESERVED_TOOL_WORK_BYTES;
// Reject links before the read, including dangling links. The candidate has no active writer here.
const INSTRUCTION_READ_COMMAND: &str = "if [ -L AGENTS.md ]; then exit 4; fi; if [ ! -e AGENTS.md ]; then exit 3; fi; if [ ! -f AGENTS.md ] || [ ! -r AGENTS.md ]; then exit 1; fi; head -c 32769 -- AGENTS.md";
const INSTRUCTION_READ_DEADLINE: Duration = if cfg!(test) {
    Duration::from_millis(50)
} else {
    Duration::from_secs(5)
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProjectInstructions {
    Absent,
    Present(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstructionError {
    Path,
    Read,
    Invalid,
    Bound,
    Credential,
}

impl InstructionError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Path => "The project instruction path is not inside the target candidate.",
            Self::Read => "Power Plant could not read the target project's AGENTS.md file.",
            Self::Invalid => "The target project's AGENTS.md file is not valid text.",
            Self::Bound => "The target project's AGENTS.md file is too large.",
            Self::Credential => {
                "The target project's AGENTS.md file contains a provider credential."
            }
        }
    }
}

pub(crate) fn validate_instruction_text(
    text: &str,
    secret: Option<&str>,
) -> Result<(), InstructionError> {
    if text.len() > MAXIMUM_PROJECT_INSTRUCTION_BYTES {
        return Err(InstructionError::Bound);
    }
    if text.contains('\u{fffd}')
        || text
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(InstructionError::Invalid);
    }
    if secret.is_some_and(|secret| !secret.is_empty() && text.contains(secret)) {
        return Err(InstructionError::Credential);
    }
    Ok(())
}

pub(crate) fn classify_instruction_exit(
    exited: Option<i32>,
    text_is_empty: bool,
) -> Result<Option<ProjectInstructions>, InstructionError> {
    if exited == Some(3) && text_is_empty {
        return Ok(Some(ProjectInstructions::Absent));
    }
    if exited == Some(4) {
        return Err(InstructionError::Path);
    }
    if exited != Some(0) {
        return Err(InstructionError::Read);
    }
    Ok(None)
}

pub(crate) async fn read_project_instructions(
    sandbox: &GuestSandbox,
    secret: Option<&str>,
) -> Result<ProjectInstructions, InstructionError> {
    read_instructions_at(sandbox, GUEST_PROJECT, secret).await
}

pub(crate) async fn read_directory_instructions(
    sandbox: &GuestSandbox,
    authority: &crate::execution::ProjectFreeAuthority,
    secret: Option<&str>,
) -> Result<ProjectInstructions, InstructionError> {
    let mut combined = String::new();
    for grant in authority.policy.grants() {
        if let ProjectInstructions::Present(text) =
            read_instructions_at(sandbox, &grant.guest_path, secret).await?
        {
            let section = format!("\n## {}/AGENTS.md\n{}\n", grant.guest_path, text);
            if combined.len().saturating_add(section.len()) > MAXIMUM_PROJECT_INSTRUCTION_BYTES {
                return Err(InstructionError::Bound);
            }
            combined.push_str(&section);
        }
    }
    Ok(if combined.is_empty() {
        ProjectInstructions::Absent
    } else {
        ProjectInstructions::Present(combined)
    })
}

async fn read_instructions_at(
    sandbox: &GuestSandbox,
    guest_path: &str,
    secret: Option<&str>,
) -> Result<ProjectInstructions, InstructionError> {
    let mut command = sandbox
        .exec_cmd(GuestExec::shell(INSTRUCTION_READ_COMMAND).in_dir(guest_path))
        .await
        .map_err(|_| InstructionError::Read)?;
    let deadline = Instant::now() + INSTRUCTION_READ_DEADLINE;
    let mut text_bytes = Vec::new();
    let mut exited = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            command.kill().await;
            command.close().await;
            return Err(InstructionError::Read);
        }
        let event = match tokio::time::timeout(remaining, command.recv()).await {
            Ok(event) => event,
            Err(_) => {
                command.kill().await;
                command.close().await;
                return Err(InstructionError::Read);
            }
        };
        let Some(event) = event else {
            break;
        };
        match event {
            CommandEvent::Output { stream, bytes } => {
                if stream != crate::execution::CommandStream::Stdout {
                    continue;
                }
                if text_bytes.len().saturating_add(bytes.len()) > MAXIMUM_PROJECT_INSTRUCTION_BYTES
                {
                    command.kill().await;
                    command.close().await;
                    return Err(InstructionError::Bound);
                }
                text_bytes.extend_from_slice(&bytes);
            }
            CommandEvent::Exited(code) => exited = Some(code),
            CommandEvent::Failed => {
                command.close().await;
                return Err(InstructionError::Read);
            }
        }
    }
    command.close().await;
    let text = String::from_utf8_lossy(&text_bytes).into_owned();
    if let Some(instructions) = classify_instruction_exit(exited, text.is_empty())? {
        return Ok(instructions);
    }
    validate_instruction_text(&text, secret)?;
    Ok(ProjectInstructions::Present(text))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerifiedInput {
    pub(crate) key: InputKey,
    pub(crate) kind: ArtefactKind,
    pub(crate) artefact_id: super::id::ArtefactId,
    pub(crate) artefact_hash: ArtefactHash,
    pub(crate) object_hash: ObjectHash,
    pub(crate) producer_step: Option<StepKey>,
    pub(crate) producer_output: Option<OutputKey>,
    pub(crate) candidate: Option<CandidateHash>,
    pub(crate) text: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputContextError {
    Missing,
    Changed,
    Kind,
    Provenance,
    Source,
    Bound,
    Credential,
    Brief,
    Packet,
}

impl InputContextError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Missing => "A declared input artefact is missing.",
            Self::Changed => "A declared input artefact changed.",
            Self::Kind => "That input kind does not match the stored artefact.",
            Self::Provenance => "That input artefact does not belong to this run.",
            Self::Source => "That input does not match its declared source.",
            Self::Bound => "Imported plan or report text is too large.",
            Self::Credential => "The initial context cannot include a provider credential.",
            Self::Brief => "Enter a task brief of at most 32 KiB.",
            Self::Packet => "The workflow context is too large for this launch.",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ContextCandidateReference {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) artefact_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub(crate) enum ProjectInstructionState {
    Absent,
    Present { text: String, content_hash: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ProjectInstructionSnapshot {
    pub(crate) candidate: Option<ContextCandidateReference>,
    pub(crate) guest_path: String,
    pub(crate) state: ProjectInstructionState,
}

impl ProjectInstructionSnapshot {
    fn from_verified(verified: &[VerifiedInput], instructions: &ProjectInstructions) -> Self {
        let candidate = verified
            .iter()
            .find(|input| input.kind == ArtefactKind::CandidateRevision)
            .map(|input| ContextCandidateReference {
                id: input.artefact_id.as_hex(),
                kind: input.kind.as_str().to_owned(),
                artefact_hash: input.artefact_hash.as_str(),
            });
        let state = match instructions {
            ProjectInstructions::Absent => ProjectInstructionState::Absent,
            ProjectInstructions::Present(text) => ProjectInstructionState::Present {
                text: text.clone(),
                content_hash: ObjectHash::of(text.as_bytes()).as_str(),
            },
        };
        Self {
            candidate,
            guest_path: "AGENTS.md".to_owned(),
            state,
        }
    }

    pub(crate) fn text(&self) -> Option<&str> {
        match &self.state {
            ProjectInstructionState::Absent => None,
            ProjectInstructionState::Present { text, .. } => Some(text),
        }
    }

    pub(crate) fn content_hash(&self) -> Option<&str> {
        match &self.state {
            ProjectInstructionState::Absent => None,
            ProjectInstructionState::Present { content_hash, .. } => Some(content_hash),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ContextTool {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) parameters: serde_json::Value,
}

impl ContextTool {
    fn from_definition(tool: &ToolDefinition) -> Self {
        Self {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        }
    }

    fn request_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
        }
    }

    fn byte_len(&self) -> usize {
        json_byte_len(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ContextBudget {
    pub(crate) packet_bytes: u64,
    pub(crate) reserved_output_bytes: u64,
    pub(crate) reserved_tool_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) estimated_input_tokens: u64,
    pub(crate) estimated_total_tokens: u64,
    pub(crate) model_context_limit: Option<u64>,
}

impl ContextBudget {
    pub(crate) fn capacity_label(&self) -> String {
        self.model_context_limit
            .map(|limit| format!("{limit} tokens"))
            .unwrap_or_else(|| "Unknown".to_owned())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    deny_unknown_fields,
    tag = "role",
    content = "text",
    rename_all = "kebab-case"
)]
pub(crate) enum ContextMessage {
    User(String),
    Assistant(String),
}

impl ContextMessage {
    fn byte_len(&self) -> usize {
        json_byte_len(self)
    }

    pub(crate) fn text(&self) -> &str {
        match self {
            Self::User(text) | Self::Assistant(text) => text,
        }
    }

    pub(crate) fn role(&self) -> &'static str {
        match self {
            Self::User(_) => "User",
            Self::Assistant(_) => "Assistant",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct AttemptContextPacket {
    pub(crate) prompt: String,
    pub(crate) messages: Vec<ContextMessage>,
    pub(crate) tools: Vec<ContextTool>,
    pub(crate) source_available: String,
    pub(crate) excluded_context: String,
    pub(crate) project_instructions: ProjectInstructionSnapshot,
    pub(crate) budget: ContextBudget,
}

impl AttemptContextPacket {
    pub(crate) fn request_messages(&self) -> Vec<crate::providers::ChatTurn> {
        self.messages
            .iter()
            .map(|message| match message {
                ContextMessage::User(text) => crate::providers::ChatTurn::user(text.clone()),
                ContextMessage::Assistant(text) => {
                    crate::providers::ChatTurn::assistant(text.clone().into())
                }
            })
            .collect()
    }

    pub(crate) fn byte_len(&self) -> usize {
        self.prompt
            .len()
            .saturating_add(
                self.messages
                    .iter()
                    .map(ContextMessage::byte_len)
                    .try_fold(0usize, usize::checked_add)
                    .unwrap_or(usize::MAX),
            )
            .saturating_add(
                self.tools
                    .iter()
                    .map(ContextTool::byte_len)
                    .try_fold(0usize, usize::checked_add)
                    .unwrap_or(usize::MAX),
            )
    }

    pub(crate) fn request_tools(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(ContextTool::request_definition)
            .collect()
    }

    pub(crate) fn tool_ids(&self) -> Vec<ToolId> {
        self.tools
            .iter()
            .filter_map(|tool| ToolId::parse(&tool.name))
            .collect()
    }

    pub(crate) fn validate(&self) -> bool {
        let byte_len = self.byte_len();
        let (output_bytes, tool_bytes) = reserved_bytes(self.budget.model_context_limit);
        let Some(total_bytes) = byte_len.checked_add(output_bytes.saturating_add(tool_bytes))
        else {
            return false;
        };
        let Some(input_tokens) = estimate_tokens(byte_len) else {
            return false;
        };
        let Some(total_tokens) = estimate_tokens(total_bytes) else {
            return false;
        };
        !self.messages.is_empty()
            && self.source_available.len() <= 8192
            && self.excluded_context.len() <= 1024
            && self.prompt.len() <= MAXIMUM_INITIAL_CONTEXT_BYTES
            && byte_len <= MAXIMUM_INITIAL_CONTEXT_BYTES
            && total_bytes <= MAXIMUM_ATTEMPT_PACKET_BYTES
            && self.budget.packet_bytes == byte_len as u64
            && self.budget.reserved_output_bytes == output_bytes as u64
            && self.budget.reserved_tool_bytes == tool_bytes as u64
            && self.budget.total_bytes == total_bytes as u64
            && self.budget.estimated_input_tokens == input_tokens
            && self.budget.estimated_total_tokens == total_tokens
            && self
                .budget
                .model_context_limit
                .is_none_or(|limit| total_tokens <= limit)
            && self.project_instructions.guest_path == "AGENTS.md"
            && self
                .project_instructions
                .text()
                .is_none_or(|text| text.len() <= MAXIMUM_PROJECT_INSTRUCTION_BYTES)
            && self
                .project_instructions
                .content_hash()
                .is_none_or(|hash| ObjectHash::parse(hash).is_some())
            && match &self.project_instructions.state {
                ProjectInstructionState::Absent => true,
                ProjectInstructionState::Present { text, content_hash } => {
                    ObjectHash::parse(content_hash)
                        .is_some_and(|hash| hash == ObjectHash::of(text.as_bytes()))
                }
            }
    }
}

pub(crate) fn validate_launch_brief(brief: &str) -> Result<String, InputContextError> {
    let brief = brief.trim();
    if brief.is_empty()
        || brief.len() > MAXIMUM_LAUNCH_BRIEF_BYTES
        || brief.contains('\0')
        || brief
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(InputContextError::Brief);
    }
    Ok(brief.to_owned())
}

pub(crate) fn authorised_source_text(
    settings: &crate::execution::ExecutionSettings,
    writes_source: bool,
) -> String {
    if settings.location == crate::execution::ToolLocation::Host {
        let directories = settings
            .directories
            .iter()
            .map(|grant| format!("- {}: work location", grant.host_path.display()))
            .collect::<Vec<_>>();
        format!(
            "Tools run on this computer. Work locations are conveniences, not confinement:\n{}\nSandbox guest paths such as /access/<alias> and /workspace from earlier turns grant no host authority. File import still uses an explicit read-only sandbox.",
            if directories.is_empty() {
                "None. Commands start in Power Plant's current directory.".to_owned()
            } else {
                directories.join("\n")
            }
        )
    } else {
        let directories = settings
            .directories
            .iter()
            .map(|grant| {
                let access = match grant.access {
                    crate::execution::DirectoryAccess::DirectWrite => {
                        "Direct write: immediate host changes. Discard and cancellation do not undo them."
                    }
                    crate::execution::DirectoryAccess::ReviewBeforeApply if writes_source => {
                        "Review before apply"
                    }
                    _ => "Read only",
                };
                format!("- {}: {}", grant.guest_path(), access)
            })
            .collect::<Vec<_>>();
        format!(
            "Private scratch: /workspace. Only the listed tools are available. Authorised directories:\n{}",
            if directories.is_empty() {
                "None".to_owned()
            } else {
                directories.join("\n")
            }
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_attempt_packet_for_request(
    run: &WorkflowRun,
    step: &StepDefinition,
    resolved: &[AttemptArtefactInput],
    store: &super::artefacts::WorkflowArtefactRepository,
    project_instructions: ProjectInstructions,
    conversation_turns: &[crate::providers::ChatTurn],
    role_preamble: &str,
    tools: &[ToolDefinition],
    model_context_limit: Option<u64>,
    secret: Option<&str>,
) -> Result<AttemptContextPacket, InputContextError> {
    let brief = validate_launch_brief(&run.launch_brief)?;
    let verified = verify_inputs(run, step, resolved, store)?;
    let project_instructions =
        ProjectInstructionSnapshot::from_verified(&verified, &project_instructions);
    let source_available = if let Some(settings) = run.phase_settings(&step.key) {
        authorised_source_text(settings, step.writes_primary_source())
    } else if matches!(run.source, super::run::RunSource::None) {
        let directories = match &step.action {
            StepAction::Agent(action) => action
                .authority
                .directories
                .iter()
                .map(|directory| format!("- /access/{}: Read only", directory.alias))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        if directories.is_empty() {
            "Private scratch storage is available at /workspace through the listed tools. No host directory is mounted.".to_owned()
        } else {
            format!(
                "Private scratch storage is available at /workspace through the listed tools. The first authorised directory is the default command directory. Host directories are mounted read only:\n{}",
                directories.join("\n")
            )
        }
    } else if tools.iter().any(|tool| {
        matches!(
            ToolId::parse(&tool.name),
            Some(ToolId::List | ToolId::Read | ToolId::Run)
        )
    }) {
        "The materialised candidate and authorised secondary directories are available only through the listed tools. Nested AGENTS.md files and secondary-project instructions are available through authorised tools, not automatic context.".to_owned()
    } else {
        "No source files are available through tools for this phase. Project instructions above are the only automatic source context.".to_owned()
    };
    let revision_feedback = verify_revision_feedback(run, step, resolved, store)?
        .map(|feedback| {
            format!(
                "\n\n# Human revision feedback\n\n{feedback}\n\nThe rejected candidate remains the revision input. The original task brief and immutable diff base remain unchanged."
            )
        })
        .unwrap_or_default();
    let excluded_context = if run.kind == super::run::RunKind::Configured
        || run.revision_feedback(&step.key).is_some()
    {
        "The source conversation, its messages, thoughts and tool output, plus worker transcripts from other attempts, are excluded.".to_owned()
    } else {
        "Worker transcripts from other attempts are excluded. The ordinary conversation messages below remain part of this request.".to_owned()
    };
    let instructions = match &project_instructions.state {
        ProjectInstructionState::Absent if matches!(run.source, super::run::RunSource::None) => {
            "# Directory context\n\nNo project instructions are supplied automatically. Use authorised tools to inspect the listed directories."
                .to_owned()
        }
        ProjectInstructionState::Absent => {
            "# Project instructions\n\nNo root AGENTS.md file was present in this candidate."
                .to_owned()
        }
        ProjectInstructionState::Present { text, .. } => {
            format!("# Project instructions\n\n{text}")
        }
    };
    let context = format!(
        "Brief:\n{}\n\n{}\n\n{}{}\n\n# Context boundary\n\nSource available through tools: {}\n\nExcluded context: {}",
        brief.trim(),
        format_agent_context(&verified, step.writes_primary_source()),
        instructions,
        revision_feedback,
        source_available,
        excluded_context,
    );
    let prompt = if role_preamble.trim().is_empty() {
        context
    } else {
        format!("{}\n\n{}", role_preamble.trim(), context)
    };
    let messages = if run.kind == super::run::RunKind::QuickTask
        && run.revision_feedback(&step.key).is_none()
        && !conversation_turns.is_empty()
    {
        conversation_turns
            .iter()
            .map(|turn| match turn.role {
                crate::providers::Role::User => ContextMessage::User(turn.text.clone()),
                crate::providers::Role::Assistant => ContextMessage::Assistant(turn.text.clone()),
            })
            .collect::<Vec<_>>()
    } else {
        // Rig requires a user message even when the complete task is in the preamble.
        vec![ContextMessage::User(
            "Execute the assigned task in the initial prompt.".to_owned(),
        )]
    };
    let tools: Vec<_> = tools.iter().map(ContextTool::from_definition).collect();
    if secret.is_some_and(|secret| {
        !secret.is_empty()
            && (prompt.contains(secret)
                || messages
                    .iter()
                    .any(|message| message.text().contains(secret))
                || tools.iter().any(|tool| {
                    tool.name.contains(secret)
                        || tool.description.contains(secret)
                        || tool.parameters.to_string().contains(secret)
                }))
    }) {
        return Err(InputContextError::Credential);
    }
    let packet_bytes = prompt
        .len()
        .saturating_add(
            messages
                .iter()
                .map(ContextMessage::byte_len)
                .try_fold(0usize, usize::checked_add)
                .unwrap_or(usize::MAX),
        )
        .saturating_add(
            tools
                .iter()
                .map(ContextTool::byte_len)
                .try_fold(0usize, usize::checked_add)
                .unwrap_or(usize::MAX),
        );
    let (output_bytes, tool_bytes) = reserved_bytes(model_context_limit);
    let total_bytes = packet_bytes
        .checked_add(output_bytes)
        .and_then(|bytes| bytes.checked_add(tool_bytes))
        .ok_or(InputContextError::Packet)?;
    let estimated_input_tokens = estimate_tokens(packet_bytes).ok_or(InputContextError::Packet)?;
    let estimated_total_tokens = estimate_tokens(total_bytes).ok_or(InputContextError::Packet)?;
    if packet_bytes > MAXIMUM_INITIAL_CONTEXT_BYTES
        || total_bytes > MAXIMUM_ATTEMPT_PACKET_BYTES
        || model_context_limit.is_some_and(|limit| estimated_total_tokens > limit)
    {
        return Err(InputContextError::Packet);
    }
    let packet = AttemptContextPacket {
        prompt,
        messages,
        tools,
        source_available,
        excluded_context,
        project_instructions,
        budget: ContextBudget {
            packet_bytes: packet_bytes as u64,
            reserved_output_bytes: output_bytes as u64,
            reserved_tool_bytes: tool_bytes as u64,
            total_bytes: total_bytes as u64,
            estimated_input_tokens,
            estimated_total_tokens,
            model_context_limit,
        },
    };
    if !packet.validate() {
        return Err(InputContextError::Packet);
    }
    Ok(packet)
}

// Reserve half of a known context, up to the application allowance. A fixed maximum
// reserve alone excludes small-context models even when their initial prompt is short.
fn reserved_bytes(model_context_limit: Option<u64>) -> (usize, usize) {
    let maximum = RESERVED_MODEL_OUTPUT_BYTES + RESERVED_TOOL_WORK_BYTES;
    let reserve = model_context_limit
        .map(|tokens| tokens.saturating_mul(2).min(maximum as u64) as usize)
        .unwrap_or(maximum);
    let output = reserve / 7;
    (output, reserve - output)
}

fn json_byte_len(value: &impl Serialize) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

fn estimate_tokens(bytes: usize) -> Option<u64> {
    u64::try_from(bytes)
        .ok()
        .and_then(|bytes| bytes.checked_add(3))
        .map(|bytes| bytes / 4)
}

fn verify_revision_feedback(
    run: &WorkflowRun,
    step: &StepDefinition,
    resolved: &[AttemptArtefactInput],
    store: &super::artefacts::WorkflowArtefactRepository,
) -> Result<Option<String>, InputContextError> {
    let Some(reservation) = &run.revision_reservation else {
        return Ok(None);
    };
    let gate = run
        .gates
        .iter()
        .find(|gate| gate.id == reservation.gate)
        .ok_or(InputContextError::Source)?;
    let super::run::RunSource::Captured { source } = &run.source else {
        return Err(InputContextError::Source);
    };
    let plan_revision = reservation.candidate.kind == ArtefactKind::Plan;
    if reservation.target != step.key
        || gate.state != super::gates::HumanGateState::RevisionRequested
        || gate.decision.as_ref() != Some(&reservation.decision)
        || gate.candidate != reservation.candidate
        || gate.diff_base != reservation.diff_base
        || (!plan_revision && source.initial != reservation.diff_base)
        || !resolved
            .iter()
            .any(|input| input.artefact == reservation.candidate)
        || run
            .human_revision_policy(&gate.step)
            .is_none_or(|policy| policy.revision_target != step.key)
    {
        return Err(InputContextError::Source);
    }
    let declared = RequiredInput {
        key: super::definition::InputKey::parse("human-revision-feedback")
            .expect("revision input key"),
        kind: if plan_revision {
            ArtefactKind::PlanDecision
        } else {
            ArtefactKind::HumanDecision
        },
        source: ArtefactSource::StepOutput {
            step: gate.step.clone(),
            output: gate.output.clone(),
        },
    };
    let input = AttemptArtefactInput {
        key: declared.key.clone(),
        artefact: reservation.decision.clone(),
    };
    let verified = verify_one(run, &declared, &input, store, &mut 0)?;
    let record = run
        .artefact(&verified.artefact_id)
        .ok_or(InputContextError::Missing)?;
    if !matches!(&record.provenance.producer,
        ArtefactProducer::HumanGate { gate_id: id, .. } if *id == gate.id)
    {
        return Err(InputContextError::Provenance);
    }
    let bytes = store
        .get(&record.object_hash)
        .map_err(|_| InputContextError::Missing)?;
    let payload = parse_typed_payload(record.kind, &bytes).map_err(map_payload)?;
    if plan_revision {
        let TypedPayload::PlanDecision(decision) = payload else {
            return Err(InputContextError::Kind);
        };
        if decision.decision != super::gates::PlanDecisionKind::RevisionRequested
            || decision.plan != reservation.candidate.artefact_hash.as_str()
            || decision.note.as_deref() != Some(reservation.feedback.as_str())
        {
            return Err(InputContextError::Changed);
        }
        Ok(decision.note)
    } else {
        let TypedPayload::HumanDecision(decision) = payload else {
            return Err(InputContextError::Kind);
        };
        let candidate = run
            .artefact(&reservation.candidate.id)
            .and_then(super::artefacts::ArtefactRecord::candidate_hash)
            .ok_or(InputContextError::Changed)?;
        let base = run
            .artefact(&reservation.diff_base.id)
            .and_then(super::artefacts::ArtefactRecord::candidate_hash)
            .ok_or(InputContextError::Changed)?;
        if decision.decision != super::gates::HumanDecisionKind::RevisionRequested
            || decision.candidate != candidate.as_str()
            || decision.diff_base != base.as_str()
            || decision.note.as_deref() != Some(reservation.feedback.as_str())
        {
            return Err(InputContextError::Changed);
        }
        Ok(decision.note)
    }
}

pub(crate) fn verify_inputs(
    run: &WorkflowRun,
    step: &StepDefinition,
    resolved: &[AttemptArtefactInput],
    store: &super::artefacts::WorkflowArtefactRepository,
) -> Result<Vec<VerifiedInput>, InputContextError> {
    if resolved.len() != step.inputs.len() {
        return Err(InputContextError::Missing);
    }
    let mut verified = Vec::new();
    let mut imported = 0usize;
    for (declared, resolved) in step.inputs.iter().zip(resolved.iter()) {
        if declared.key != resolved.key {
            return Err(InputContextError::Source);
        }
        verified.push(verify_one(run, declared, resolved, store, &mut imported)?);
    }
    let plan_decisions: Vec<_> = verified
        .iter()
        .filter(|input| input.kind == ArtefactKind::PlanDecision)
        .collect();
    if !plan_decisions.is_empty() {
        let plans: Vec<_> = verified
            .iter()
            .filter(|input| input.kind == ArtefactKind::Plan)
            .collect();
        if plans.len() != 1
            || plan_decisions.iter().any(|input| {
                !run.artefact(&input.artefact_id).is_some_and(|record| {
                    matches!(
                        record.summary,
                        ArtefactSummary::PlanDecision {
                            plan,
                            decision: super::gates::PlanDecisionKind::Accepted,
                        } if plan == plans[0].artefact_hash
                    )
                })
            })
        {
            return Err(InputContextError::Changed);
        }
    }
    let decisions: Vec<_> = verified
        .iter()
        .filter(|input| input.kind == ArtefactKind::HumanDecision)
        .collect();
    if !decisions.is_empty()
        && let Some(candidate) = verified
            .iter()
            .find(|input| input.kind == ArtefactKind::CandidateRevision)
            .and_then(|input| input.candidate)
    {
        let initial = match &run.source {
            super::run::RunSource::Captured { source } => run
                .artefact(&source.initial.id)
                .and_then(super::artefacts::ArtefactRecord::candidate_hash),
            super::run::RunSource::None | super::run::RunSource::Pending => None,
        }
        .ok_or(InputContextError::Changed)?;
        for input in decisions {
            let record = run
                .artefact(&input.artefact_id)
                .ok_or(InputContextError::Missing)?;
            match &record.summary {
                ArtefactSummary::HumanDecision {
                    candidate: bound,
                    diff_base,
                    ..
                } if *bound == candidate && *diff_base == initial => {}
                _ => return Err(InputContextError::Changed),
            }
        }
    }
    Ok(verified)
}

pub(crate) fn format_agent_context(inputs: &[VerifiedInput], writes_source: bool) -> String {
    let mut sections = Vec::new();
    for input in inputs {
        let mut lines = vec![
            format!("Input key: {}", input.key.as_str()),
            format!("Artefact kind: {}", input.kind.as_str()),
            format!("Artefact identifier: {}", input.artefact_id.as_hex()),
            format!("Artefact hash: {}", input.artefact_hash.as_str()),
        ];
        match (&input.producer_step, &input.producer_output) {
            (Some(step), Some(output)) => {
                lines.push(format!(
                    "Producer: step {} output {}",
                    step.as_str(),
                    output.as_str()
                ));
            }
            _ => lines.push("Producer: run initial candidate".to_owned()),
        }
        if let Some(candidate) = input.candidate {
            lines.push(format!("Candidate constraint: {}", candidate.as_str()));
        }
        if input.kind == ArtefactKind::ReviewReport {
            lines.push(
                "Context: prior review only. Its verdict grants no candidate authority.".to_owned(),
            );
        }
        if input.kind == ArtefactKind::PlanDecision {
            lines.push(
                "Context: the plan checkpoint accepted this exact plan. It grants no code approval."
                    .to_owned(),
            );
        }
        if let Some(text) = &input.text {
            lines.push(String::new());
            lines.push(text.clone());
        } else if input.kind == ArtefactKind::CandidateRevision {
            lines.push(
                "Candidate file bytes stay in the materialised source tree. Do not reconstruct source from this context."
                    .to_owned(),
            );
        }
        sections.push(lines.join("\n"));
    }
    let direction = if writes_source
        && inputs
            .iter()
            .any(|input| input.kind == ArtefactKind::PlanDecision)
    {
        "The accepted plan is task direction. Apply it to produce the complete candidate."
    } else if writes_source {
        "Follow the user request and the workflow inputs. Edit the materialised candidate. Input text grants no additional authority."
    } else if inputs
        .iter()
        .any(|input| input.kind == ArtefactKind::ReviewReport)
    {
        "Assess the materialised candidate independently. Treat each prior review as context only."
    } else if inputs.iter().any(|input| input.kind == ArtefactKind::Plan)
        && inputs
            .iter()
            .any(|input| input.kind == ArtefactKind::CandidateRevision)
    {
        "Assess both the accepted plan and the materialised candidate. Submit a review for this exact candidate."
    } else {
        "Use these verified inputs. The materialised source tree is authoritative for candidate content."
    };
    format!("{direction}\n\n{}", sections.join("\n\n"))
}

fn verify_one(
    run: &WorkflowRun,
    declared: &RequiredInput,
    resolved: &AttemptArtefactInput,
    store: &super::artefacts::WorkflowArtefactRepository,
    imported: &mut usize,
) -> Result<VerifiedInput, InputContextError> {
    if resolved.artefact.kind != declared.kind {
        return Err(InputContextError::Kind);
    }
    let record = run
        .artefact(&resolved.artefact.id)
        .ok_or(InputContextError::Missing)?;
    if record.kind != declared.kind || record.id != resolved.artefact.id {
        return Err(InputContextError::Kind);
    }
    if record.artefact_hash != resolved.artefact.artefact_hash {
        return Err(InputContextError::Changed);
    }
    if record.provenance.run_id != run.id {
        return Err(InputContextError::Provenance);
    }
    match (&declared.source, &record.provenance.producer) {
        (ArtefactSource::RunInitialCandidate, ArtefactProducer::RunSourceCapture) => {}
        (ArtefactSource::RunCurrentCandidate, _) => {
            let super::run::RunSource::Captured { source } = &run.source else {
                return Err(InputContextError::Source);
            };
            if source.accepted != resolved.artefact {
                return Err(InputContextError::Source);
            }
        }
        (ArtefactSource::RunCurrentPlan, _) => {
            let current = run.current_plan().ok_or(InputContextError::Source)?;
            if current != resolved.artefact {
                return Err(InputContextError::Source);
            }
        }

        (
            ArtefactSource::StepOutput { step, output },
            ArtefactProducer::StepAttempt {
                step: producer_step,
                output: Some(producer_output),
                ..
            },
        ) if producer_step == step && producer_output == output => {}
        (
            ArtefactSource::StepOutput { step, output },
            ArtefactProducer::HumanGate {
                step: producer_step,
                output: producer_output,
                ..
            },
        ) if producer_step == step && producer_output == output => {}
        (ArtefactSource::StepOutput { .. }, ArtefactProducer::StepAttempt { output: None, .. }) => {
            return Err(InputContextError::Source);
        }
        _ => return Err(InputContextError::Source),
    }
    let bytes = store
        .get(&record.object_hash)
        .map_err(|_| InputContextError::Missing)?;
    if ObjectHash::of(&bytes) != record.object_hash {
        return Err(InputContextError::Changed);
    }
    let (text, candidate) = match record.kind {
        ArtefactKind::CandidateRevision => {
            let artefact = super::artefacts::CandidatePayload::from_manifest_bytes(&bytes)
                .ok_or(InputContextError::Changed)?;
            let hash = super::artefacts::artefact_hash_for(
                ArtefactKind::CandidateRevision,
                super::artefacts::CANDIDATE_SCHEMA,
                &bytes,
            );
            if hash != record.artefact_hash {
                return Err(InputContextError::Changed);
            }
            (None, Some(artefact.candidate_hash()))
        }
        ArtefactKind::Plan
        | ArtefactKind::ReviewReport
        | ArtefactKind::TestReport
        | ArtefactKind::HumanDecision
        | ArtefactKind::PlanDecision => {
            let payload = parse_typed_payload(record.kind, &bytes).map_err(map_payload)?;
            let schema = match record.kind {
                ArtefactKind::HumanDecision => super::artefacts::payload::HUMAN_DECISION_SCHEMA,
                ArtefactKind::PlanDecision => super::artefacts::payload::PLAN_DECISION_SCHEMA,
                _ => super::artefacts::payload::PLAN_SCHEMA,
            };
            let hash = super::artefacts::artefact_hash_for(record.kind, schema, &bytes);
            if hash != record.artefact_hash {
                return Err(InputContextError::Changed);
            }
            let (markdown, candidate) = match payload {
                TypedPayload::Plan(plan) => (plan.markdown, None),
                TypedPayload::Review(report) => (
                    report.markdown,
                    Some(
                        CandidateHash::parse(&report.candidate)
                            .ok_or(InputContextError::Changed)?,
                    ),
                ),
                TypedPayload::Test(report) => (
                    report.markdown,
                    Some(
                        CandidateHash::parse(&report.candidate)
                            .ok_or(InputContextError::Changed)?,
                    ),
                ),
                TypedPayload::HumanDecision(decision) => (
                    format!("Decision: {}", decision.decision.as_label()),
                    Some(
                        CandidateHash::parse(&decision.candidate)
                            .ok_or(InputContextError::Changed)?,
                    ),
                ),
                TypedPayload::PlanDecision(decision) => {
                    let plan =
                        super::gates::plan_hash(&decision).ok_or(InputContextError::Changed)?;
                    if !matches!(
                        &record.summary,
                        ArtefactSummary::PlanDecision { plan: bound, decision: kind }
                            if *bound == plan && *kind == decision.decision
                    ) {
                        return Err(InputContextError::Changed);
                    }
                    (
                        format!("Plan decision: {}", decision.decision.as_label()),
                        None,
                    )
                }
            };
            *imported = imported
                .checked_add(markdown.len())
                .ok_or(InputContextError::Bound)?;
            if *imported > MAXIMUM_IMPORTED_TEXT_BYTES {
                return Err(InputContextError::Bound);
            }
            (Some(markdown), candidate)
        }
    };
    if let ArtefactSummary::Review {
        candidate: bound, ..
    }
    | ArtefactSummary::Test {
        candidate: bound, ..
    } = &record.summary
        && candidate != Some(*bound)
    {
        return Err(InputContextError::Changed);
    }
    Ok(VerifiedInput {
        key: declared.key.clone(),
        kind: record.kind,
        artefact_id: record.id,
        artefact_hash: record.artefact_hash,
        object_hash: record.object_hash,
        producer_step: match &record.provenance.producer {
            ArtefactProducer::StepAttempt { step, .. }
            | ArtefactProducer::HumanGate { step, .. } => Some(step.clone()),
            ArtefactProducer::RunSourceCapture => None,
        },
        producer_output: match &record.provenance.producer {
            ArtefactProducer::StepAttempt { output, .. } => output.clone(),
            ArtefactProducer::HumanGate { output, .. } => Some(output.clone()),
            ArtefactProducer::RunSourceCapture => None,
        },
        candidate,
        text,
    })
}

fn map_payload(error: super::artefacts::payload::PayloadError) -> InputContextError {
    match error {
        super::artefacts::payload::PayloadError::Credential => InputContextError::Credential,
        super::artefacts::payload::PayloadError::Bound => InputContextError::Bound,
        _ => InputContextError::Changed,
    }
}

#[cfg(test)]
mod tests;
