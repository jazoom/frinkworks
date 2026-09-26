use rig_core::completion::ToolDefinition;
use serde::Deserialize;
use std::time::Duration;

use crate::agents::{DirectoryPolicy, ToolId};
use crate::execution::{
    CommandFailure, CommandResult, CommandTermination, OutputScope, ToolLocation,
};
use crate::sandbox::{GuestExec, GuestSandbox};
use crate::sessions::Job;

mod edit;
pub(crate) mod read;

pub(crate) const MAXIMUM_TOOL_BYTES: usize = 64 * 1024;
pub(crate) const MAXIMUM_WRITE_BYTES: usize = 256 * 1024;
pub(crate) const MAXIMUM_COMMAND_BYTES: usize = 32_768;

/// A sandbox command deadline. It is separate from host command timeouts.
pub(crate) const SANDBOX_COMMAND_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(30)
} else {
    Duration::from_secs(120)
};

impl ToolId {
    fn description_for(self, location: ToolLocation) -> &'static str {
        match (self, location) {
            (Self::List, ToolLocation::Host) => {
                "List files on this computer with the Frinkworks process user's permissions."
            }
            (Self::Read, ToolLocation::Host) => {
                "Read a file on this computer, up to the 8 MiB scan limit. Offset is a 1-based line. Returns the next offset when more content remains."
            }
            (Self::Edit, ToolLocation::Host) => {
                "Apply exact-match replacements to one existing host file. Each search must occur once in the original file. Changes take effect immediately."
            }
            (Self::Write, ToolLocation::Host) => {
                "Write a host file. Creates parent directories. Use this for new files or complete replacements. Changes take effect immediately."
            }
            (Self::List, _) => "List files in a granted directory.",
            (Self::Read, _) => {
                "Read a file inside a granted directory, up to the 8 MiB scan limit. Offset is a 1-based line. Returns the next offset when more content remains."
            }
            (Self::Edit, _) => {
                "Apply exact-match replacements to one existing file inside a writable granted directory. Each search must occur once. Replacements use the original file, not earlier edits."
            }
            (Self::Write, _) => {
                "Write a file inside a writable granted directory. Creates parent directories. Use this for new files or complete replacements."
            }
            (Self::Run, ToolLocation::Host) => {
                "Run a shell command on this computer under the selected approval policy. Starts in the selected work location. Approval does not inspect script internals."
            }
            (Self::Run, ToolLocation::Sandbox) => {
                "Run a shell command in Microsandbox under the selected approval policy. Starts in the primary directory. Read mounts remain read-only."
            }
        }
    }

    fn parameters(self, location: ToolLocation) -> serde_json::Value {
        let path_description = match location {
            ToolLocation::Host => {
                "An absolute host path or a path relative to the selected work location. Defaults to the work location."
            }
            ToolLocation::Sandbox => {
                "Path inside a granted guest directory. Defaults to the first authorised directory or /workspace."
            }
        };
        match self {
            Self::List => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": path_description
                    }
                },
                "additionalProperties": false
            }),
            Self::Read => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": path_description
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based line to start from. Omit to start at the first line."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": read::MAXIMUM_PAGE_LINES,
                        "description": "Maximum number of lines to return."
                    }
                },
                "additionalProperties": false
            }),
            Self::Edit => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": path_description
                    },
                    "edits": {
                        "type": "array",
                        "description": "Exact-match replacements against the original file.",
                        "minItems": 1,
                        "maxItems": edit::MAXIMUM_EDIT_COUNT,
                        "items": {
                            "type": "object",
                            "properties": {
                                "search": {
                                    "type": "string",
                                    "description": "Non-empty text that must occur exactly once."
                                },
                                "replace": {
                                    "type": "string",
                                    "description": "Replacement text."
                                }
                            },
                            "required": ["search", "replace"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["path", "edits"],
                "additionalProperties": false
            }),
            Self::Write => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": path_description
                    },
                    "contents": {
                        "type": "string",
                        "description": "File contents."
                    }
                },
                "required": ["path", "contents"],
                "additionalProperties": false
            }),
            Self::Run => serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to run. Starts in the primary directory or selected host work location."
                    },
                    "explanation": {
                        "type": "string",
                        "description": "Why this command is necessary. The command approval shows this explanation."
                    }
                },
                "required": if location == ToolLocation::Host { vec!["command", "explanation"] } else { vec!["command"] },
                "additionalProperties": false
            }),
        }
    }
}

pub(crate) fn definitions_for(selected: &[ToolId], location: ToolLocation) -> Vec<ToolDefinition> {
    selected
        .iter()
        .copied()
        .map(|kind| ToolDefinition {
            name: kind.as_str().to_owned(),
            description: kind.description_for(location).to_owned(),
            parameters: kind.parameters(location),
        })
        .collect()
}

pub(crate) const READ_OUTPUT: &str = "read_output";
pub(crate) const ASK_USER: &str = crate::conversations::questions::ASK_USER;

pub(crate) fn ask_user_definition() -> ToolDefinition {
    ToolDefinition {
        name: ASK_USER.to_owned(),
        description: "Ask the user a question and wait for one answer. Use this for ordinary choices or short free text. The answer grants no command or directory authority.".to_owned(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to show the user."
                },
                "options": {
                    "type": "array",
                    "description": "Optional choices with unique identifiers.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "label": { "type": "string" }
                        },
                        "required": ["id", "label"],
                        "additionalProperties": false
                    }
                },
                "allow_free_text": {
                    "type": "boolean",
                    "description": "If true, the user can type an answer instead of choosing an option."
                }
            },
            "required": ["question"],
            "additionalProperties": false
        }),
    }
}

pub(crate) fn read_output_definition() -> ToolDefinition {
    ToolDefinition {
        name: READ_OUTPUT.to_owned(),
        description: "Read a later page of a retained command result. Use the reference shown with a command result. Offset is a 1-based line. Returns the next offset when more content remains.".to_owned(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "reference": {
                    "type": "string",
                    "description": "The reference shown with a retained command result."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "1-based line to start from. Omit to start at the first line."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": read::MAXIMUM_PAGE_LINES,
                    "description": "Maximum number of lines to return."
                }
            },
            "required": ["reference"],
            "additionalProperties": false
        }),
    }
}

pub(crate) fn definitions_for_step(
    selected: &[ToolId],
    outputs: &[crate::workflows::definition::RequiredOutput],
    location: ToolLocation,
) -> Vec<ToolDefinition> {
    let mut tools = definitions_for(selected, location);
    if outputs.iter().any(|output| {
        matches!(
            output.kind,
            crate::workflows::definition::OutputKind::Plan
                | crate::workflows::definition::OutputKind::ReviewReport
                | crate::workflows::definition::OutputKind::TestReport
        )
    }) {
        tools.push(submit_definition(outputs));
    }
    tools
}

