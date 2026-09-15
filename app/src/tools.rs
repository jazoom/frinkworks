use rig_core::completion::ToolDefinition;
use serde::Deserialize;

use crate::agents::{DirectoryPolicy, ToolId};
use crate::execution::ToolLocation;
use crate::sandbox::{GuestExec, GuestSandbox};
use crate::sessions::Job;

pub(crate) const MAXIMUM_TOOL_BYTES: usize = 64 * 1024;
pub(crate) const MAXIMUM_WRITE_BYTES: usize = 256 * 1024;
pub(crate) const MAXIMUM_COMMAND_BYTES: usize = 32_768;

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
    pub(crate) output_drafts:
        Option<&'a std::sync::Mutex<crate::workflows::artefacts::output::OutputDrafts>>,
    pub(crate) required_outputs: &'a [crate::workflows::definition::RequiredOutput],
}

pub(crate) struct ToolTrace {
    pub(crate) label: String,
    pub(crate) output: String,
    pub(crate) failed: bool,
}

pub(crate) async fn invoke(
    context: &AgentToolContext<'_>,
    name: &str,
    arguments: &serde_json::Value,
) -> ToolTrace {
    if context.job.cancel_requested() {
        return ToolTrace {
            label: name.to_owned(),
            output: "Stopped.".to_owned(),
            failed: true,
        };
    }
    if name == SUBMIT_WORKFLOW_OUTPUT {
        return submit_output(context, arguments);
    }
    let Some(kind) = authorised_tool(context.tools, name, context.location) else {
        return ToolTrace {
            label: name.to_owned(),
            output: "That tool is not available.".to_owned(),
            failed: true,
        };
    };
    match dispatch(context, kind, arguments).await {
        Ok((label, output)) => ToolTrace {
            label,
            output,
            failed: false,
        },
        Err(message) => ToolTrace {
            label: kind.as_str().to_owned(),
            output: message.to_owned(),
            failed: true,
        },
    }
}

fn authorised_tool(selected: &[ToolId], name: &str, location: ToolLocation) -> Option<ToolId> {
    ToolId::parse(name).filter(|kind| advertised(selected, location).contains(kind))
}

fn submit_output(context: &AgentToolContext<'_>, arguments: &serde_json::Value) -> ToolTrace {
    let Some(drafts) = context.output_drafts else {
        return ToolTrace {
            label: SUBMIT_WORKFLOW_OUTPUT.to_owned(),
            output: "That tool is not available.".to_owned(),
            failed: true,
        };
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
        return ToolTrace {
            label: SUBMIT_WORKFLOW_OUTPUT.to_owned(),
            output: crate::workflows::artefacts::output::OutputDraftError::Kind
                .message()
                .to_owned(),
            failed: true,
        };
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
        Ok(()) => ToolTrace {
            label: format!("submit `{key}`"),
            output: "Stored.".to_owned(),
            failed: false,
        },
        Err(error) => ToolTrace {
            label: SUBMIT_WORKFLOW_OUTPUT.to_owned(),
            output: error.message().to_owned(),
            failed: true,
        },
    }
}

async fn dispatch(
    context: &AgentToolContext<'_>,
    kind: ToolId,
    arguments: &serde_json::Value,
) -> Result<(String, String), &'static str> {
    if context.location == ToolLocation::Host && kind != ToolId::Run {
        return Err("That tool is not available on this computer.");
    }
    match kind {
        ToolId::List => {
            let args: PathArgs = parse_args(arguments)?;
            let (path, _) = context.policy.resolve(&args.path)?;
            let output = capture(
                context,
                confined_existing_command(&path, &context.policy.guest_roots(), "ls", &["-la"]),
            )
            .await?;
            Ok((format!("list `{path}`"), output))
        }
        ToolId::Read => {
            let args: PathArgs = parse_args(arguments)?;
            let (path, _) = context.policy.resolve(&args.path)?;
            if path == context.policy.primary_guest()
                || context
                    .policy
                    .grants()
                    .iter()
                    .any(|grant| grant.guest_path == path)
            {
                return Err("Choose a file to read.");
            }
            let maximum = MAXIMUM_TOOL_BYTES.to_string();
            let output = capture(
                context,
                confined_existing_command(
                    &path,
                    &context.policy.guest_roots(),
                    "head",
                    &["-c", &maximum],
                ),
            )
            .await?;
            Ok((format!("read `{path}`"), output))
        }
        ToolId::Write => {
            let args: WriteArgs = parse_args(arguments)?;
            let (path, access) = context.policy.resolve(&args.path)?;
            if !access.is_writable() {
                return Err("That path is read-only.");
            }
            if path == context.policy.primary_guest()
                || context
                    .policy
                    .grants()
                    .iter()
                    .any(|grant| grant.guest_path == path)
            {
                return Err("Choose a file to write.");
            }
            if args.contents.len() > MAXIMUM_WRITE_BYTES {
                return Err("That file is too large to write.");
            }
            let output = capture(
                context,
                confined_write_command(&path, &context.policy.writable_roots())
                    .with_stdin(args.contents.into_bytes()),
            )
            .await?;
            let body = if output.trim().is_empty() {
                "Wrote the file.".to_owned()
            } else {
                output
            };
            Ok((format!("write `{path}`"), body))
        }
        ToolId::Run => {
            let args: RunArgs = parse_args(arguments)?;
            let command = args.command.trim();
            if command.is_empty() {
                return Err("Enter a command.");
            }
            if command.len() > MAXIMUM_COMMAND_BYTES {
                return Err("That command is too long.");
            }
            if context.location == ToolLocation::Host {
                if args.explanation.trim().is_empty() {
                    return Err("Explain why this command is necessary.");
                }
                return host_run(context, command, args.explanation.trim()).await;
            }
            let output = capture(
                context,
                GuestExec::shell(command).in_dir(context.policy.primary_guest()),
            )
            .await?;
            Ok((format!("run `{command}`"), output))
        }
    }
}

