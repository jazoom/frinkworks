use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::conversations::ConversationId;
use crate::sessions::{Job, JobId, SessionId};

#[cfg(test)]
mod tests;

pub(crate) const ASK_USER: &str = "ask_user";
pub(crate) const MAXIMUM_QUESTION_BYTES: usize = 4 * 1024;
pub(crate) const MAXIMUM_ANSWER_BYTES: usize = MAXIMUM_QUESTION_BYTES;
pub(crate) const MAXIMUM_OPTIONS: usize = 8;
pub(crate) const MAXIMUM_OPTION_ID_BYTES: usize = 64;
pub(crate) const MAXIMUM_OPTION_LABEL_BYTES: usize = 256;
pub(crate) const CANCELLED_RESULT: &str = "The user cancelled this question.";
pub(crate) const INTERRUPTED_RESULT: &str = "The question was interrupted.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuestionError {
    Invalid,
    Duplicate,
    Missing,
    JobCancelled,
    Bound,
}

impl QuestionError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Invalid => "That answer does not match the question.",
            Self::Duplicate => "That question already has an answer.",
            Self::Missing => "That question is no longer waiting.",
            Self::JobCancelled => "Stopped.",
            Self::Bound => "Enter a question or answer within the limit.",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct QuestionOption {
    pub(crate) id: String,
    pub(crate) label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingQuestion {
    pub(crate) conversation: ConversationId,
    pub(crate) job: JobId,
    pub(crate) session: SessionId,
    pub(crate) tool_call: String,
    pub(crate) prompt: String,
    pub(crate) options: Vec<QuestionOption>,
    pub(crate) allow_free_text: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum QuestionAnswer {
    Option { id: String, label: String },
    FreeText { text: String },
    Cancelled,
}

impl QuestionAnswer {
    pub(crate) fn result_text(&self) -> String {
        match self {
            Self::Option { id, label } => {
                format!("The user selected option `{id}`: {label}")
            }
            Self::FreeText { text } => format!("The user answered: {text}"),
            Self::Cancelled => CANCELLED_RESULT.to_owned(),
        }
    }
}

pub(crate) fn parse_question(
    arguments: &serde_json::Value,
    conversation: ConversationId,
    job: JobId,
    session: SessionId,
    tool_call: &str,
) -> Result<PendingQuestion, QuestionError> {
    if tool_call.is_empty()
        || tool_call.len() > crate::conversations::history::MAXIMUM_CALL_IDENTIFIER_BYTES
        || tool_call.contains('\0')
    {
        return Err(QuestionError::Invalid);
    }
    let prompt = bounded_text(
        arguments
            .get("question")
            .and_then(serde_json::Value::as_str),
        MAXIMUM_QUESTION_BYTES,
    )?;
    let options = parse_options(arguments.get("options"))?;
    let allow_free_text = match arguments.get("allow_free_text") {
        None => options.is_empty(),
        Some(value) => value.as_bool().ok_or(QuestionError::Invalid)?,
    };
    if options.is_empty() && !allow_free_text {
        return Err(QuestionError::Invalid);
    }
    Ok(PendingQuestion {
        conversation,
        job,
        session,
        tool_call: tool_call.to_owned(),
        prompt,
        options,
        allow_free_text,
    })
}

pub(crate) fn parse_answer(
    question: &PendingQuestion,
    option: Option<&str>,
    text: Option<&str>,
    cancelled: bool,
) -> Result<QuestionAnswer, QuestionError> {
    if cancelled {
        if option.is_some_and(|value| !value.is_empty())
            || text.is_some_and(|value| !value.is_empty())
        {
            return Err(QuestionError::Invalid);
        }
        return Ok(QuestionAnswer::Cancelled);
    }
    match (
        option.map(str::trim).filter(|value| !value.is_empty()),
        text,
    ) {
        (Some(id), None | Some("")) => {
            let option = question
                .options
                .iter()
                .find(|option| option.id == id)
                .ok_or(QuestionError::Invalid)?;
            Ok(QuestionAnswer::Option {
                id: option.id.clone(),
                label: option.label.clone(),
            })
        }
        (None, Some(text)) if question.allow_free_text => Ok(QuestionAnswer::FreeText {
            text: bounded_text(Some(text), MAXIMUM_ANSWER_BYTES)?,
        }),
        _ => Err(QuestionError::Invalid),
    }
}

pub(crate) fn valid_question(question: &PendingQuestion) -> bool {
    parse_question(
        &serde_json::json!({
            "question": question.prompt,
            "options": question.options,
            "allow_free_text": question.allow_free_text,
        }),
        question.conversation,
        question.job,
        question.session,
        &question.tool_call,
    )
    .ok()
    .as_ref()
        == Some(question)
}

struct PendingWait {
    question: PendingQuestion,
    answer: Mutex<Option<QuestionAnswer>>,
    notify: Notify,
    claimed: AtomicBool,
    invalidated: AtomicBool,
}

#[derive(Default)]
pub(crate) struct QuestionWaiters {
    pending: Mutex<HashMap<(ConversationId, JobId), Arc<PendingWait>>>,
    inflight: Mutex<HashMap<(ConversationId, JobId), Arc<PendingWait>>>,
}

impl QuestionWaiters {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn submit(&self, question: PendingQuestion) -> Result<(), QuestionError> {
        if !valid_question(&question) {
            return Err(QuestionError::Invalid);
        }
        let key = (question.conversation, question.job);
        let wait = Arc::new(PendingWait {
            question,
            answer: Mutex::new(None),
            notify: Notify::new(),
            claimed: AtomicBool::new(false),
            invalidated: AtomicBool::new(false),
        });
        let mut pending = lock(&self.pending);
        let mut inflight = lock(&self.inflight);
        if pending.contains_key(&key)
            || inflight.contains_key(&key)
            || inflight
                .values()
                .any(|stored| stored.question.job == wait.question.job)
        {
            return Err(QuestionError::Duplicate);
        }
        pending.insert(key, wait.clone());
        inflight.insert(key, wait);
        Ok(())
    }

    pub(crate) fn pending_for(
        &self,
        conversation: ConversationId,
        job: JobId,
    ) -> Option<PendingQuestion> {
        lock(&self.pending)
            .get(&(conversation, job))
            .map(|wait| wait.question.clone())
    }

    // A claimed answer stays unresolved until execution consumes it.
    pub(crate) fn has_pending(&self, conversation: ConversationId) -> bool {
        lock(&self.pending)
            .keys()
            .any(|(id, _)| *id == conversation)
            || lock(&self.inflight)
                .keys()
                .any(|(id, _)| *id == conversation)
    }

    pub(crate) fn decide(
        &self,
        conversation: ConversationId,
        job: &Job,
        tool_call: &str,
        answer: QuestionAnswer,
    ) -> Result<(), QuestionError> {
        let key = (conversation, job.id());
        let mut pending = lock(&self.pending);
        let wait = pending.get(&key).ok_or(QuestionError::Missing)?;
        if wait.question.tool_call != tool_call {
            return Err(QuestionError::Invalid);
        }
        let validated = match &answer {
            QuestionAnswer::Option { id, .. } => {
                parse_answer(&wait.question, Some(id), None, false)
            }
            QuestionAnswer::FreeText { text } => {
                parse_answer(&wait.question, None, Some(text), false)
            }
            QuestionAnswer::Cancelled => Ok(QuestionAnswer::Cancelled),
        }?;
        if validated != answer {
            return Err(QuestionError::Invalid);
        }
        if wait.invalidated.load(Ordering::Acquire) || job.cancel_requested() {
            return Err(QuestionError::Missing);
        }
        let mut stored = lock(&wait.answer);
        if stored.is_some() {
            return Err(QuestionError::Duplicate);
        }
        // Resume before notification so a fast next question keeps its own wait status.
        let _ = job.resume();
        *stored = Some(answer);
        drop(stored);
        wait.notify.notify_one();
        pending.remove(&key);
        Ok(())
    }

    pub(crate) async fn wait(
        &self,
        conversation: ConversationId,
        job: &Job,
        tool_call: &str,
    ) -> Result<QuestionAnswer, QuestionError> {
        let wait = lock(&self.inflight)
            .get(&(conversation, job.id()))
            .cloned()
            .ok_or(QuestionError::Missing)?;
        if wait.question.tool_call != tool_call {
            return Err(QuestionError::Invalid);
        }
        if wait.claimed.swap(true, Ordering::AcqRel) {
            return Err(QuestionError::Duplicate);
        }
        loop {
            if job.cancel_requested() || wait.invalidated.load(Ordering::Acquire) {
                self.invalidate_job(job.id());
                return Err(QuestionError::JobCancelled);
            }
            if let Some(answer) = lock(&wait.answer).clone() {
                lock(&self.inflight).remove(&(conversation, job.id()));
                return Ok(answer);
            }
            tokio::select! {
                biased;
                _ = job.cancelled() => {
                    self.invalidate_job(job.id());
                    return Err(QuestionError::JobCancelled);
                }
                _ = wait.notify.notified() => {}
            }
        }
    }

    pub(crate) fn invalidate_job(&self, job: JobId) {
        let drop_job = |wait: &Arc<PendingWait>| {
            if wait.question.job == job {
                wait.invalidated.store(true, Ordering::Release);
                wait.notify.notify_one();
                false
            } else {
                true
            }
        };
        lock(&self.pending).retain(|_, wait| drop_job(wait));
        lock(&self.inflight).retain(|_, wait| drop_job(wait));
    }

    pub(crate) fn invalidate_conversation(&self, conversation: ConversationId) {
        let drop_conversation = |wait: &Arc<PendingWait>| {
            if wait.question.conversation == conversation {
                wait.invalidated.store(true, Ordering::Release);
                wait.notify.notify_one();
                false
            } else {
                true
            }
        };
        lock(&self.pending).retain(|_, wait| drop_conversation(wait));
        lock(&self.inflight).retain(|_, wait| drop_conversation(wait));
    }

    pub(crate) fn retain_sessions(&self, live: impl Fn(&SessionId) -> bool) {
        let drop_session = |wait: &Arc<PendingWait>| {
            if live(&wait.question.session) {
                true
            } else {
                wait.invalidated.store(true, Ordering::Release);
                wait.notify.notify_one();
                false
            }
        };
        lock(&self.pending).retain(|_, wait| drop_session(wait));
        lock(&self.inflight).retain(|_, wait| drop_session(wait));
    }
}

fn parse_options(value: Option<&serde_json::Value>) -> Result<Vec<QuestionOption>, QuestionError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value.as_array().ok_or(QuestionError::Invalid)?;
    if items.len() > MAXIMUM_OPTIONS {
        return Err(QuestionError::Bound);
    }
    let mut options = Vec::with_capacity(items.len());
    let mut seen = HashSet::new();
    for item in items {
        let id = bounded_identifier(
            item.get("id").and_then(serde_json::Value::as_str),
            MAXIMUM_OPTION_ID_BYTES,
        )?;
        let label = bounded_text(
            item.get("label").and_then(serde_json::Value::as_str),
            MAXIMUM_OPTION_LABEL_BYTES,
        )?;
        if !seen.insert(id.clone()) {
            return Err(QuestionError::Invalid);
        }
        options.push(QuestionOption { id, label });
    }
    Ok(options)
}

fn bounded_text(value: Option<&str>, maximum: usize) -> Result<String, QuestionError> {
    let text = value.ok_or(QuestionError::Invalid)?.trim();
    if text.is_empty() || text.len() > maximum {
        return Err(QuestionError::Bound);
    }
    if text
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(QuestionError::Invalid);
    }
    Ok(text.to_owned())
}

fn bounded_identifier(value: Option<&str>, maximum: usize) -> Result<String, QuestionError> {
    let text = value.ok_or(QuestionError::Invalid)?.trim();
    if text.is_empty() || text.len() > maximum || text.contains('\0') {
        return Err(QuestionError::Invalid);
    }
    if text
        .chars()
        .any(|character| !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_')))
    {
        return Err(QuestionError::Invalid);
    }
    Ok(text.to_owned())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