fn submit_definition(outputs: &[crate::workflows::definition::RequiredOutput]) -> ToolDefinition {
    let keys: Vec<&str> = outputs
        .iter()
        .filter(|output| {
            matches!(
                output.kind,
                crate::workflows::definition::OutputKind::Plan
                    | crate::workflows::definition::OutputKind::ReviewReport
                    | crate::workflows::definition::OutputKind::TestReport
            )
        })
        .map(|output| output.key.as_str())
        .collect();
    ToolDefinition {
        name: SUBMIT_WORKFLOW_OUTPUT.to_owned(),
        description: "Submit a declared plan or report output for this step.".to_owned(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "enum": keys
                },
                "kind": {
                    "type": "string",
                    "enum": ["plan", "review-report", "test-report"]
                },
                "markdown": { "type": "string" },
                "verdict": {
                    "type": "string",
                    "enum": ["approved", "revision-required", "blocked"]
                },
                "outcome": {
                    "type": "string",
                    "enum": ["passed", "failed", "not-run"]
                }
            },
            "required": ["key", "kind"],
            "additionalProperties": false
        }),
    }
}

pub(crate) const SUBMIT_WORKFLOW_OUTPUT: &str = "submit_workflow_output";

#[derive(Clone, Debug)]
pub(crate) struct HostRunSpec {
    pub(crate) session: crate::sessions::SessionId,
    pub(crate) conversation: crate::conversations::ConversationId,
    pub(crate) execution_revision: u32,
    pub(crate) directory: std::path::PathBuf,
    pub(crate) settings: crate::execution::ExecutionSettings,
    pub(crate) run: Option<String>,
    pub(crate) step: Option<String>,
    pub(crate) attempt: Option<String>,
}

pub(crate) struct HostToolContext<'a> {
    pub(crate) state: &'a crate::state::AppState,
    pub(crate) settings: &'a crate::execution::ExecutionSettings,
    pub(crate) secret: Option<&'a str>,
    pub(crate) session: crate::sessions::SessionId,
    pub(crate) conversation: crate::conversations::ConversationId,
    pub(crate) execution_revision: u32,
    pub(crate) directory: std::path::PathBuf,
    pub(crate) run: Option<String>,
    pub(crate) step: Option<String>,
    pub(crate) attempt: Option<String>,
}

pub(crate) struct AgentToolContext<'a> {
    pub(crate) advertised_resources: &'a [crate::execution::ResourceSource],
    pub(crate) sandbox: Option<&'a GuestSandbox>,
    pub(crate) policy: &'a DirectoryPolicy,
    pub(crate) job: &'a Job,
    pub(crate) tools: &'a [ToolId],
    pub(crate) location: ToolLocation,
    pub(crate) host: Option<HostToolContext<'a>>,
    pub(crate) secret: Option<&'a str>,
    pub(crate) outputs: Option<&'a crate::execution::OutputStore>,
    pub(crate) output_scope: Option<OutputScope>,
    pub(crate) output_drafts:
        Option<&'a std::sync::Mutex<crate::workflows::artefacts::output::OutputDrafts>>,
    pub(crate) required_outputs: &'a [crate::workflows::definition::RequiredOutput],
    pub(crate) questions: Option<QuestionToolContext<'a>>,
}

pub(crate) struct QuestionToolContext<'a> {
    pub(crate) state: &'a crate::state::AppState,
    pub(crate) session: crate::sessions::SessionId,
    pub(crate) conversation: crate::conversations::ConversationId,
}

pub(crate) struct ToolTrace {
    pub(crate) resource: Option<crate::execution::ResourceSource>,
    pub(crate) label: String,
    pub(crate) output: String,
    pub(crate) failure: Option<ToolFailureKind>,
    pub(crate) command: Option<crate::execution::CommandResult>,
}

/// Ordinary errors and explicit rejections have no unsettled effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolFailureKind {
    Ordinary,
    Rejected,
    Authority,
    Persistence,
    Cancellation,
    Uncertain,
}

impl ToolFailureKind {
    pub(crate) fn stops_loop(self) -> bool {
        !matches!(self, Self::Ordinary | Self::Rejected)
    }

    pub(crate) fn from_termination(termination: CommandTermination) -> Self {
        match termination {
            CommandTermination::Exited(_) | CommandTermination::NotDispatched => Self::Ordinary,
            CommandTermination::Cancelled => Self::Cancellation,
            CommandTermination::StorageFailure => Self::Persistence,
            CommandTermination::TimedOut
            | CommandTermination::ResourceLimit
            | CommandTermination::Unknown => Self::Uncertain,
        }
    }
}

struct ToolRun {
    resource: Option<crate::execution::ResourceSource>,
    label: String,
    output: String,
    command: Option<crate::execution::CommandResult>,
}

impl ToolRun {
    fn plain(label: String, output: String) -> Self {
        Self {
            resource: None,
            label,
            output,
            command: None,
        }
    }
}

