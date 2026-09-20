use serde::{Deserialize, Serialize};

/// The capture bound is a resource limit. It is separate from any display limit.
pub(crate) const MAXIMUM_COMMAND_CHUNKS: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CommandStream {
    Stdout,
    Stderr,
}

impl CommandStream {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }

    pub(crate) fn is_stderr(self) -> bool {
        self == Self::Stderr
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct CommandChunk {
    pub(crate) stream: CommandStream,
    pub(crate) text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CommandTermination {
    Exited(i32),
    Cancelled,
    TimedOut,
    /// Retained output reached its resource limit before the process ended.
    ResourceLimit,
    NotDispatched,
    /// No trustworthy termination is available.
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct CommandResult {
    /// Chunks appear in the arrival order the capture observed. Stream identity is
    /// preserved even when stdout and stderr interleave.
    pub(crate) chunks: Vec<CommandChunk>,
    pub(crate) termination: CommandTermination,
}

impl CommandResult {
    pub(crate) fn new(chunks: Vec<CommandChunk>, termination: CommandTermination) -> Self {
        Self {
            chunks,
            termination,
        }
    }

    pub(crate) fn exit_code(&self) -> Option<i32> {
        match self.termination {
            CommandTermination::Exited(code) => Some(code),
            _ => None,
        }
    }

    pub(crate) fn is_success(&self) -> bool {
        self.exit_code() == Some(0)
    }

    /// A failure label that remains meaningful without colour.
    pub(crate) fn status_text(&self) -> String {
        match self.termination {
            CommandTermination::Exited(code) => format!("Exit code {code}"),
            CommandTermination::Cancelled => "Cancelled".to_owned(),
            CommandTermination::TimedOut => "Timed out".to_owned(),
            CommandTermination::ResourceLimit => "Output limit reached".to_owned(),
            CommandTermination::NotDispatched => "Not started".to_owned(),
            CommandTermination::Unknown => "Result unknown".to_owned(),
        }
    }

    pub(crate) fn is_error(&self) -> bool {
        match self.termination {
            CommandTermination::Exited(code) => code != 0,
            CommandTermination::Cancelled
            | CommandTermination::TimedOut
            | CommandTermination::ResourceLimit
            | CommandTermination::NotDispatched
            | CommandTermination::Unknown => true,
        }
    }

    pub(crate) fn combined(&self) -> String {
        let mut text = String::new();
        for chunk in &self.chunks {
            text.push_str(&chunk.text);
        }
        text
    }

    fn outcome_text(&self) -> Option<String> {
        match self.termination {
            CommandTermination::Exited(0) => None,
            CommandTermination::Exited(code) => {
                Some(format!("The command exited with code {code}."))
            }
            CommandTermination::ResourceLimit => {
                Some("The command exceeded the output resource limit.".to_owned())
            }
            CommandTermination::Cancelled => Some("The command was cancelled.".to_owned()),
            CommandTermination::TimedOut => Some("The command exceeded the time limit.".to_owned()),
            CommandTermination::NotDispatched => Some("The command did not start.".to_owned()),
            CommandTermination::Unknown => Some("The command result is unknown.".to_owned()),
        }
    }

    /// Text for the model tool result. The exit line stays outside captured output.
    pub(crate) fn report(&self) -> String {
        let mut output = self.combined();
        if let Some(outcome) = self.outcome_text() {
            append_line(&mut output, &outcome);
        }
        if output.is_empty() {
            "(no output)".to_owned()
        } else {
            output
        }
    }

    pub(crate) fn is_bounded(&self) -> bool {
        self.chunks.len() <= MAXIMUM_COMMAND_CHUNKS
            && self.chunks.iter().all(|chunk| !chunk.text.contains('\0'))
            && self
                .chunks
                .iter()
                .map(|chunk| chunk.text.len())
                .sum::<usize>()
                <= crate::tools::MAXIMUM_TOOL_BYTES
    }

    /// Redact a credential even when the stream split it across chunk boundaries.
    pub(crate) fn redacted(&self, secret: Option<&str>) -> Self {
        let Some(secret) = secret.filter(|secret| !secret.is_empty()) else {
            return self.clone();
        };
        let mut hidden = vec![false; self.chunks.iter().map(|chunk| chunk.text.len()).sum()];
        // Check both arrival order and each stream so interleaved diagnostics cannot expose a credential.
        for stream in [
            None,
            Some(CommandStream::Stdout),
            Some(CommandStream::Stderr),
        ] {
            let mut text = String::new();
            let mut positions = Vec::new();
            let mut offset = 0;
            for chunk in &self.chunks {
                if stream.is_none_or(|stream| chunk.stream == stream) {
                    text.push_str(&chunk.text);
                    positions.extend(offset..offset + chunk.text.len());
                }
                offset += chunk.text.len();
            }
            for (start, _) in text.match_indices(secret) {
                for &position in &positions[start..start + secret.len()] {
                    hidden[position] = true;
                }
            }
            let retained = (1..secret.len().min(text.len() + 1))
                .rev()
                .find(|&length| {
                    secret.is_char_boundary(length) && text.ends_with(&secret[..length])
                })
                .unwrap_or(0);
            for &position in &positions[positions.len() - retained..] {
                hidden[position] = true;
            }
        }
        let mut offset = 0;
        let chunks = self
            .chunks
            .iter()
            .map(|chunk| {
                let mut text = String::new();
                let mut redacting = false;
                for (index, character) in chunk.text.char_indices() {
                    if hidden[offset + index] {
                        if !redacting {
                            text.push_str("[redacted]");
                        }
                        redacting = true;
                    } else {
                        redacting = false;
                        text.push(character);
                    }
                }
                offset += chunk.text.len();
                CommandChunk {
                    stream: chunk.stream,
                    text,
                }
            })
            .collect();
        Self {
            chunks,
            termination: self.termination,
        }
    }

    /// Return a copy with at most `maximum` bytes of chunk text. The bool reports
    /// whether any text was removed.
    pub(crate) fn bounded(&self, maximum: usize) -> (Self, bool) {
        let total = self
            .chunks
            .iter()
            .map(|chunk| chunk.text.len())
            .sum::<usize>();
        if total <= maximum {
            return (self.clone(), false);
        }
        let mut chunks = Vec::new();
        let mut bytes = 0usize;
        for chunk in &self.chunks {
            let remaining = maximum.saturating_sub(bytes);
            if remaining == 0 {
                break;
            }
            let text = if chunk.text.len() <= remaining {
                chunk.text.clone()
            } else {
                let mut end = remaining;
                while end > 0 && !chunk.text.is_char_boundary(end) {
                    end -= 1;
                }
                chunk.text[..end].to_owned()
            };
            bytes += text.len();
            chunks.push(CommandChunk {
                stream: chunk.stream,
                text,
            });
            if chunk.text.len() > remaining || bytes >= maximum {
                break;
            }
        }
        (
            Self {
                chunks,
                termination: self.termination,
            },
            true,
        )
    }
}

/// A command that ended without a successful result. Captured output is retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommandFailure {
    pub(crate) result: CommandResult,
    pub(crate) message: &'static str,
}

impl CommandFailure {
    pub(crate) fn new(result: CommandResult, message: &'static str) -> Self {
        Self { result, message }
    }

    /// Text for the model tool result. Partial output precedes the failure reason.
    pub(crate) fn report(&self) -> String {
        let mut output = self.result.combined();
        if let Some(outcome) = self.result.outcome_text() {
            append_line(&mut output, &outcome);
        }
        append_line(&mut output, self.message);
        if output.is_empty() {
            self.message.to_owned()
        } else {
            output
        }
    }
}

/// Ordered capture of observed stream bytes. Incomplete UTF-8 waits for the next read.
pub(crate) struct CommandCapture {
    chunks: Vec<CommandChunk>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    bytes: usize,
    overflowed: bool,
    limit: usize,
}

impl CommandCapture {
    pub(crate) fn new() -> Self {
        Self::with_limit(crate::tools::MAXIMUM_TOOL_BYTES)
    }

    pub(crate) fn with_limit(limit: usize) -> Self {
        Self {
            chunks: Vec::new(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            bytes: 0,
            overflowed: false,
            limit,
        }
    }

    /// Append received bytes for one stream. Return true when the capture bound is reached.
    pub(crate) fn push(&mut self, stream: CommandStream, bytes: &[u8]) -> bool {
        if self.overflowed {
            return true;
        }
        let text = match stream {
            CommandStream::Stdout => {
                self.stdout.extend_from_slice(bytes);
                drain_decoded(&mut self.stdout)
            }
            CommandStream::Stderr => {
                self.stderr.extend_from_slice(bytes);
                drain_decoded(&mut self.stderr)
            }
        };
        self.record(stream, &text)
    }

    /// Flush any complete or replacement-decoded tail.
    pub(crate) fn finish(&mut self) -> Vec<CommandChunk> {
        let stdout = std::mem::take(&mut self.stdout);
        if !stdout.is_empty() {
            let text = String::from_utf8_lossy(&stdout).into_owned();
            self.record(CommandStream::Stdout, &text);
        }
        let stderr = std::mem::take(&mut self.stderr);
        if !stderr.is_empty() {
            let text = String::from_utf8_lossy(&stderr).into_owned();
            self.record(CommandStream::Stderr, &text);
        }
        std::mem::take(&mut self.chunks)
    }

    pub(crate) fn into_result(mut self, termination: CommandTermination) -> CommandResult {
        let chunks = self.finish();
        let termination = if self.overflowed {
            CommandTermination::ResourceLimit
        } else {
            termination
        };
        CommandResult::new(chunks, termination)
    }

    fn record(&mut self, stream: CommandStream, text: &str) -> bool {
        if text.is_empty() || self.overflowed {
            return self.overflowed;
        }
        if self.chunks.len() >= MAXIMUM_COMMAND_CHUNKS {
            self.overflowed = true;
            return true;
        }
        let text = text.replace('\0', "\u{fffd}");
        let remaining = self.limit.saturating_sub(self.bytes);
        if text.len() <= remaining {
            self.bytes += text.len();
            self.chunks.push(CommandChunk {
                stream,
                text: text.to_owned(),
            });
            return false;
        }
        let mut end = remaining;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end > 0 {
            self.bytes += end;
            self.chunks.push(CommandChunk {
                stream,
                text: text[..end].to_owned(),
            });
        }
        self.overflowed = true;
        true
    }
}

fn append_line(output: &mut String, line: &str) {
    if line.is_empty() {
        return;
    }
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(line);
}

/// Decode every complete UTF-8 sequence. Incomplete trailing bytes stay pending.
fn drain_decoded(pending: &mut Vec<u8>) -> String {
    let mut output = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(text) => {
                output.push_str(text);
                pending.clear();
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                output.push_str(std::str::from_utf8(&pending[..valid]).expect("valid prefix"));
                match error.error_len() {
                    Some(invalid) => {
                        output.push('\u{fffd}');
                        pending.drain(..valid + invalid);
                        if pending.is_empty() {
                            break;
                        }
                    }
                    None => {
                        pending.drain(..valid);
                        break;
                    }
                }
            }
        }
    }
    output
}
