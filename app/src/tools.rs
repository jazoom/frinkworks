use rig_core::completion::ToolDefinition;
use serde::Deserialize;
use std::time::Duration;

use crate::agents::{DirectoryPolicy, ToolId};
use crate::execution::{
    CommandFailure, CommandResult, CommandTermination, OutputScope, ToolLocation,
};
use crate::sandbox::{GuestExec, GuestSandbox};
use crate::sessions::Job;

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
            (Self::List, _) => "List files in a granted directory.",
            (Self::Read, _) => "Read a file inside a granted directory.",
            (Self::Write, _) => {
                "Write a file inside a writable granted directory. Creates parent directories."
            }
            (Self::Run, ToolLocation::Host) => {
                "Run a shell command on this computer after the user approves the exact command. Starts in the selected work location. Approval does not inspect script internals."
            }
            (Self::Run, ToolLocation::Sandbox) => {
                "Run a shell command. Starts in the primary directory."
            }
        }
    }

    fn parameters(self, location: ToolLocation) -> serde_json::Value {
        match self {
            Self::List | Self::Read => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path inside a granted guest directory. Defaults to the first authorised directory or /workspace."
                    }
                },
                "additionalProperties": false
            }),
            Self::Write => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path inside a writable granted guest directory."
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
                        "description": "Why this command is needed. Shown to the user before host approval."
                    }
                },
                "required": if location == ToolLocation::Host { vec!["command", "explanation"] } else { vec!["command"] },
                "additionalProperties": false
            }),
        }
    }
}

pub(crate) fn advertised(selected: &[ToolId], location: ToolLocation) -> Vec<ToolId> {
    match location {
        ToolLocation::Sandbox => selected.to_vec(),
        ToolLocation::Host => selected
            .iter()
            .copied()
            .filter(|tool| *tool == ToolId::Run)
            .collect(),
    }
}

pub(crate) fn definitions_for(selected: &[ToolId], location: ToolLocation) -> Vec<ToolDefinition> {
    advertised(selected, location)
        .into_iter()
        .map(|kind| ToolDefinition {
            name: kind.as_str().to_owned(),
            description: kind.description_for(location).to_owned(),
            parameters: kind.parameters(location),
        })
        .collect()
}

pub(crate) const READ_OUTPUT: &str = "read_output";