enum ToolFailure {
    Ordinary(&'static str),
    Authority(&'static str),
    Persistence(&'static str),
    Cancellation,
    Rejected {
        label: String,
    },
    Command {
        label: String,
        failure: crate::execution::CommandFailure,
    },
}

impl ToolTrace {
    fn success(label: String, output: String, command: Option<CommandResult>) -> Self {
        let failure = command.as_ref().and_then(|command| {
            command
                .is_error()
                .then(|| ToolFailureKind::from_termination(command.termination))
        });
        Self {
            resource: None,
            label,
            output,
            failure,
            command,
        }
    }

    fn fail(
        label: String,
        output: String,
        failure: ToolFailureKind,
        command: Option<CommandResult>,
    ) -> Self {
        Self {
            resource: None,
            label,
            output,
            failure: Some(failure),
            command,
        }
    }
}

pub(crate) async fn invoke(
    context: &AgentToolContext<'_>,
    call_id: &str,
    name: &str,
    arguments: &serde_json::Value,
) -> ToolTrace {
    if context.job.cancel_requested() {
        return ToolTrace::fail(
            name.to_owned(),
            "Stopped.".to_owned(),
            ToolFailureKind::Cancellation,
            None,
        );
    }
    if name == SUBMIT_WORKFLOW_OUTPUT {
        return submit_output(context, arguments);
    }
    if name == READ_OUTPUT {
        return read_output(context, arguments);
    }
    if name == ASK_USER {
        return ask_user(context, call_id, arguments).await;
    }
    let Some(kind) = authorised_tool(context.tools, name) else {
        return ToolTrace::fail(
            name.to_owned(),
            "That tool is not available.".to_owned(),
            ToolFailureKind::Authority,
            None,
        );
    };
    match dispatch(context, call_id, kind, arguments).await {
        Ok(run) => {
            let mut trace = ToolTrace::success(run.label, run.output, run.command);
            trace.resource = run.resource;
            trace
        }
        Err(ToolFailure::Ordinary(message)) => ToolTrace::fail(
            kind.as_str().to_owned(),
            message.to_owned(),
            ToolFailureKind::Ordinary,
            None,
        ),
        Err(ToolFailure::Authority(message)) => ToolTrace::fail(
            kind.as_str().to_owned(),
            message.to_owned(),
            ToolFailureKind::Authority,
            None,
        ),
        Err(ToolFailure::Persistence(message)) => ToolTrace::fail(
            kind.as_str().to_owned(),
            message.to_owned(),
            ToolFailureKind::Persistence,
            None,
        ),
        Err(ToolFailure::Cancellation) => ToolTrace::fail(
            kind.as_str().to_owned(),
            "Stopped.".to_owned(),
            ToolFailureKind::Cancellation,
            None,
        ),
        Err(ToolFailure::Rejected { label }) => ToolTrace::fail(
            label,
            "The user rejected this command.".to_owned(),
            ToolFailureKind::Rejected,
            None,
        ),
        Err(ToolFailure::Command { label, mut failure }) => {
            failure.result = retain_command(context, call_id, failure.result);
            ToolTrace::fail(
                label,
                failure.report(),
                ToolFailureKind::from_termination(failure.result.termination),
                Some(failure.result),
            )
        }
    }
}

fn authorised_tool(selected: &[ToolId], name: &str) -> Option<ToolId> {
    ToolId::parse(name).filter(|kind| selected.contains(kind))
}

fn read_output(context: &AgentToolContext<'_>, arguments: &serde_json::Value) -> ToolTrace {
    let Some(outputs) = context.outputs else {
        return plain_failure(
            READ_OUTPUT,
            "That command output is not available.",
            ToolFailureKind::Ordinary,
        );
    };
    let Some(scope) = context.output_scope.clone() else {
        return plain_failure(
            READ_OUTPUT,
            "That command output is not available.",
            ToolFailureKind::Ordinary,
        );
    };
    let reference = arguments
        .get("reference")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if !valid_output_reference(reference) {
        return plain_failure(
            READ_OUTPUT,
            "That command output reference is not valid.",
            ToolFailureKind::Ordinary,
        );
    }
    let request = match page_request(arguments) {
        Ok(request) => request,
        Err(message) => {
            return plain_failure(READ_OUTPUT, message, ToolFailureKind::Ordinary);
        }
    };
    match outputs.model_page(reference, &scope, request) {
        Ok(page) => {
            let mut output = String::new();
            for chunk in &page.chunks {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str(chunk.stream.label());
                output.push_str(": ");
                output.push_str(&chunk.text);
            }
            if let Some(next) = page.next {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str(&format!(
                    "More output remains. Read again with offset={next}."
                ));
            }
            if page.line_truncated {
                output.push_str("\nThe last included line exceeded the byte limit.");
            }
            if page.truncated {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str("The retained output reached its storage limit.");
            }
            if output.is_empty() {
                output.push_str("(no output)");
            }
            ToolTrace::success(format!("read_output `{reference}`"), output, None)
        }
        Err(error) => plain_failure(READ_OUTPUT, error.message(), ToolFailureKind::Ordinary),
    }
}

async fn ask_user(
    context: &AgentToolContext<'_>,
    call_id: &str,
    arguments: &serde_json::Value,
) -> ToolTrace {
    let Some(questions) = context.questions.as_ref() else {
        return plain_failure(
            ASK_USER,
            "That question tool is not available.",
            ToolFailureKind::Authority,
        );
    };
    if !questions.state.sessions.contains_live(&questions.session)
        || !questions.state.sessions.owns_conversation_job(
            &questions.session,
            questions.conversation,
            context.job.id(),
        )
    {
        return plain_failure(
            ASK_USER,
            "That question tool is not available.",
            ToolFailureKind::Authority,
        );
    }
    let question = match crate::conversations::questions::parse_question(
        arguments,
        questions.conversation,
        context.job.id(),
        questions.session,
        call_id,
    ) {
        Ok(question) => question,
        Err(error) => {
            return plain_failure(ASK_USER, error.message(), ToolFailureKind::Ordinary);
        }
    };
    if let Err(error) = questions
        .state
        .conversations
        .submit_question(question.clone())
    {
        return plain_failure(ASK_USER, error.message(), ToolFailureKind::Ordinary);
    }
    if context.job.set_awaiting_question().is_none() && context.job.cancel_requested() {
        questions
            .state
            .conversations
            .invalidate_questions_for_job(context.job.id());
        return ToolTrace::fail(
            ASK_USER.to_owned(),
            "Stopped.".to_owned(),
            ToolFailureKind::Cancellation,
            None,
        );
    }
    let decision = questions
        .state
        .conversations
        .wait_question(questions.conversation, context.job, call_id)
        .await;
    let _ = context.job.resume();
    match decision {
        Ok(answer) => {
            let failure = matches!(answer, crate::conversations::QuestionAnswer::Cancelled)
                .then_some(ToolFailureKind::Rejected);
            let output = answer.result_text();
            ToolTrace {
                resource: None,
                label: ASK_USER.to_owned(),
                output,
                failure,
                command: None,
            }
        }
        Err(error) => {
            let failure = if context.job.cancel_requested()
                || error == crate::conversations::QuestionError::JobCancelled
            {
                ToolFailureKind::Cancellation
            } else {
                ToolFailureKind::Ordinary
            };
            ToolTrace::fail(
                ASK_USER.to_owned(),
                error.message().to_owned(),
                failure,
                None,
            )
        }
    }
}

fn plain_failure(label: &str, message: &'static str, kind: ToolFailureKind) -> ToolTrace {
    ToolTrace::fail(label.to_owned(), message.to_owned(), kind, None)
}

/// A reference is server-generated lowercase hexadecimal. A model-supplied path
/// can never select stored output.
pub(crate) fn valid_output_reference(reference: &str) -> bool {
    reference.len() == 32 && reference.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn submit_output(context: &AgentToolContext<'_>, arguments: &serde_json::Value) -> ToolTrace {
    let Some(drafts) = context.output_drafts else {
        return ToolTrace::fail(
            SUBMIT_WORKFLOW_OUTPUT.to_owned(),
            "That tool is not available.".to_owned(),
            ToolFailureKind::Ordinary,
            None,
        );
    };
    let key = arguments
        .get("key")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let kind = arguments
        .get("kind")
        .and_then(|value| value.as_str())
        .and_then(crate::workflows::definition::OutputKind::parse);
    let Some(kind) = kind else {
        return ToolTrace::fail(
            SUBMIT_WORKFLOW_OUTPUT.to_owned(),
            crate::workflows::artefacts::output::OutputDraftError::Kind
                .message()
                .to_owned(),
            ToolFailureKind::Ordinary,
            None,
        );
    };
    let markdown = arguments
        .get("markdown")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let verdict = arguments.get("verdict").and_then(|value| value.as_str());
    let outcome = arguments.get("outcome").and_then(|value| value.as_str());
    let candidate = arguments.get("candidate").is_some();
    let human = arguments.get("decision").is_some();
    let mut guard = drafts
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match guard.submit(
        context.required_outputs,
        key,
        kind,
        markdown,
        verdict,
        outcome,
        candidate,
        human,
    ) {
        Ok(()) => ToolTrace::success(format!("submit `{key}`"), "Stored.".to_owned(), None),
        Err(error) => ToolTrace::fail(
            SUBMIT_WORKFLOW_OUTPUT.to_owned(),
            error.message().to_owned(),
            ToolFailureKind::Ordinary,
            None,
        ),
    }
}

async fn dispatch(
    context: &AgentToolContext<'_>,
    call_id: &str,
    kind: ToolId,
    arguments: &serde_json::Value,
) -> Result<ToolRun, ToolFailure> {
    if context.location == ToolLocation::Host {
        let host = context
            .host
            .as_ref()
            .ok_or(ToolFailure::Authority("Host execution is not available."))?;
        validate_host_dispatch(host, context.job).map_err(ToolFailure::Authority)?;
    }
    match kind {
        ToolId::List => {
            let args: PathArgs = parse_args(arguments).map_err(ToolFailure::Ordinary)?;
            let (path, _) = context
                .policy
                .resolve(&args.path)
                .map_err(ToolFailure::Authority)?;
            let output = plain_capture(
                format!("list `{path}`"),
                capture(
                    context,
                    call_id,
                    confined_existing_command(&path, &context.policy.guest_roots(), "ls", &["-la"]),
                )
                .await,
            )?;
            Ok(ToolRun::plain(format!("list `{path}`"), output))
        }
        ToolId::Read => read_file(context, call_id, arguments).await,
        ToolId::Edit => edit_file(context, call_id, arguments).await,
        ToolId::Write => {
            let args: WriteArgs = parse_args(arguments).map_err(ToolFailure::Ordinary)?;
            let (path, access) = context
                .policy
                .resolve(&args.path)
                .map_err(ToolFailure::Authority)?;
            if !access.is_writable() {
                return Err(ToolFailure::Authority("That path is read-only."));
            }
            if path == context.policy.primary_guest()
                || context
                    .policy
                    .grants()
                    .iter()
                    .any(|grant| grant.guest_path == path)
            {
                return Err(ToolFailure::Ordinary("Choose a file to write."));
            }
            if args.contents.len() > MAXIMUM_WRITE_BYTES {
                return Err(ToolFailure::Ordinary("That file is too large to write."));
            }
            let output = plain_capture(
                format!("write `{path}`"),
                capture(
                    context,
                    call_id,
                    confined_write_command(&path, &context.policy.writable_roots())
                        .with_stdin(args.contents.into_bytes()),
                )
                .await,
            )?;
            let body = if output.trim().is_empty() {
                "Wrote the file.".to_owned()
            } else {
                output
            };
            Ok(ToolRun::plain(format!("write `{path}`"), body))
        }
        ToolId::Run => {
            let args: RunArgs = parse_args(arguments).map_err(ToolFailure::Ordinary)?;
            let command = args.command.trim();
            if command.is_empty() {
                return Err(ToolFailure::Ordinary("Enter a command."));
            }
            if command.len() > MAXIMUM_COMMAND_BYTES {
                return Err(ToolFailure::Ordinary("That command is too long."));
            }
            if context.location == ToolLocation::Host {
                if args.explanation.trim().is_empty() {
                    return Err(ToolFailure::Ordinary(
                        "Explain why this command is necessary.",
                    ));
                }
                return host_run(context, call_id, command, args.explanation.trim()).await;
            }
            let host = context
                .host
                .as_ref()
                .ok_or(ToolFailure::Authority("Command approval is unavailable."))?;
            validate_host_dispatch(host, context.job).map_err(ToolFailure::Authority)?;
            let mut request = crate::execution::HostCommandRequest {
                token: String::new(),
                session: host.session,
                job: context.job.id(),
                conversation: host.conversation,
                execution_revision: host.execution_revision,
                command: command.to_owned(),
                directory: std::path::PathBuf::from(context.policy.primary_guest()),
                explanation: args.explanation.trim().to_owned(),
                run: host.run.clone(),
                step: host.step.clone(),
                attempt: host.attempt.clone(),
            };
            crate::execution::approval::approve_command(
                host.state,
                &mut request,
                host.settings,
                context.job,
                host.secret,
            )
            .await
            .map_err(ToolFailure::Authority)?;
            validate_host_dispatch(host, context.job).map_err(ToolFailure::Authority)?;
            let label = format!("run `{command}`");
            record_host_evidence(host, &request, "dispatching", "", None)
                .map_err(ToolFailure::Persistence)?;
            let sandbox = context
                .sandbox
                .ok_or(ToolFailure::Authority("That tool is not available."))?;
            let key = crate::execution::OutputKey {
                scope: host_output_scope(host, &request),
                job: context.job.id(),
                tool_call: redact(call_id, context.secret),
                model_hidden: false,
            };
            let result = crate::execution::command::capture_sandbox_command(
                sandbox,
                GuestExec::shell(command).in_dir(context.policy.primary_guest()),
                context.job,
                context.secret,
                SANDBOX_COMMAND_TIMEOUT,
                &key.tool_call,
                Some((&host.state.outputs, &key)),
            )
            .await;
            let output = match &result {
                Ok(output) => output,
                Err(failure) => &failure.result,
            };
            record_host_evidence(
                host,
                &request,
                if output.is_success() {
                    "completed"
                } else {
                    "failed"
                },
                &output.report(),
                Some(output),
            )
            .map_err(ToolFailure::Persistence)?;
            match result {
                Ok(result) => {
                    let output = result.report();
                    Ok(ToolRun {
                        resource: None,
                        label,
                        output,
                        command: Some(result),
                    })
                }
                Err(failure) => Err(ToolFailure::Command { label, failure }),
            }
        }
    }
}

fn plain_capture(
    label: String,
    result: Result<CommandResult, CommandFailure>,
) -> Result<String, ToolFailure> {
    match result {
        Ok(result) if result.is_success() => Ok(result
            .chunks
            .iter()
            .filter(|chunk| chunk.stream == crate::execution::CommandStream::Stdout)
            .map(|chunk| chunk.text.as_str())
            .collect()),
        Ok(result) => Err(ToolFailure::Command {
            label,
            failure: CommandFailure::new(result, "The file tool command failed."),
        }),
        Err(failure) => Err(ToolFailure::Command { label, failure }),
    }
}

async fn host_run(
    context: &AgentToolContext<'_>,
    call_id: &str,
    command: &str,
    explanation: &str,
) -> Result<ToolRun, ToolFailure> {
    let label = format!("run `{command}`");
    let host = context
        .host
        .as_ref()
        .ok_or(ToolFailure::Authority("Host execution is not available."))?;
    validate_host_dispatch(host, context.job).map_err(ToolFailure::Authority)?;
    let mut request = crate::execution::HostCommandRequest {
        token: String::new(),
        session: host.session,
        job: context.job.id(),
        conversation: host.conversation,
        execution_revision: host.execution_revision,
        command: command.to_owned(),
        directory: host.directory.clone(),
        explanation: explanation.to_owned(),
        run: host.run.clone(),
        step: host.step.clone(),
        attempt: host.attempt.clone(),
    };
    match crate::execution::approval::approve_command(
        host.state,
        &mut request,
        host.settings,
        context.job,
        host.secret,
    )
    .await
    {
        Ok(()) => dispatch_host_command(host, context.job, call_id, label, &request).await,
        Err(_) if context.job.cancel_requested() => Err(ToolFailure::Cancellation),
        Err("The user rejected this command.") => Err(ToolFailure::Rejected { label }),
        Err(error) => Err(ToolFailure::Authority(error)),
    }
}

fn record_host_evidence(
    host: &HostToolContext<'_>,
    request: &crate::execution::HostCommandRequest,
    status: &str,
    output: &str,
    command: Option<&crate::execution::CommandResult>,
) -> Result<(), &'static str> {
    // Keep token-scoped approval evidence even without a workflow attempt.
    host.state
        .workflow_evidence
        .host_command(request, status, output, host.secret, command)
        .map_err(|error| error.message())
}

async fn dispatch_host_command(
    host: &HostToolContext<'_>,
    job: &Job,
    call_id: &str,
    label: String,
    request: &crate::execution::HostCommandRequest,
) -> Result<ToolRun, ToolFailure> {
    validate_host_dispatch(host, job).map_err(ToolFailure::Authority)?;
    record_host_evidence(host, request, "dispatching", "", None)
        .map_err(ToolFailure::Persistence)?;
    let visible_call_id = redact(call_id, host.secret);
    let reporter = crate::execution::CommandReporter {
        tool_call: &visible_call_id,
        secret: host.secret,
        outputs: &host.state.outputs,
        output_key: crate::execution::OutputKey {
            scope: host_output_scope(host, request),
            job: job.id(),
            tool_call: visible_call_id.clone(),
            model_hidden: false,
        },
    };
    let require_success = host.run.is_some();
    let result = crate::execution::run_shell_reported(
        &request.command,
        &host.directory,
        job,
        crate::execution::COMMAND_TIMEOUT,
        require_success,
        &reporter,
    )
    .await;
    match result {
        Ok(command) => {
            let command = command.redacted(host.secret);
            let output = command.report();
            if record_host_evidence(host, request, "finished", &output, Some(&command)).is_err() {
                let mut command = command;
                command.termination = CommandTermination::StorageFailure;
                return Err(ToolFailure::Command {
                    label,
                    failure: crate::execution::CommandFailure::new(
                        command,
                        "Frinkworks could not store command output. Command effects can remain incomplete.",
                    ),
                });
            }
            Ok(ToolRun {
                resource: None,
                label,
                output,
                command: Some(command),
            })
        }
        Err(mut failure) => {
            failure.result = failure.result.redacted(host.secret);
            let output = failure.report();
            if let Err(message) =
                record_host_evidence(host, request, "failed", &output, Some(&failure.result))
            {
                failure.message = message;
                failure.result.termination = CommandTermination::StorageFailure;
            }
            Err(ToolFailure::Command { label, failure })
        }
    }
}

fn host_output_scope(
    host: &HostToolContext<'_>,
    request: &crate::execution::HostCommandRequest,
) -> OutputScope {
    OutputScope {
        conversation: Some(host.conversation),
        run: request
            .run
            .as_deref()
            .and_then(crate::workflows::RunId::parse),
        attempt: request
            .attempt
            .as_deref()
            .and_then(crate::workflows::AttemptId::parse),
    }
}

fn retain_command(
    context: &AgentToolContext<'_>,
    call_id: &str,
    command: CommandResult,
) -> CommandResult {
    if command.chunks.is_empty() || command.retained.is_some() {
        return command;
    }
    let command = command.redacted(context.secret);
    let (Some(outputs), Some(scope)) = (context.outputs, &context.output_scope) else {
        return command.bounded(crate::execution::OUTPUT_PREVIEW_BYTES).0;
    };
    let key = crate::execution::OutputKey {
        scope: scope.clone(),
        job: context.job.id(),
        tool_call: redact(call_id, context.secret),
        model_hidden: false,
    };
    match outputs.store(&key, &command) {
        Ok(retained) => command.retain(retained),
        Err(_) => command.into_storage_limit(),
    }
}

fn validate_host_dispatch(host: &HostToolContext<'_>, job: &Job) -> Result<(), &'static str> {
    let record = host
        .state
        .conversations
        .get(&host.conversation)
        .ok_or("This conversation is not available.")?;
    if job.cancel_requested() || !host.state.sessions.contains_live(&host.session) {
        return Err("Host access expired or changed. Approve the current settings again.");
    }
    if host.run.is_some() {
        validate_workflow_host_dispatch(host, job, &record)?;
    } else if record.active_job != Some(job.id())
        || record
            .model
            .as_ref()
            .is_none_or(|model| model.settings != *host.settings)
        || (host.settings.location == ToolLocation::Host
            && (!host.settings.host_access_allowed()
                || !host.state.access_consent.authorised_host_conversation(
                    host.session,
                    host.conversation,
                    host.settings,
                )))
    {
        return Err("Host access expired or changed. Approve the current settings again.");
    }
    for grant in &host.settings.directories {
        grant
            .revalidate()
            .map_err(|_| "A work location changed or is not available.")?;
        if host.settings.location == ToolLocation::Sandbox
            && host.run.is_none()
            && grant.requires_access_consent(host.state.local_data.root())
            && !host.state.access_consent.authorised_conversation(
                host.session,
                host.conversation,
                host.settings,
                grant,
            )
            && !host.state.conversations.directory_approved(
                &host.conversation,
                host.settings,
                grant,
            )
        {
            return Err("Directory access needs explicit approval.");
        }
    }
    Ok(())
}

fn validate_workflow_host_dispatch(
    host: &HostToolContext<'_>,
    job: &Job,
    record: &crate::conversations::ConversationRecord,
) -> Result<(), &'static str> {
    if record.active_job != Some(job.id()) {
        return Err("Host access expired or changed. Approve the current settings again.");
    }
    let run_id = host
        .run
        .as_deref()
        .and_then(crate::workflows::RunId::parse)
        .ok_or("That host command is not bound to this run.")?;
    let step = host
        .step
        .as_deref()
        .and_then(|step| crate::workflows::definition::StepKey::parse(step).ok())
        .ok_or("That host command is not bound to this step.")?;
    let run = host
        .state
        .workflow_runs
        .get(&run_id)
        .ok_or("The workflow run is unavailable.")?;
    if run.conversation_id != Some(host.conversation)
        || run
            .active_attempt()
            .and_then(|id| run.attempts.iter().find(|attempt| attempt.id == id))
            .is_none_or(|attempt| {
                Some(attempt.id.as_hex()) != host.attempt
                    || attempt.step != step
                    || attempt.state != crate::workflows::run::AttemptState::Active
                    || (host.settings.location == ToolLocation::Host
                        && attempt.sandbox.kind
                            != crate::workflows::run::AttemptSandboxKind::HostExecution)
            })
    {
        return Err("That host command is not bound to the active run, step and attempt.");
    }
    let settings = run
        .phase_settings(&step)
        .cloned()
        .or_else(|| run.directory_settings())
        .ok_or("The pinned host settings are unavailable.")?;
    if settings != *host.settings || !settings.host_access_allowed() {
        return Err("Host access expired or changed. Approve the current settings again.");
    }
    let launch_ok = host.state.access_consent.authorised_launch(
        run_id,
        host.session,
        host.conversation,
        host.settings,
    );
    if !launch_ok {
        return Err("Host access needs explicit approval for this run.");
    }
    Ok(())
}

#[derive(Deserialize)]
struct PathArgs {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
struct ReadArgs {
    #[serde(default)]
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
}

#[derive(Deserialize)]
struct EditArgs {
    path: String,
    edits: Vec<edit::Change>,
}

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    contents: String,
}

#[derive(Deserialize)]
struct RunArgs {
    command: String,
    #[serde(default)]
    explanation: String,
}

fn parse_args<T: for<'de> Deserialize<'de>>(
    arguments: &serde_json::Value,
) -> Result<T, &'static str> {
    serde_json::from_value(arguments.clone()).map_err(|_| "Those tool arguments are not valid.")
}

fn page_request(arguments: &serde_json::Value) -> Result<read::PageRequest, &'static str> {
    let offset = match arguments.get("offset") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or("Read offset must be a 1-based line number.")?,
        ),
    };
    let limit = match arguments.get("limit") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or("Read limit must be a positive line count.")?,
        ),
    };
    read::parse_request(offset, limit).map_err(read::PageError::message)
}

