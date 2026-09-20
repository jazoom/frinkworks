use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::providers::{AssistantReply, ToolOutput};

use super::{AttemptId, RunId};

pub(crate) const EVIDENCE_RECORD_VERSION: u32 = 1;
pub(crate) const MAXIMUM_ACTIVITY_EVENTS: usize = 256;
// Escaped evidence must also fit one Hypergraft navigation envelope.
pub(crate) const MAXIMUM_ACTIVITY_BYTES: usize = 64 * 1024;
pub(crate) const MAXIMUM_ACTIVITY_TEXT_BYTES: usize = 16 * 1024;
pub(crate) const MAXIMUM_HISTORY_TURNS: usize = 128;
pub(crate) const MAXIMUM_TERMINAL_TEXT_BYTES: usize = 16 * 1024;
pub(crate) const MAXIMUM_TERMINAL_TOOLS: usize = 128;
pub(crate) const MAXIMUM_TERMINAL_TOOL_BYTES: usize = 64 * 1024;
pub(crate) const MAXIMUM_TERMINAL_CONTENT_BYTES: usize = 32 * 1024;
pub(crate) const MAXIMUM_EVIDENCE_RECORD_BYTES: usize = 512 * 1024;
pub(crate) const MAXIMUM_EVIDENCE_RECORDS: usize = 4096;

const TRUNCATION_MARKER: &str = "\n[evidence truncated]";

#[derive(Clone)]
pub(crate) struct AttemptEvidenceContext {
    store: Arc<WorkflowEvidenceStore>,
    run_id: RunId,
    attempt_id: AttemptId,
    phase: String,
}

impl AttemptEvidenceContext {
    pub(crate) fn new(
        store: Arc<WorkflowEvidenceStore>,
        run_id: RunId,
        attempt_id: AttemptId,
        phase: impl Into<String>,
    ) -> Self {
        Self {
            store,
            run_id,
            attempt_id,
            phase: phase.into(),
        }
    }

    pub(crate) fn response(&self, text: &str, secret: Option<&str>) {
        let _ = self.store.append_activity(
            self.run_id,
            self.attempt_id,
            ActivityInput {
                phase: &self.phase,
                kind: ActivityKind::Response,
                text,
                label: "",
                provider: None,
                input_tokens: None,
                secret,
            },
        );
    }

    pub(crate) fn thinking(&self, text: &str, secret: Option<&str>) {
        let _ = self.store.append_activity(
            self.run_id,
            self.attempt_id,
            ActivityInput {
                phase: &self.phase,
                kind: ActivityKind::Thinking,
                text,
                label: "",
                provider: None,
                input_tokens: None,
                secret,
            },
        );
    }

    pub(crate) fn tool(&self, tool: &ToolOutput, secret: Option<&str>) {
        let _ = self.store.append_activity(
            self.run_id,
            self.attempt_id,
            ActivityInput {
                phase: &self.phase,
                kind: ActivityKind::Tool,
                text: &tool.output,
                label: &tool.label,
                provider: None,
                input_tokens: None,
                secret,
            },
        );
    }

    pub(crate) fn usage(&self, usage: &crate::providers::ModelUsage) {
        let _ = self.store.append_activity(
            self.run_id,
            self.attempt_id,
            ActivityInput {
                phase: &self.phase,
                kind: ActivityKind::Usage,
                text: "",
                label: "",
                provider: Some(usage.provider.as_str()),
                input_tokens: Some(usage.input_tokens),
                secret: None,
            },
        );
    }

    /// The caller must exclude credentials before durable publication.
    pub(crate) fn turn(&self, turn: &crate::providers::ChatTurn) -> Result<(), EvidenceError> {
        self.store
            .append_history(self.run_id, self.attempt_id, turn, &self.phase)
    }