pub(crate) fn read_output_definition() -> ToolDefinition {
    ToolDefinition {
        name: READ_OUTPUT.to_owned(),
        description: "Read a later page of a retained command result. Use the reference shown with a command result. The cursor continues from the previous page.".to_owned(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "reference": {
                    "type": "string",
                    "description": "The reference shown with a retained command result."
                },
                "cursor": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Byte position from the previous page. Omit for the first page."
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
}

pub(crate) struct ToolTrace {
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
    label: String,
    output: String,
    command: Option<crate::execution::CommandResult>,
}

impl ToolRun {
    fn plain(label: String, output: String) -> Self {
        Self {
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
    let Some(kind) = authorised_tool(context.tools, name, context.location) else {
        return ToolTrace::fail(
            name.to_owned(),
            "That tool is not available.".to_owned(),
            ToolFailureKind::Authority,
            None,
        );
    };
    match dispatch(context, call_id, kind, arguments).await {
        Ok(run) => ToolTrace::success(run.label, run.output, run.command),
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
        Err(ToolFailure::Command { label, failure }) => ToolTrace::fail(
            label,
            failure.report(),
            ToolFailureKind::from_termination(failure.result.termination),
            Some(failure.result),
        ),
    }
}

fn authorised_tool(selected: &[ToolId], name: &str, location: ToolLocation) -> Option<ToolId> {
    ToolId::parse(name).filter(|kind| advertised(selected, location).contains(kind))
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
    let cursor = match arguments.get("cursor") {
        None | Some(serde_json::Value::Null) => 0,
        Some(value) => match value.as_u64() {
            Some(value) => value as usize,
            None => {
                return plain_failure(
                    READ_OUTPUT,
                    "That output cursor is not valid.",
                    ToolFailureKind::Ordinary,
                );
            }
        },
    };
    match outputs.page(reference, &scope, cursor) {
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
                    "More output remains. Read again with cursor={next}."
                ));
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
    if context.location == ToolLocation::Host && kind != ToolId::Run {
        return Err(ToolFailure::Authority(
            "That tool is not available on this computer.",
        ));
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
        ToolId::Read => {
            let args: PathArgs = parse_args(arguments).map_err(ToolFailure::Ordinary)?;
            let (path, _) = context
                .policy
                .resolve(&args.path)
                .map_err(ToolFailure::Authority)?;
            if path == context.policy.primary_guest()
                || context
                    .policy
                    .grants()
                    .iter()
                    .any(|grant| grant.guest_path == path)
            {
                return Err(ToolFailure::Ordinary("Choose a file to read."));
            }
            let maximum = MAXIMUM_TOOL_BYTES.to_string();
            let output = plain_capture(
                format!("read `{path}`"),
                capture(
                    context,
                    call_id,
                    confined_existing_command(
                        &path,
                        &context.policy.guest_roots(),
                        "head",
                        &["-c", &maximum],
                    ),
                )
                .await,
            )?;
            Ok(ToolRun::plain(format!("read `{path}`"), output))
        }
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
            let label = format!("run `{command}`");
            match capture(
                context,
                call_id,
                GuestExec::shell(command).in_dir(context.policy.primary_guest()),
            )
            .await
            {
                Ok(result) => {
                    let output = result.report();
                    Ok(ToolRun {
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
    if host.settings.automatic_host_commands() {
        request.token = crate::execution::command_token()
            .map_err(|error| ToolFailure::Ordinary(error.message()))?;
        return dispatch_host_command(host, context.job, call_id, label, &request).await;
    }
    let approvals = &host.state.host_approvals;
    let token = approvals
        .submit(request.clone())
        .map_err(|error| ToolFailure::Authority(error.message()))?;
    request.token = token.clone();
    if let Err(error) = record_host_evidence(host, &request, "awaiting_approval", "", None) {
        approvals.invalidate_job(context.job.id());
        return Err(ToolFailure::Persistence(error));
    }
    if context.job.set_awaiting_decision().is_none() && context.job.cancel_requested() {
        approvals.invalidate_job(context.job.id());
        return Err(ToolFailure::Cancellation);
    }
    let decision = approvals.wait(&token, context.job).await;
    let _ = context.job.resume();
    match decision {
        Ok(crate::execution::HostCommandDecision::Approved) => {
            dispatch_host_command(host, context.job, call_id, label, &request).await
        }
        Ok(crate::execution::HostCommandDecision::Rejected) => {
            record_host_evidence(
                host,
                &request,
                "rejected",
                "The user rejected this command.",
                None,
            )
            .map_err(ToolFailure::Persistence)?;
            Err(ToolFailure::Rejected { label })
        }
        Err(error) => {
            record_host_evidence(host, &request, "invalidated", error.message(), None)
                .map_err(ToolFailure::Persistence)?;
            if context.job.cancel_requested() {
                Err(ToolFailure::Cancellation)
            } else {
                Err(ToolFailure::Authority(error.message()))
            }
        }
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
    let scope = host_output_scope(host, request);
    match result {
        Ok(command) => {
            let command = command.redacted(host.secret);
            let command = retain_command(host.state, scope, job.id(), call_id, command);
            let output = command.report();
            if record_host_evidence(host, request, "finished", &output, Some(&command)).is_err() {
                let mut command = command;
                command.termination = CommandTermination::StorageFailure;
                return Err(ToolFailure::Command {
                    label,
                    failure: crate::execution::CommandFailure::new(
                        command,
                        "Power Plant could not store command output. Command effects can remain incomplete.",
                    ),
                });
            }
            Ok(ToolRun {
                label,
                output,
                command: Some(command),
            })
        }
        Err(mut failure) => {
            failure.result = failure.result.redacted(host.secret);
            failure.result = retain_command(host.state, scope, job.id(), call_id, failure.result);
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

pub(crate) fn retain_command(
    state: &crate::state::AppState,
    scope: OutputScope,
    job: crate::sessions::JobId,
    tool_call: &str,
    command: CommandResult,
) -> CommandResult {
    if command.chunks.is_empty() || command.retained.is_some() {
        return command;
    }
    let key = crate::execution::OutputKey {
        scope,
        job,
        tool_call: tool_call.to_owned(),
    };
    match state.outputs.store(&key, &command) {
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
        || !host.state.access_consent.authorised_host_conversation(
            host.session,
            host.conversation,
            host.settings,
        )
    {
        return Err("Host access expired or changed. Approve the current settings again.");
    }
    for grant in &host.settings.directories {
        grant
            .revalidate()
            .map_err(|_| "A work location changed or is not available.")?;
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
                    || attempt.sandbox.kind
                        != crate::workflows::run::AttemptSandboxKind::HostExecution
            })
    {
        return Err("That host command is not bound to the active run, step and attempt.");
    }
    let settings = run
        .phase_settings(&step)
        .cloned()
        .or_else(|| run.directory_settings())
        .ok_or("The pinned host settings are unavailable.")?;
    if settings != *host.settings || !settings.host_tools() {
        return Err("Host access expired or changed. Approve the current settings again.");
    }
    let launch_ok = host.state.access_consent.authorised_launch(
        run_id,
        host.session,
        host.conversation,
        host.settings,
    );
    let ordinary_consent = run.kind == crate::workflows::RunKind::QuickTask
        && record
            .model
            .as_ref()
            .is_some_and(|model| model.settings == settings)
        && host.state.access_consent.authorised_host_conversation(
            host.session,
            host.conversation,
            &settings,
        );
    if !launch_ok && !ordinary_consent {
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

const CONFINED_EXISTING_SCRIPT: &str = r#"
roots=$1
resolved=$(realpath "$2") || { printf '%s\n' 'That path does not exist.'; exit 1; }
ok=0
oldifs=$IFS
IFS=:
for root in $roots; do
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

fn encode_roots(roots: &[String]) -> String {
    roots.join(":")
}

async fn capture(
    context: &AgentToolContext<'_>,
    call_id: &str,
    request: GuestExec,
) -> Result<CommandResult, CommandFailure> {
    let Some(sandbox) = context.sandbox else {
        return Err(command_not_dispatched("That tool is not available."));
    };
    let visible_call_id = redact(call_id, context.secret);
    let mut capture = match (context.outputs, &context.output_scope) {
        (Some(store), Some(scope)) => crate::execution::command::CommandCapture::with_output(
            context.secret,
            store,
            &crate::execution::OutputKey {
                scope: scope.clone(),
                job: context.job.id(),
                tool_call: visible_call_id.clone(),
            },
        )
        .map_err(|error| command_not_dispatched(error.message()))?,
        _ => crate::execution::command::CommandCapture::with_secret(context.secret),
    };
    let mut session = match sandbox.exec_cmd(request).await {
        Ok(session) => session,
        Err(error) => return Err(command_not_dispatched(error.message())),
    };
    let deadline = tokio::time::Instant::now() + SANDBOX_COMMAND_TIMEOUT;
    let mut progress = crate::execution::command::CommandProgress::new();
    let mut progress_tick = tokio::time::interval(Duration::from_millis(100));
    progress_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut exit = None;
    loop {
        let event = tokio::select! {
            biased;
            _ = context.job.cancelled() => {
                session.kill().await;
                session.close().await;
                return Err(command_failure(capture, CommandTermination::Cancelled, "Stopped."));
            }
            _ = tokio::time::sleep_until(deadline) => {
                session.kill().await;
                session.close().await;
                return Err(command_failure(
                    capture,
                    CommandTermination::TimedOut,
                    "The command exceeded the time limit.",
                ));
            }
            _ = progress_tick.tick() => {
                progress.publish(context.job, &visible_call_id);
                continue;
            }
            event = session.recv() => event,
        };
        let Some(event) = event else {
            break;
        };
        match event {
            crate::sandbox::CommandEvent::Output { stream, bytes } => {
                let push = capture.push(stream, &bytes);
                for chunk in push.safe {
                    progress.push(chunk.stream, chunk.text);
                }
                if push.overflow {
                    session.kill().await;
                    session.close().await;
                    return Err(command_failure(
                        capture,
                        CommandTermination::ResourceLimit,
                        "The command exceeded the output resource limit.",
                    ));
                }
            }
            crate::sandbox::CommandEvent::Exited(code) => {
                exit = Some(code);
                break;
            }
            crate::sandbox::CommandEvent::Failed => {
                session.kill().await;
                session.close().await;
                return Err(command_failure(
                    capture,
                    CommandTermination::Unknown,
                    "Power Plant could not run the command. Try again.",
                ));
            }
        }
    }
    if exit.is_none() {
        session.kill().await;
    }
    session.close().await;
    let termination = match exit {
        Some(code) => CommandTermination::Exited(code),
        None => CommandTermination::Unknown,
    };
    Ok(capture.into_result(termination))
}

fn command_not_dispatched(message: &'static str) -> CommandFailure {
    CommandFailure::new(
        CommandResult::new(Vec::new(), CommandTermination::NotDispatched),
        message,
    )
}

fn command_failure(
    capture: crate::execution::command::CommandCapture,
    termination: CommandTermination,
    message: &'static str,
) -> CommandFailure {
    CommandFailure::new(capture.into_result(termination), message)
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