fn existing_file_path(context: &AgentToolContext<'_>, raw: &str) -> Result<String, ToolFailure> {
    let (path, _) = context
        .policy
        .resolve(raw)
        .map_err(ToolFailure::Authority)?;
    reject_grant_root(context, &path, "Choose a file to read.")?;
    Ok(path)
}

fn reject_grant_root(
    context: &AgentToolContext<'_>,
    path: &str,
    message: &'static str,
) -> Result<(), ToolFailure> {
    if path == context.policy.primary_guest()
        || context
            .policy
            .grants()
            .iter()
            .any(|grant| grant.guest_path == path)
        || context
            .policy
            .skill_root()
            .is_some_and(|root| root.guest_path == path)
    {
        return Err(ToolFailure::Ordinary(message));
    }
    Ok(())
}

async fn read_file(
    context: &AgentToolContext<'_>,
    call_id: &str,
    arguments: &serde_json::Value,
) -> Result<ToolRun, ToolFailure> {
    let args: ReadArgs = parse_args(arguments).map_err(ToolFailure::Ordinary)?;
    let request = read::parse_request(args.offset, args.limit)
        .map_err(|error| ToolFailure::Ordinary(error.message()))?;
    let path = existing_file_path(context, &args.path)?;
    let skill = crate::execution::resources::skill_directory(&path).is_some()
        || context
            .policy
            .skill_root()
            .is_some_and(|root| path.starts_with(&format!("{}/", root.guest_path)));
    let scan = if skill {
        crate::execution::resources::MAXIMUM_SKILL_BODY_BYTES
    } else {
        read::MAXIMUM_SCAN_BYTES
    };
    let bytes = capture_stdout_bytes(
        context,
        call_id,
        confined_read_command(&path, &context.policy.guest_roots(), scan + 1),
    )
    .await?;
    if bytes.len() > scan {
        return Err(ToolFailure::Ordinary(if skill {
            "That skill file exceeds the read scan limit."
        } else {
            "That file exceeds the read scan limit."
        }));
    }
    let text = read::decode_text(&bytes).map_err(|error| ToolFailure::Ordinary(error.message()))?;
    if skill {
        crate::execution::resources::validate_skill_read(&path, &bytes)
            .map_err(|error| ToolFailure::Ordinary(error.message()))?;
    }
    let page =
        read::page_text(text, request).map_err(|error| ToolFailure::Ordinary(error.message()))?;
    let resource = if skill || path.ends_with("/AGENTS.md") {
        let scope = context
            .policy
            .resource_scope(&path)
            .ok_or(ToolFailure::Authority(
                "That resource has no authorised root.",
            ))?;
        let source = crate::execution::ResourceSource::new(
            if skill {
                crate::execution::ResourceKind::Skill
            } else {
                crate::execution::ResourceKind::Instruction
            },
            scope,
            &path,
            &bytes,
        );
        source
            .validate_read(&bytes, context.advertised_resources, context.secret)
            .map_err(|error| ToolFailure::Ordinary(error.message()))?;
        Some(source)
    } else {
        None
    };
    Ok(ToolRun {
        label: format!("read `{path}`"),
        output: read::render_file_page(&page),
        command: None,
        resource,
    })
}