async fn host_run(
    context: &AgentToolContext<'_>,
    command: &str,
    explanation: &str,
) -> Result<(String, String), &'static str> {
    let host = context
        .host
        .as_ref()
        .ok_or("Host execution is not available.")?;
    validate_host_dispatch(host, context.job)?;
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
        request.token = crate::execution::command_token().map_err(|error| error.message())?;
        return dispatch_host_command(host, context.job, command, &request).await;
    }
    let approvals = &host.state.host_approvals;
    let token = approvals
        .submit(request.clone())
        .map_err(|error| error.message())?;
    request.token = token.clone();
    if let Err(error) = record_host_evidence(host, &request, "awaiting_approval", "") {
        approvals.invalidate_job(context.job.id());
        return Err(error);
    }
    if context.job.set_awaiting_decision().is_none() && context.job.cancel_requested() {
        approvals.invalidate_job(context.job.id());
        return Err("Stopped.");
    }
    let decision = approvals.wait(&token, context.job).await;
    let _ = context.job.resume();
    match decision {
        Ok(crate::execution::HostCommandDecision::Approved) => {
            dispatch_host_command(host, context.job, command, &request).await
        }
        Ok(crate::execution::HostCommandDecision::Rejected) => {
            record_host_evidence(
                host,
                &request,
                "rejected",
                "The user rejected this command.",
            )?;
            Ok((
                format!("run `{command}`"),
                "The user rejected this command.".to_owned(),
            ))
        }
        Err(error) => {
            record_host_evidence(host, &request, "invalidated", error.message())?;
            Err(error.message())
        }
    }
}

fn record_host_evidence(
    host: &HostToolContext<'_>,
    request: &crate::execution::HostCommandRequest,
    status: &str,
    output: &str,
) -> Result<(), &'static str> {
    host.state
        .workflow_evidence
        .host_command(request, status, output, host.secret)
        .map_err(|error| error.message())
}

async fn dispatch_host_command(
    host: &HostToolContext<'_>,
    job: &Job,
    command: &str,
    request: &crate::execution::HostCommandRequest,
) -> Result<(String, String), &'static str> {
    validate_host_dispatch(host, job)?;
    record_host_evidence(host, request, "dispatching", "")?;
    let result = if host.run.is_some() {
        crate::execution::run_workflow_shell(
            command,
            &host.directory,
            job,
            crate::execution::COMMAND_TIMEOUT,
        )
        .await
    } else {
        crate::execution::run_shell(
            command,
            &host.directory,
            job,
            crate::execution::COMMAND_TIMEOUT,
        )
        .await
    };
    let output = result.as_deref().unwrap_or_else(|error| error);
    record_host_evidence(
        host,
        request,
        if result.is_ok() { "finished" } else { "failed" },
        output,
    )?;
    Ok((format!("run `{command}`"), result?))
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
    request: GuestExec,
) -> Result<String, &'static str> {
    let sandbox = context.sandbox.ok_or("That tool is not available.")?;
    let mut session = sandbox
        .exec_cmd(request)
        .await
        .map_err(|error| error.message())?;
    let mut output = String::new();
    let mut exit = None;
    loop {
        let event = tokio::select! {
            biased;
            _ = context.job.cancelled() => {
                session.kill().await;
                session.close().await;
                return Err("Stopped.");
            }
            event = session.recv() => event,
        };
        let Some(event) = event else {
            break;
        };
        match event {
            crate::sandbox::CommandEvent::Output(text) => {
                if append_bounded(&mut output, &text) {
                    session.kill().await;
                    session.close().await;
                    mark_truncated(&mut output);
                    return Ok(output);
                }
            }
            crate::sandbox::CommandEvent::Exited(code) => {
                exit = Some(code);
                break;
            }
            crate::sandbox::CommandEvent::Failed => {
                session.close().await;
                return Err("Power Plant could not run the command. Try again.");
            }
        }
    }
    session.close().await;
    match exit {
        Some(0) => Ok(empty_output(output)),
        None => Err("Power Plant lost the command result. Try again."),
        Some(code) => {
            let mut failed = output;
            if !failed.is_empty() && !failed.ends_with('\n') {
                failed.push('\n');
            }
            failed.push_str(&format!("The command exited with code {code}."));
            Ok(failed)
        }
    }
}

const TRUNCATED_OUTPUT: &str = "\n[output truncated]";

pub(super) fn mark_truncated(output: &mut String) {
    let maximum = MAXIMUM_TOOL_BYTES.saturating_sub(TRUNCATED_OUTPUT.len());
    let mut end = output.len().min(maximum);
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    output.truncate(end);
    output.push_str(TRUNCATED_OUTPUT);
}

fn empty_output(output: String) -> String {
    if output.is_empty() {
        "(no output)".to_owned()
    } else {
        output
    }
}

fn append_bounded(buffer: &mut String, piece: &str) -> bool {
    let remaining = MAXIMUM_TOOL_BYTES.saturating_sub(buffer.len());
    if piece.len() <= remaining {
        buffer.push_str(piece);
        return false;
    }
    let mut end = remaining;
    while end > 0 && !piece.is_char_boundary(end) {
        end -= 1;
    }
    buffer.push_str(&piece[..end]);
    true
}

pub(crate) fn redact(text: &str, secret: Option<&str>) -> String {
    let Some(secret) = secret.filter(|value| !value.is_empty()) else {
        return text.to_owned();
    };
    text.replace(secret, "[redacted]")
}

#[cfg(test)]
mod tests;