    pub(crate) fn terminal(
        &self,
        state: TerminalState,
        reply: &AssistantReply,
        error: Option<&str>,
        secret: Option<&str>,
    ) {
        let _ = self.store.record_terminal(
            self.run_id,
            self.attempt_id,
            TerminalInput {
                phase: &self.phase,
                state,
                reply,
                error,
                secret,
            },
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ActivityKind {
    Response,
    Thinking,
    Tool,
    Usage,
}

pub(crate) struct ActivityInput<'a> {
    pub(crate) phase: &'a str,
    pub(crate) kind: ActivityKind,
    pub(crate) text: &'a str,
    pub(crate) label: &'a str,
    pub(crate) provider: Option<&'a str>,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) secret: Option<&'a str>,
}

pub(crate) struct TerminalInput<'a> {
    pub(crate) phase: &'a str,
    pub(crate) state: TerminalState,
    pub(crate) reply: &'a AssistantReply,
    pub(crate) error: Option<&'a str>,
    pub(crate) secret: Option<&'a str>,
}

impl ActivityKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Response => "Response",
            Self::Thinking => "Thinking",
            Self::Tool => "Tool output",
            Self::Usage => "Usage",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ActivityEvent {
    pub(crate) sequence: u64,
    pub(crate) phase: String,
    pub(crate) kind: ActivityKind,
    pub(crate) text: String,
    pub(crate) label: String,
    pub(crate) provider: Option<String>,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TerminalState {
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl TerminalState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Interrupted => "Interrupted",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct EvidenceTool {
    pub(crate) label: String,
    pub(crate) output: String,
    pub(crate) truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<crate::execution::CommandResult>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct TerminalResponse {
    pub(crate) phase: String,
    pub(crate) state: TerminalState,
    pub(crate) text: String,
    pub(crate) thinking: String,
    pub(crate) tools: Vec<EvidenceTool>,
    pub(crate) error: Option<String>,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct AttemptEvidence {
    pub(crate) version: u32,
    pub(crate) run_id: String,
    pub(crate) attempt_id: String,
    pub(crate) phase: String,
    pub(crate) events: Vec<ActivityEvent>,
    pub(crate) activity_bytes: usize,
    pub(crate) activity_truncated: bool,
    pub(crate) terminal: Option<TerminalResponse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) history: Vec<crate::providers::ChatTurn>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EvidenceError {
    Persist,
    Corrupt,
    Conflict,
    Full,
}

impl EvidenceError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Persist => "Power Plant could not store workflow evidence.",
            Self::Corrupt => "Workflow evidence is unreadable.",
            Self::Conflict => "Workflow evidence already has a different terminal result.",
            Self::Full => {
                "This workflow attempt produced more evidence than Power Plant can retain."
            }
        }
    }
}

pub(crate) struct WorkflowEvidenceStore {
    dir: Option<PathBuf>,
    inner: Mutex<BTreeMap<(RunId, AttemptId), AttemptEvidence>>,
}

impl WorkflowEvidenceStore {
    pub(crate) fn open(dir: PathBuf) -> Result<Self, EvidenceError> {
        crate::storage::ensure_private_dir(&dir).map_err(|_| EvidenceError::Persist)?;
        let records = load_dir(&dir)?;
        Ok(Self {
            dir: Some(dir),
            inner: Mutex::new(records),
        })
    }

    /// This record describes command evidence, never permission to replay a command.
    pub(crate) fn host_command(
        &self,
        request: &crate::execution::HostCommandRequest,
        status: &str,
        output: &str,
        secret: Option<&str>,
        command: Option<&crate::execution::CommandResult>,
    ) -> Result<(), EvidenceError> {
        if request.token.len() != 64 || !request.token.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(EvidenceError::Corrupt);
        }
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        let _guard = self.lock();
        let dir = dir.join("host");
        crate::storage::ensure_private_dir(&dir).map_err(|_| EvidenceError::Persist)?;
        let path = dir.join(format!("{}.json", request.token));
        if !path.exists()
            && fs::read_dir(&dir)
                .map_err(|_| EvidenceError::Persist)?
                .count()
                >= MAXIMUM_EVIDENCE_RECORDS
        {
            return Err(EvidenceError::Full);
        }
        let text =
            |value: &str, maximum| bounded_text(&crate::tools::redact(value, secret), maximum).0;
        let command = command.map(|command| {
            command
                .redacted(secret)
                .bounded(crate::tools::MAXIMUM_TOOL_BYTES)
                .0
        });
        let record = serde_json::json!({
            "version": 1,
            "conversation": request.conversation.to_string(),
            "job": request.job.to_string(),
            "execution_revision": request.execution_revision,
            "command": text(&request.command, crate::tools::MAXIMUM_COMMAND_BYTES),
            "directory": text(&request.directory.to_string_lossy(), MAXIMUM_ACTIVITY_TEXT_BYTES),
            "explanation": text(&request.explanation, MAXIMUM_ACTIVITY_TEXT_BYTES),
            "run": request.run.clone().unwrap_or_default(),
            "step": request.step.clone().unwrap_or_default(),
            "attempt": request.attempt.clone().unwrap_or_default(),
            "status": status,
            "output": text(output, crate::tools::MAXIMUM_TOOL_BYTES),
            "result": command,
        });
        let bytes = serde_json::to_vec(&record).map_err(|_| EvidenceError::Persist)?;
        crate::storage::write_private(&path, &bytes).map_err(|_| EvidenceError::Persist)
    }

    pub(crate) fn get(&self, run_id: &RunId, attempt_id: &AttemptId) -> Option<AttemptEvidence> {
        self.lock().get(&(*run_id, *attempt_id)).cloned()
    }

    pub(crate) fn append_activity(
        &self,
        run_id: RunId,
        attempt_id: AttemptId,
        input: ActivityInput<'_>,
    ) -> Result<bool, EvidenceError> {
        let ActivityInput {
            phase,
            kind,
            text,
            label,
            provider,
            input_tokens,
            secret,
        } = input;
        let mut records = self.lock();
        let key = (run_id, attempt_id);
        if !records.contains_key(&key) && records.len() >= MAXIMUM_EVIDENCE_RECORDS {
            return Err(EvidenceError::Full);
        }
        let mut next = records
            .get(&key)
            .cloned()
            .unwrap_or_else(|| empty_record(run_id, attempt_id, phase));
        if next.phase != phase {
            return Err(EvidenceError::Conflict);
        }
        let (text, text_truncated) = bounded_text(
            &crate::tools::redact(text, secret),
            MAXIMUM_ACTIVITY_TEXT_BYTES,
        );
        let (label, label_truncated) = bounded_text(
            &crate::tools::redact(label, secret),
            MAXIMUM_ACTIVITY_TEXT_BYTES,
        );
        let (provider, provider_truncated) = provider.map_or((None, false), |value| {
            let (value, truncated) = bounded_text(
                &crate::tools::redact(value, secret),
                MAXIMUM_ACTIVITY_TEXT_BYTES,
            );
            (Some(value), truncated)
        });
        let event_bytes = text
            .len()
            .saturating_add(label.len())
            .saturating_add(provider.as_ref().map_or(0, String::len));
        let truncated = text_truncated || label_truncated || provider_truncated;
        if next.events.len() >= MAXIMUM_ACTIVITY_EVENTS
            || next.activity_bytes.saturating_add(event_bytes) > MAXIMUM_ACTIVITY_BYTES
        {
            if next.activity_truncated {
                return Ok(false);
            }
            next.activity_truncated = true;
            persist_record(self.dir.as_deref(), &mut next)?;
            records.insert(key, next);
            return Ok(false);
        }
        if text.is_empty() && label.is_empty() && provider.is_none() && input_tokens.is_none() {
            return Ok(false);
        }
        let sequence = next.events.last().map_or(1, |event| event.sequence + 1);
        next.events.push(ActivityEvent {
            sequence,
            phase: phase.to_owned(),
            kind,
            text,
            label,
            provider,
            input_tokens,
            truncated,
        });
        next.activity_bytes = next.activity_bytes.saturating_add(event_bytes);
        next.activity_truncated |= truncated;
        persist_record(self.dir.as_deref(), &mut next)?;
        records.insert(key, next);
        Ok(true)
    }

    pub(crate) fn record_terminal(
        &self,
        run_id: RunId,
        attempt_id: AttemptId,
        input: TerminalInput<'_>,
    ) -> Result<(), EvidenceError> {
        let TerminalInput {
            phase,
            state,
            reply,
            error,
            secret,
        } = input;
        let mut records = self.lock();
        let key = (run_id, attempt_id);
        if !records.contains_key(&key) && records.len() >= MAXIMUM_EVIDENCE_RECORDS {
            return Err(EvidenceError::Full);
        }
        let mut next = records
            .get(&key)
            .cloned()
            .unwrap_or_else(|| empty_record(run_id, attempt_id, phase));
        if next.phase != phase {
            return Err(EvidenceError::Conflict);
        }
        let (text, text_truncated) = bounded_text(
            &crate::tools::redact(&reply.text, secret),
            MAXIMUM_TERMINAL_TEXT_BYTES,
        );
        let (thinking, thinking_truncated) = bounded_text(
            &crate::tools::redact(&reply.thinking, secret),
            MAXIMUM_TERMINAL_TEXT_BYTES,
        );
        let mut tools = Vec::new();
        let mut tools_truncated = false;
        let mut tool_bytes = 0usize;
        for tool in reply.tools.iter().take(MAXIMUM_TERMINAL_TOOLS) {
            let remaining = MAXIMUM_TERMINAL_CONTENT_BYTES.saturating_sub(tool_bytes);
            if remaining == 0 {
                tools_truncated = true;
                break;
            }
            let (label, label_truncated) = bounded_text(
                &crate::tools::redact(&tool.label, secret),
                MAXIMUM_ACTIVITY_TEXT_BYTES.min(remaining),
            );
            let output_limit = remaining.saturating_sub(label.len());
            let (output, output_truncated) = bounded_text(
                &crate::tools::redact(&tool.output, secret),
                MAXIMUM_TERMINAL_TOOL_BYTES.min(output_limit),
            );
            let item_truncated = label_truncated || output_truncated;
            let after_output = tool_bytes
                .saturating_add(label.len())
                .saturating_add(output.len());
            let command_budget = MAXIMUM_TERMINAL_CONTENT_BYTES.saturating_sub(after_output);
            let (command, command_truncated) = match tool.command.as_ref() {
                Some(command) => {
                    let (bounded, truncated) = command.redacted(secret).bounded(command_budget);
                    (Some(bounded), truncated)
                }
                None => (None, false),
            };
            let command_bytes = command.as_ref().map_or(0, |command| {
                command
                    .chunks
                    .iter()
                    .map(|chunk| chunk.text.len())
                    .sum::<usize>()
            });
            tool_bytes = after_output.saturating_add(command_bytes);
            let item_truncated = item_truncated || command_truncated;
            tools_truncated |= item_truncated;
            tools.push(EvidenceTool {
                label,
                output,
                truncated: item_truncated,
                command,
            });
        }
        tools_truncated |= reply.tools.len() > MAXIMUM_TERMINAL_TOOLS;
        let (error, error_truncated) = error.map_or((None, false), |value| {
            let (value, truncated) = bounded_text(
                &crate::tools::redact(value, secret),
                MAXIMUM_ACTIVITY_TEXT_BYTES,
            );
            (Some(value), truncated)
        });
        let terminal = TerminalResponse {
            phase: phase.to_owned(),
            state,
            text,
            thinking,
            tools,
            error,
            truncated: text_truncated || thinking_truncated || tools_truncated || error_truncated,
        };
        if next
            .terminal
            .as_ref()
            .is_some_and(|current| current != &terminal)
        {
            return Err(EvidenceError::Conflict);
        }
        next.terminal = Some(terminal);
        persist_record(self.dir.as_deref(), &mut next)?;
        records.insert(key, next);
        Ok(())
    }

    pub(crate) fn append_history(
        &self,
        run_id: RunId,
        attempt_id: AttemptId,
        turn: &crate::providers::ChatTurn,
        phase: &str,
    ) -> Result<(), EvidenceError> {
        let mut records = self.lock();
        let key = (run_id, attempt_id);
        if !records.contains_key(&key) && records.len() >= MAXIMUM_EVIDENCE_RECORDS {
            return Err(EvidenceError::Full);
        }
        let mut next = records
            .get(&key)
            .cloned()
            .unwrap_or_else(|| empty_record(run_id, attempt_id, phase));
        if next.phase != phase {
            return Err(EvidenceError::Conflict);
        }
        next.history.push(turn.clone());
        if next.history.len() > MAXIMUM_HISTORY_TURNS {
            return Err(EvidenceError::Full);
        }
        persist_record(self.dir.as_deref(), &mut next)?;
        records.insert(key, next);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<(RunId, AttemptId), AttemptEvidence>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn empty_record(run_id: RunId, attempt_id: AttemptId, phase: &str) -> AttemptEvidence {
    AttemptEvidence {
        version: EVIDENCE_RECORD_VERSION,
        run_id: run_id.as_hex(),
        attempt_id: attempt_id.as_hex(),
        phase: phase.to_owned(),
        events: Vec::new(),
        activity_bytes: 0,
        activity_truncated: false,
        terminal: None,
        history: Vec::new(),
    }
}

fn bounded_text(text: &str, maximum: usize) -> (String, bool) {
    if text.len() <= maximum {
        return (text.to_owned(), false);
    }
    if maximum <= TRUNCATION_MARKER.len() {
        let mut end = maximum.min(text.len());
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        return (text[..end].to_owned(), true);
    }
    let limit = maximum - TRUNCATION_MARKER.len();
    let mut end = limit.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = text[..end].to_owned();
    bounded.push_str(TRUNCATION_MARKER);
    (bounded, true)
}

fn load_dir(dir: &Path) -> Result<BTreeMap<(RunId, AttemptId), AttemptEvidence>, EvidenceError> {
    let mut records = BTreeMap::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(records),
        Err(_) => return Err(EvidenceError::Persist),
    };
    for entry in entries {
        let entry = entry.map_err(|_| EvidenceError::Persist)?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        if records.len() >= MAXIMUM_EVIDENCE_RECORDS {
            return Err(EvidenceError::Full);
        }
        let bytes = crate::storage::read_private_bounded(&path, MAXIMUM_EVIDENCE_RECORD_BYTES)
            .map_err(|_| EvidenceError::Corrupt)?;
        let record: AttemptEvidence =
            serde_json::from_slice(&bytes).map_err(|_| EvidenceError::Corrupt)?;
        validate_record(&record)?;
        let run_id = RunId::parse(&record.run_id).ok_or(EvidenceError::Corrupt)?;
        let attempt_id = AttemptId::parse(&record.attempt_id).ok_or(EvidenceError::Corrupt)?;
        let expected = evidence_path(dir, run_id, attempt_id)?;
        if path != expected {
            return Err(EvidenceError::Corrupt);
        }
        if records.insert((run_id, attempt_id), record).is_some() {
            return Err(EvidenceError::Corrupt);
        }
    }
    Ok(records)
}

fn validate_record(record: &AttemptEvidence) -> Result<(), EvidenceError> {
    if record.version != EVIDENCE_RECORD_VERSION
        || record.phase.is_empty()
        || record.phase.len() > super::definition::MAXIMUM_KEY_BYTES
        || record.events.len() > MAXIMUM_ACTIVITY_EVENTS
        || record.activity_bytes > MAXIMUM_ACTIVITY_BYTES
    {
        return Err(EvidenceError::Corrupt);
    }
    let mut activity_bytes = 0usize;
    for (expected_sequence, event) in (1..).zip(&record.events) {
        if event.sequence != expected_sequence
            || event.phase != record.phase
            || event.text.len() > MAXIMUM_ACTIVITY_TEXT_BYTES
            || event.label.len() > MAXIMUM_ACTIVITY_TEXT_BYTES
            || event
                .provider
                .as_ref()
                .is_some_and(|value| value.len() > MAXIMUM_ACTIVITY_TEXT_BYTES)
        {
            return Err(EvidenceError::Corrupt);
        }
        activity_bytes = activity_bytes
            .saturating_add(event.text.len())
            .saturating_add(event.label.len())
            .saturating_add(event.provider.as_ref().map_or(0, String::len));
    }
    if activity_bytes != record.activity_bytes {
        return Err(EvidenceError::Corrupt);
    }
    if let Some(terminal) = &record.terminal
        && (terminal.phase != record.phase
            || terminal.text.len() > MAXIMUM_TERMINAL_TEXT_BYTES
            || terminal.thinking.len() > MAXIMUM_TERMINAL_TEXT_BYTES
            || terminal.tools.len() > MAXIMUM_TERMINAL_TOOLS
            || terminal
                .error
                .as_ref()
                .is_some_and(|error| error.len() > MAXIMUM_ACTIVITY_TEXT_BYTES)
            || terminal.tools.iter().any(|tool| {
                tool.label.len() > MAXIMUM_ACTIVITY_TEXT_BYTES
                    || tool.output.len() > MAXIMUM_TERMINAL_TOOL_BYTES
                    || tool
                        .command
                        .as_ref()
                        .is_some_and(|command| !command.is_bounded())
            })
            || terminal
                .tools
                .iter()
                .map(|tool| {
                    tool.label
                        .len()
                        .saturating_add(tool.output.len())
                        .saturating_add(tool.command.as_ref().map_or(0, |command| {
                            command
                                .chunks
                                .iter()
                                .map(|chunk| chunk.text.len())
                                .sum::<usize>()
                        }))
                })
                .sum::<usize>()
                > MAXIMUM_TERMINAL_CONTENT_BYTES)
    {
        return Err(EvidenceError::Corrupt);
    }
    Ok(())
}

fn evidence_path(
    dir: &Path,
    run_id: RunId,
    attempt_id: AttemptId,
) -> Result<PathBuf, EvidenceError> {
    crate::storage::confined_child(
        dir,
        &format!("{}-{}.json", run_id.as_hex(), attempt_id.as_hex()),
    )
    .map_err(|_| EvidenceError::Persist)
}

fn persist_record(dir: Option<&Path>, record: &mut AttemptEvidence) -> Result<(), EvidenceError> {
    validate_record(record)?;
    // JSON escaping also consumes the record budget. Preserve the terminal result before activity.
    let bytes = loop {
        let bytes = serde_json::to_vec(record).map_err(|_| EvidenceError::Persist)?;
        if bytes.len() <= MAXIMUM_EVIDENCE_RECORD_BYTES {
            break bytes;
        }
        if let Some(event) = record.events.pop() {
            record.activity_bytes -= event.text.len()
                + event.label.len()
                + event.provider.as_ref().map_or(0, String::len);
            record.activity_truncated = true;
        } else if let Some(terminal) = &mut record.terminal {
            terminal.truncated = true;
            if terminal.tools.pop().is_none() {
                terminal.text = bounded_text(&terminal.text, terminal.text.len() / 2).0;
                terminal.thinking = bounded_text(&terminal.thinking, terminal.thinking.len() / 2).0;
            }
        } else {
            return Err(EvidenceError::Full);
        }
    };
    let Some(dir) = dir else {
        return Ok(());
    };
    crate::storage::ensure_private_dir(dir).map_err(|_| EvidenceError::Persist)?;
    let run_id = RunId::parse(&record.run_id).ok_or(EvidenceError::Corrupt)?;
    let attempt_id = AttemptId::parse(&record.attempt_id).ok_or(EvidenceError::Corrupt)?;
    let path = evidence_path(dir, run_id, attempt_id)?;
    crate::storage::write_private(&path, &bytes).map_err(|_| EvidenceError::Persist)
}

#[cfg(test)]
mod tests;