async fn edit_file(
    context: &AgentToolContext<'_>,
    call_id: &str,
    arguments: &serde_json::Value,
) -> Result<ToolRun, ToolFailure> {
    let args: EditArgs = parse_args(arguments).map_err(ToolFailure::Ordinary)?;
    let (path, access) = context
        .policy
        .resolve(&args.path)
        .map_err(ToolFailure::Authority)?;
    if !access.is_writable() {
        return Err(ToolFailure::Authority("That path is read-only."));
    }
    reject_grant_root(context, &path, "Choose a file to edit.")?;
    let original_bytes = capture_stdout_bytes(
        context,
        call_id,
        confined_read_command(
            &path,
            &context.policy.guest_roots(),
            MAXIMUM_WRITE_BYTES.saturating_add(1),
        ),
    )
    .await?;
    if original_bytes.len() > MAXIMUM_WRITE_BYTES {
        return Err(ToolFailure::Ordinary("That file is too large to edit."));
    }
    let original = read::decode_text(&original_bytes)
        .map_err(|error| ToolFailure::Ordinary(error.message()))?;
    let next = edit::apply(original, &args.edits)
        .map_err(|error| ToolFailure::Ordinary(error.message()))?;
    let output = plain_capture(
        format!("edit `{path}`"),
        capture(
            context,
            call_id,
            confined_edit_command(&path, &context.policy.writable_roots())
                .with_stdin(edit_payload(&original_bytes, next.as_bytes())),
        )
        .await,
    )?;
    let body = if output.trim().is_empty() {
        "Updated the file.".to_owned()
    } else {
        output
    };
    Ok(ToolRun::plain(format!("edit `{path}`"), body))
}

