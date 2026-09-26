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

/// Merge adjacent chunks from the same stream. Capture reads split one stream
/// into several chunks, and a display must not repeat its heading for each.
pub(crate) fn merge_adjacent_streams(chunks: &[CommandChunk]) -> Vec<CommandChunk> {
    let mut merged: Vec<CommandChunk> = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        if let Some(last) = merged.last_mut().filter(|last| last.stream == chunk.stream) {
            last.text.push_str(&chunk.text);
        } else {
            merged.push(chunk.clone());
        }
    }
    merged
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CommandTermination {
    Exited(i32),
    Cancelled,
    TimedOut,
    /// Retained output reached its resource limit before the process ended.
    ResourceLimit,
    StorageFailure,
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
    /// A server-generated reference to the full retained output. The chunks above
    /// hold only the bounded display and model projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retained: Option<super::output::RetainedOutput>,
}

impl CommandResult {
    pub(crate) fn new(chunks: Vec<CommandChunk>, termination: CommandTermination) -> Self {
        Self {
            chunks,
            termination,
            retained: None,
        }
    }

    pub(crate) fn retain(mut self, retained: super::output::RetainedOutput) -> Self {
        self.retained = Some(retained);
        // Retention precedes the preview bound, so persistence never duplicates full capture.
        self.bounded(super::output::OUTPUT_PREVIEW_BYTES).0
    }

    pub(crate) fn retained_reference(&self) -> Option<&str> {
        self.retained
            .as_ref()
            .map(|retained| retained.reference.as_str())
    }

    /// Mark a result whose retained storage failed. Captured output stays visible.
    pub(crate) fn into_storage_limit(mut self) -> Self {
        self.termination = CommandTermination::StorageFailure;
        self.retained = None;
        self.bounded(super::output::OUTPUT_PREVIEW_BYTES).0
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
            CommandTermination::StorageFailure => "Output storage failed".to_owned(),
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
            | CommandTermination::StorageFailure
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

    /// The outcome and retrieval reference stay outside captured output and its preview bound.
    pub(crate) fn report(&self) -> String {
        let mut output = self.combined();
        if output.is_empty() {
            output.push_str("(no output)");
        }
        append_line(
            &mut output,
            &format!("Command outcome: {}.", self.status_text()),
        );
        if let Some(retained) = &self.retained {
            append_line(
                &mut output,
                &format!(
                    "Retained output reference: {}. read_output accepts this reference with offset 1. Retained bytes: {}. Storage truncated: {}.",
                    retained.reference, retained.bytes, retained.truncated,
                ),
            );
        }
        output
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
            retained: self.retained.clone(),
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
                retained: self.retained.clone(),
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
        let message = if result.termination == CommandTermination::StorageFailure {
            "Frinkworks could not store command output. Command effects can remain incomplete."
        } else {
            message
        };
        Self { result, message }
    }

    /// Text for the model tool result. Partial output precedes the failure reason.
    pub(crate) fn report(&self) -> String {
        let mut output = self.result.report();
        append_line(&mut output, self.message);
        output
    }
}

/// The result of one capture read. `safe` is already redacted.
pub(crate) struct CapturePush {
    pub(crate) overflow: bool,
    pub(crate) safe: Vec<CommandChunk>,
}

/// Buffer throttled progress so publication never skips an earlier chunk.
pub(crate) struct CommandProgress {
    chunks: Vec<CommandChunk>,
    bytes: usize,
    full: bool,
}

impl CommandProgress {
    pub(crate) fn new() -> Self {
        Self {
            chunks: Vec::new(),
            bytes: 0,
            full: false,
        }
    }

    pub(crate) fn push(&mut self, stream: CommandStream, mut text: String) {
        if text.is_empty() || self.full {
            return;
        }
        if self.chunks.len() >= MAXIMUM_COMMAND_CHUNKS {
            self.full = true;
            return;
        }
        let remaining = super::output::OUTPUT_PREVIEW_BYTES.saturating_sub(self.bytes);
        if text.len() > remaining {
            let mut end = remaining;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            self.full = true;
        }
        self.bytes += text.len();
        if let Some(last) = self
            .chunks
            .last_mut()
            .filter(|chunk| chunk.stream == stream)
        {
            last.text.push_str(&text);
        } else {
            self.chunks.push(CommandChunk { stream, text });
        }
    }

    pub(crate) fn publish(&mut self, job: &crate::sessions::Job, call: &str) {
        for chunk in self.chunks.drain(..) {
            job.push_tool_progress(call.to_owned(), chunk.stream, chunk.text);
        }
    }
}

/// Ordered capture of observed stream bytes. Incomplete UTF-8 waits for the next read.
pub(crate) struct CommandCapture<'a> {
    chunks: Vec<CommandChunk>,
    pending: Vec<(CommandStream, Vec<u8>)>,
    pending_bytes: usize,
    secret: Option<String>,
    store: Option<&'a super::output::OutputStore>,
    retained: Option<super::output::RetainedOutput>,
    storage_failed: bool,
    bytes: usize,
    overflowed: bool,
    limit: usize,
}