fn edit_payload(original: &[u8], next: &[u8]) -> Vec<u8> {
    let mut payload = format!(
        "{}
{}
",
        original.len(),
        next.len()
    )
    .into_bytes();
    payload.extend_from_slice(original);
    payload.extend_from_slice(next);
    payload
}

const CONFINED_EXISTING_SCRIPT: &str = r#"
roots=$1
resolved=$(realpath "$2") || { printf '%s\n' 'That path does not exist.'; exit 1; }
ok=0
oldifs=$IFS
IFS=:
for root in $roots; do
    root=${root%/}
    case "$resolved" in
        "$root"|"$root"/*) ok=1 ;;
    esac
done
IFS=$oldifs
if [ "$ok" -ne 1 ]; then
    printf '%s\n' 'Stay inside a granted directory.'
    exit 1
fi
shift 2
exec "$@" -- "$resolved"
"#;

fn confined_existing_command(
    path: &str,
    roots: &[String],
    program: &str,
    args: &[&str],
) -> GuestExec {
    let mut command_args = vec![
        "-c".to_owned(),
        CONFINED_EXISTING_SCRIPT.to_owned(),
        "project-path".to_owned(),
        encode_roots(roots),
        path.to_owned(),
        program.to_owned(),
    ];
    command_args.extend(args.iter().map(|arg| (*arg).to_owned()));
    GuestExec::command("sh", command_args)
}

const CONFINED_READ_SCRIPT: &str = r#"
roots=$1
resolved=$(realpath "$2") || { printf '%s\n' 'That path does not exist.'; exit 1; }
ok=0
oldifs=$IFS
IFS=:
for root in $roots; do
    root=${root%/}
    case "$resolved" in
        "$root"|"$root"/*) ok=1 ;;
    esac
done
IFS=$oldifs
if [ "$ok" -ne 1 ]; then
    printf '%s\n' 'Stay inside a granted directory.'
    exit 4
fi
if [ -d "$resolved" ]; then
    printf '%s\n' 'That path is a directory.'
    exit 2
fi
if [ ! -f "$resolved" ]; then
    printf '%s\n' 'That path is not a file.'
    exit 3
fi
head -c "$3" -- "$resolved"
"#;

fn confined_read_command(path: &str, roots: &[String], scan: usize) -> GuestExec {
    GuestExec::command(
        "sh",
        vec![
            "-c".to_owned(),
            CONFINED_READ_SCRIPT.to_owned(),
            "project-read".to_owned(),
            encode_roots(roots),
            path.to_owned(),
            scan.to_string(),
        ],
    )
}

const CONFINED_WRITE_SCRIPT: &str = r#"
roots=$1
target=$2
parent=${target%/*}
ancestor=$parent
suffix=
while [ ! -e "$ancestor" ] && [ ! -L "$ancestor" ]; do
    name=${ancestor##*/}
    suffix=/$name$suffix
    ancestor=${ancestor%/*}
done
resolved=$(realpath "$ancestor") || { printf '%s\n' 'That path is not valid.'; exit 1; }
ok=0
oldifs=$IFS
IFS=:
for root in $roots; do
    root=${root%/}
    case "$resolved" in
        "$root"|"$root"/*) ok=1 ;;
    esac
done
IFS=$oldifs
if [ "$ok" -ne 1 ]; then
    printf '%s\n' 'Stay inside a granted directory.'
    exit 1
fi
parent=$resolved$suffix
mkdir -p -- "$parent" || exit 1
resolved=$(realpath "$parent") || { printf '%s\n' 'That path is not valid.'; exit 1; }
ok=0
oldifs=$IFS
IFS=:
for root in $roots; do
    root=${root%/}
    case "$resolved" in
        "$root"|"$root"/*) ok=1 ;;
    esac
done
IFS=$oldifs
if [ "$ok" -ne 1 ]; then
    printf '%s\n' 'Stay inside a granted directory.'
    exit 1
fi
target=$resolved/${target##*/}
if [ -L "$target" ]; then
    target=$(realpath "$target") || { printf '%s\n' 'That path is not valid.'; exit 1; }
    ok=0
    oldifs=$IFS
    IFS=:
    for root in $roots; do
        root=${root%/}
        case "$target" in
            "$root"/*) ok=1 ;;
        esac
    done
    IFS=$oldifs
    if [ "$ok" -ne 1 ]; then
        printf '%s\n' 'Stay inside a granted directory.'
        exit 1
    fi
fi
cat > "$target"
"#;

fn confined_write_command(path: &str, roots: &[String]) -> GuestExec {
    GuestExec::command(
        "sh",
        vec![
            "-c".to_owned(),
            CONFINED_WRITE_SCRIPT.to_owned(),
            "project-write".to_owned(),
            encode_roots(roots),
            path.to_owned(),
        ],
    )
}

const CONFINED_EDIT_SCRIPT: &str = r#"
roots=$1
resolved=$(realpath "$2") || { printf '%s\n' 'That path does not exist.'; exit 1; }
ok=0
oldifs=$IFS
IFS=:
for root in $roots; do
    root=${root%/}
    case "$resolved" in
        "$root"|"$root"/*) ok=1 ;;
    esac
done
IFS=$oldifs
if [ "$ok" -ne 1 ]; then
    printf '%s\n' 'Stay inside a granted directory.'
    exit 4
fi
if [ -d "$resolved" ]; then
    printf '%s\n' 'That path is a directory.'
    exit 2
fi
if [ ! -f "$resolved" ]; then
    printf '%s\n' 'That path is not a file.'
    exit 3
fi
IFS= read -r original_bytes || exit 1
IFS= read -r new_bytes || exit 1
case $original_bytes in
    ''|*[!0-9]*) printf '%s\n' 'Those tool arguments are not valid.'; exit 1 ;;
esac
case $new_bytes in
    ''|*[!0-9]*) printf '%s\n' 'Those tool arguments are not valid.'; exit 1 ;;
esac
dir=${resolved%/*}
cd -P -- "$dir" || exit 1
[ "$(pwd -P)" = "$dir" ] || exit 4
target=./${resolved##*/}
[ ! -L "$target" ] || exit 4
umask 077
tmp=$(mktemp -d .frinkworks-edit.XXXXXXXXXX) || exit 1
orig=$tmp/original
next=$tmp/next
trap 'rm -f -- "$orig" "$next"; rmdir -- "$tmp"' EXIT
dd of="$orig" bs=1 count="$original_bytes" 2>/dev/null || exit 1
[ "$(wc -c < "$orig")" -eq "$original_bytes" ] || exit 1
cp -p -- "$target" "$next" || exit 1
dd of="$next" bs=1 count="$new_bytes" 2>/dev/null || exit 1
[ "$(wc -c < "$next")" -eq "$new_bytes" ] || exit 1
[ ! -L "$target" ] || exit 4
cmp -s -- "$orig" "$target" || { printf '%s\n' 'The file changed since it was read.'; exit 5; }
mv -fT -- "$next" "$target"
"#;

fn confined_edit_command(path: &str, roots: &[String]) -> GuestExec {
    GuestExec::command(
        "sh",
        vec![
            "-c".to_owned(),
            CONFINED_EDIT_SCRIPT.to_owned(),
            "project-edit".to_owned(),
            encode_roots(roots),
            path.to_owned(),
        ],
    )
}

fn encode_roots(roots: &[String]) -> String {
    roots.join(":")
}

async fn capture(
    context: &AgentToolContext<'_>,
    call_id: &str,
    request: GuestExec,
) -> Result<CommandResult, CommandFailure> {
    if context.location == ToolLocation::Host {
        let output = capture_host_file(context, request, MAXIMUM_TOOL_BYTES).await?;
        let mut capture = crate::execution::command::CommandCapture::with_secret(context.secret);
        capture.push(crate::execution::CommandStream::Stdout, &output.stdout);
        capture.push(crate::execution::CommandStream::Stderr, &output.stderr);
        return Ok(capture.into_result(
            output
                .status
                .code()
                .map_or(CommandTermination::Unknown, CommandTermination::Exited),
        ));
    }
    let Some(sandbox) = context.sandbox else {
        return Err(command_not_dispatched("That tool is not available."));
    };
    let visible_call_id = redact(call_id, context.secret);
    // File tools consume raw capture before their own page bounds, not a command preview.
    crate::execution::command::capture_sandbox_command(
        sandbox,
        request,
        context.job,
        context.secret,
        SANDBOX_COMMAND_TIMEOUT,
        &visible_call_id,
        None,
    )
    .await
}