impl<'a> CommandCapture<'a> {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::with_limit(super::output::MAXIMUM_RETAINED_BYTES)
    }

    #[cfg(test)]
    pub(crate) fn with_limit(limit: usize) -> Self {
        Self::with_secret_limit(None, limit)
    }

    pub(crate) fn with_secret(secret: Option<&str>) -> Self {
        Self::with_secret_limit(secret, super::output::MAXIMUM_RETAINED_BYTES)
    }

    pub(crate) fn with_output(
        secret: Option<&str>,
        store: &'a super::output::OutputStore,
        key: &super::output::OutputKey,
    ) -> Result<Self, super::output::OutputError> {
        let retained = store.store(
            key,
            &CommandResult::new(Vec::new(), CommandTermination::Unknown),
        )?;
        let mut capture = Self::with_secret(secret);
        capture.store = Some(store);
        capture.retained = Some(retained);
        Ok(capture)
    }

    pub(crate) fn with_secret_limit(secret: Option<&str>, limit: usize) -> Self {
        Self {
            chunks: Vec::new(),
            pending: Vec::new(),
            pending_bytes: 0,
            secret: secret
                .filter(|secret| !secret.is_empty())
                .map(str::to_owned),
            store: None,
            retained: None,
            storage_failed: false,
            bytes: 0,
            overflowed: false,
            limit,
        }
    }

    /// Append received bytes for one stream. The safe text is redacted.
    pub(crate) fn push(&mut self, stream: CommandStream, bytes: &[u8]) -> CapturePush {
        if self.overflowed || bytes.is_empty() {
            return CapturePush {
                overflow: self.overflowed,
                safe: Vec::new(),
            };
        }
        let remaining = self.limit.saturating_sub(self.bytes + self.pending_bytes);
        let count = bytes.len().min(remaining);
        let mut overflow = count < bytes.len();
        if let Some((_, last)) = self.pending.last_mut().filter(|(kind, _)| *kind == stream) {
            last.extend_from_slice(&bytes[..count]);
            self.pending_bytes += count;
        } else if self.chunks.len() + self.pending.len() < MAXIMUM_COMMAND_CHUNKS {
            self.pending.push((stream, bytes[..count].to_vec()));
            self.pending_bytes += count;
        } else {
            overflow = true;
        }
        let safe = self.flush_pending(overflow);
        self.overflowed |= overflow;
        if !safe.is_empty() || overflow {
            self.checkpoint();
        }
        CapturePush {
            overflow: self.overflowed || self.storage_failed,
            safe,
        }
    }

    /// Flush any complete or replacement-decoded tail.
    pub(crate) fn finish(&mut self) -> Vec<CommandChunk> {
        self.flush_pending(true);
        self.checkpoint();
        std::mem::take(&mut self.chunks)
    }

    pub(crate) fn into_result(mut self, termination: CommandTermination) -> CommandResult {
        self.finish_result(termination)
    }

    pub(crate) fn finish_result(&mut self, termination: CommandTermination) -> CommandResult {
        let chunks = self.finish();
        let termination = if self.storage_failed {
            CommandTermination::StorageFailure
        } else if self.overflowed {
            CommandTermination::ResourceLimit
        } else {
            termination
        };
        let result = CommandResult::new(chunks, termination);
        match &self.retained {
            Some(retained) => result.retain(retained.clone()),
            None => result,
        }
    }

    fn checkpoint(&mut self) {
        if self.storage_failed {
            return;
        }
        if let (Some(store), Some(retained)) = (self.store, &self.retained) {
            let termination = if self.overflowed {
                CommandTermination::ResourceLimit
            } else {
                CommandTermination::Unknown
            };
            match store.checkpoint(
                &retained.reference,
                &CommandResult::new(self.chunks.clone(), termination),
            ) {
                Ok(retained) => self.retained = Some(retained),
                Err(_) => self.storage_failed = true,
            }
        }
    }

    fn flush_pending(&mut self, final_chunk: bool) -> Vec<CommandChunk> {
        let Some(chunks) = decode_ordered(&self.pending, final_chunk) else {
            return Vec::new();
        };
        if !final_chunk
            && self.secret.as_deref().is_some_and(|secret| {
                [
                    None,
                    Some(CommandStream::Stdout),
                    Some(CommandStream::Stderr),
                ]
                .into_iter()
                .any(|stream| {
                    let text: String = chunks
                        .iter()
                        .filter(|chunk| stream.is_none_or(|stream| chunk.stream == stream))
                        .map(|chunk| chunk.text.as_str())
                        .collect();
                    let text = crate::tools::redact(&text, Some(secret));
                    (1..secret.len().min(text.len() + 1)).any(|length| {
                        secret.is_char_boundary(length) && text.ends_with(&secret[..length])
                    })
                })
            })
        {
            return Vec::new();
        }
        self.pending.clear();
        self.pending_bytes = 0;
        let safe = CommandResult::new(chunks, CommandTermination::Unknown)
            .redacted(self.secret.as_deref());
        let start = self.chunks.len();
        for chunk in safe.chunks {
            self.record(chunk.stream, &chunk.text);
        }
        self.chunks[start..].to_vec()
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

/// Capture one sandbox command with bounded output and live progress.
///
/// Model tools and direct sandbox commands share this loop so
/// cancellation, timeouts, redaction and retained output behave the same way.
/// `tool_call` labels live progress only. It is not an authority token.
pub(crate) async fn capture_sandbox_command(
    sandbox: &crate::sandbox::GuestSandbox,
    request: crate::sandbox::GuestExec,
    job: &crate::sessions::Job,
    secret: Option<&str>,
    timeout: std::time::Duration,
    tool_call: &str,
    retained: Option<(&super::output::OutputStore, &super::output::OutputKey)>,
) -> Result<CommandResult, CommandFailure> {
    let mut capture = match retained {
        Some((store, key)) => match CommandCapture::with_output(secret, store, key) {
            Ok(capture) => capture,
            Err(error) => {
                return Err(CommandFailure::new(
                    CommandResult::new(Vec::new(), CommandTermination::NotDispatched),
                    error.message(),
                ));
            }
        },
        None => CommandCapture::with_secret(secret),
    };
    if job.cancel_requested() {
        return Err(CommandFailure::new(
            capture.into_result(CommandTermination::NotDispatched),
            "Stopped before command dispatch.",
        ));
    }
    let mut session = match sandbox.exec_cmd(request).await {
        Ok(session) => session,
        Err(error) => {
            return Err(CommandFailure::new(
                CommandResult::new(Vec::new(), CommandTermination::NotDispatched),
                error.message(),
            ));
        }
    };
    let deadline = tokio::time::Instant::now() + timeout;
    let mut progress = CommandProgress::new();
    let mut progress_tick = tokio::time::interval(std::time::Duration::from_millis(100));
    progress_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut exit = None;
    loop {
        let event = tokio::select! {
            biased;
            _ = job.cancelled() => {
                session.kill().await;
                session.close().await;
                return Err(CommandFailure::new(
                    capture.into_result(CommandTermination::Cancelled),
                    "Stopped.",
                ));
            }
            _ = tokio::time::sleep_until(deadline) => {
                session.kill().await;
                session.close().await;
                return Err(CommandFailure::new(
                    capture.into_result(CommandTermination::TimedOut),
                    "The command exceeded the time limit.",
                ));
            }
            _ = progress_tick.tick() => {
                progress.publish(job, tool_call);
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
                    return Err(CommandFailure::new(
                        capture.into_result(CommandTermination::ResourceLimit),
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
                return Err(CommandFailure::new(
                    capture.into_result(CommandTermination::Unknown),
                    "The command outcome is unknown. Inspect the local execution evidence before further work.",
                ));
            }
        }
    }
    if exit.is_none() {
        session.kill().await;
    }
    session.close().await;
    match exit {
        Some(code) => Ok(capture.into_result(CommandTermination::Exited(code))),
        None => Err(CommandFailure::new(
            capture.into_result(CommandTermination::Unknown),
            "The command outcome is unknown. Inspect the local execution evidence before further work.",
        )),
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

// Each character belongs to the chunk that supplied its first byte, even if
// another stream arrived before the remaining UTF-8 bytes.
fn decode_ordered(
    pending: &[(CommandStream, Vec<u8>)],
    final_chunk: bool,
) -> Option<Vec<CommandChunk>> {
    let mut chunks: Vec<_> = pending
        .iter()
        .map(|(stream, _)| CommandChunk {
            stream: *stream,
            text: String::new(),
        })
        .collect();
    for stream in [CommandStream::Stdout, CommandStream::Stderr] {
        let mut bytes = Vec::new();
        let mut owners = Vec::new();
        for (index, (kind, data)) in pending.iter().enumerate() {
            if *kind == stream {
                bytes.extend_from_slice(data);
                owners.extend(std::iter::repeat_n(index, data.len()));
            }
        }
        let mut offset = 0;
        while offset < bytes.len() {
            let (valid, invalid) = match std::str::from_utf8(&bytes[offset..]) {
                Ok(text) => (text.len(), 0),
                Err(error) => {
                    if error.error_len().is_none() && !final_chunk {
                        return None;
                    }
                    (
                        error.valid_up_to(),
                        error
                            .error_len()
                            .unwrap_or(bytes.len() - offset - error.valid_up_to()),
                    )
                }
            };
            let text = std::str::from_utf8(&bytes[offset..offset + valid]).expect("valid prefix");
            for (index, character) in text.char_indices() {
                chunks[owners[offset + index]].text.push(character);
            }
            offset += valid;
            if invalid > 0 {
                chunks[owners[offset]].text.push('\u{fffd}');
                offset += invalid;
            }
        }
    }
    Some(chunks)
}

#[cfg(test)]
mod tests;