async fn capture_stdout_bytes(
    context: &AgentToolContext<'_>,
    call_id: &str,
    request: GuestExec,
) -> Result<Vec<u8>, ToolFailure> {
    if context.location == ToolLocation::Host {
        let output = capture_host_file(context, request, read::MAXIMUM_SCAN_BYTES + 1)
            .await
            .map_err(|failure| ToolFailure::Command {
                label: "File read".to_owned(),
                failure,
            })?;
        return file_stdout(output.status.code(), output.stdout);
    }
    let Some(sandbox) = context.sandbox else {
        return Err(ToolFailure::Ordinary("That tool is not available."));
    };
    let _ = call_id;
    let mut session = match sandbox.exec_cmd(request).await {
        Ok(session) => session,
        Err(error) => {
            return Err(ToolFailure::Ordinary(error.message()));
        }
    };
    let deadline = tokio::time::Instant::now() + SANDBOX_COMMAND_TIMEOUT;
    let mut stdout = Vec::new();
    let mut exit = None;
    loop {
        let event = tokio::select! {
            biased;
            _ = context.job.cancelled() => {
                session.kill().await;
                session.close().await;
                return Err(ToolFailure::Cancellation);
            }
            _ = tokio::time::sleep_until(deadline) => {
                session.kill().await;
                session.close().await;
                return Err(ToolFailure::Ordinary("The command exceeded the time limit."));
            }
            event = session.recv() => event,
        };
        let Some(event) = event else {
            break;
        };
        match event {
            crate::sandbox::CommandEvent::Output {
                stream: crate::execution::CommandStream::Stdout,
                bytes,
            } => {
                let cap = read::MAXIMUM_SCAN_BYTES
                    .max(MAXIMUM_WRITE_BYTES)
                    .saturating_add(1);
                if stdout.len().saturating_add(bytes.len()) > cap {
                    session.kill().await;
                    session.close().await;
                    return Err(ToolFailure::Ordinary("That read exceeded the size limit."));
                }
                stdout.extend_from_slice(&bytes);
            }
            crate::sandbox::CommandEvent::Output { .. } => {}
            crate::sandbox::CommandEvent::Exited(code) => {
                exit = Some(code);
                break;
            }
            crate::sandbox::CommandEvent::Failed => {
                session.kill().await;
                session.close().await;
                return Err(ToolFailure::Ordinary(
                    "Frinkworks could not run the command. Try again.",
                ));
            }
        }
    }
    if exit.is_none() {
        session.kill().await;
    }
    session.close().await;
    file_stdout(exit, stdout)
}

async fn capture_host_file(
    context: &AgentToolContext<'_>,
    request: GuestExec,
    maximum: usize,
) -> Result<std::process::Output, CommandFailure> {
    let host = context
        .host
        .as_ref()
        .ok_or_else(|| command_not_dispatched("Host execution is not available."))?;
    validate_host_dispatch(host, context.job).map_err(command_not_dispatched)?;
    crate::execution::capture_host_file(
        request.in_dir(host.directory.to_string_lossy()),
        Some(context.job),
        crate::execution::COMMAND_TIMEOUT,
        maximum,
    )
    .await
}

fn file_stdout(exit: Option<i32>, stdout: Vec<u8>) -> Result<Vec<u8>, ToolFailure> {
    match exit {
        Some(0) => Ok(stdout),
        Some(2) => Err(ToolFailure::Ordinary("That path is a directory.")),
        Some(3) => Err(ToolFailure::Ordinary("That path is not a file.")),
        Some(4) => Err(ToolFailure::Authority("Stay inside a granted directory.")),
        Some(5) => Err(ToolFailure::Ordinary("The file changed since it was read.")),
        Some(1) => Err(ToolFailure::Ordinary("That path does not exist.")),
        Some(_) => Err(ToolFailure::Ordinary("The file tool command failed.")),
        None => Err(ToolFailure::Ordinary(
            "Frinkworks could not run the command. Try again.",
        )),
    }
}

fn command_not_dispatched(message: &'static str) -> CommandFailure {
    CommandFailure::new(
        CommandResult::new(Vec::new(), CommandTermination::NotDispatched),
        message,
    )
}

#[cfg(test)]
const TRUNCATED_OUTPUT: &str = "\n[output truncated]";

#[cfg(test)]
pub(super) fn mark_truncated(output: &mut String) {
    let maximum = MAXIMUM_TOOL_BYTES.saturating_sub(TRUNCATED_OUTPUT.len());
    let mut end = output.len().min(maximum);
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    output.truncate(end);
    output.push_str(TRUNCATED_OUTPUT);
}

pub(crate) fn redact(text: &str, secret: Option<&str>) -> String {
    let Some(secret) = secret.filter(|value| !value.is_empty()) else {
        return text.to_owned();
    };
    text.replace(secret, "[redacted]")
}

#[cfg(test)]
mod tests;
