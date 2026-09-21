use super::{
    MAXIMUM_OPTIONS, MAXIMUM_QUESTION_BYTES, PendingQuestion, QuestionAnswer, QuestionError,
    QuestionWaiters, parse_answer, parse_question,
};
use crate::conversations::ConversationId;
use crate::sessions::{Job, JobId};

fn conversation() -> ConversationId {
    ConversationId::generate().expect("conversation")
}

fn job_id() -> JobId {
    JobId::generate().expect("job")
}

fn session() -> crate::sessions::SessionId {
    crate::sessions::generate_session_token()
        .expect("session")
        .id()
}

fn arguments(
    question: &str,
    options: serde_json::Value,
    free_text: Option<bool>,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "question": question,
        "options": options,
    });
    if let Some(allow_free_text) = free_text {
        value["allow_free_text"] = serde_json::Value::Bool(allow_free_text);
    }
    value
}

#[test]
fn unique_option_identifiers_are_required() {
    let error = parse_question(
        &arguments(
            "Choose",
            serde_json::json!([
                {"id": "yes", "label": "Yes"},
                {"id": "yes", "label": "Also yes"}
            ]),
            Some(false),
        ),
        conversation(),
        job_id(),
        session(),
        "call-1",
    );
    assert_eq!(error, Err(QuestionError::Invalid));
}

#[test]
fn answers_outside_the_submitted_question_are_rejected() {
    let question = parse_question(
        &arguments(
            "Choose",
            serde_json::json!([
                {"id": "yes", "label": "Yes"},
                {"id": "no", "label": "No"}
            ]),
            Some(false),
        ),
        conversation(),
        job_id(),
        session(),
        "call-1",
    )
    .expect("question");
    assert_eq!(
        parse_answer(&question, Some("maybe"), None, false),
        Err(QuestionError::Invalid)
    );
    assert_eq!(
        parse_answer(&question, None, Some("free text"), false),
        Err(QuestionError::Invalid)
    );
    assert_eq!(
        parse_answer(&question, Some("yes"), Some("also"), false),
        Err(QuestionError::Invalid)
    );
    assert_eq!(
        parse_answer(&question, Some("yes"), None, false),
        Ok(QuestionAnswer::Option {
            id: "yes".to_owned(),
            label: "Yes".to_owned(),
        })
    );
}

#[test]
fn cancellation_is_not_an_empty_successful_answer() {
    let question = parse_question(
        &arguments("Need a path?", serde_json::json!([]), Some(true)),
        conversation(),
        job_id(),
        session(),
        "call-1",
    )
    .expect("question");
    let cancelled = parse_answer(&question, None, None, true).expect("cancelled");
    assert_eq!(cancelled, QuestionAnswer::Cancelled);
    assert_eq!(
        parse_answer(&question, None, Some(""), false),
        Err(QuestionError::Bound)
    );
}

#[test]
fn question_and_option_bounds_are_enforced() {
    let oversized = "x".repeat(MAXIMUM_QUESTION_BYTES + 1);
    assert_eq!(
        parse_question(
            &arguments(&oversized, serde_json::json!([]), Some(true)),
            conversation(),
            job_id(),
            session(),
            "call-1",
        ),
        Err(QuestionError::Bound)
    );
    let options: Vec<_> = (0..=MAXIMUM_OPTIONS)
        .map(|index| serde_json::json!({"id": format!("opt{index}"), "label": "Choice"}))
        .collect();
    assert_eq!(
        parse_question(
            &arguments("Choose", serde_json::Value::Array(options), Some(false)),
            conversation(),
            job_id(),
            session(),
            "call-1",
        ),
        Err(QuestionError::Bound)
    );
}

#[test]
fn stale_and_duplicate_answers_are_rejected() {
    let waiters = QuestionWaiters::new();
    let question = PendingQuestion {
        conversation: conversation(),
        job: job_id(),
        session: session(),
        tool_call: "call-1".to_owned(),
        prompt: "Need a path?".to_owned(),
        options: Vec::new(),
        allow_free_text: true,
    };
    let job = Job::for_conversation(question.job, question.conversation);
    job.set_awaiting_question();
    waiters.submit(question.clone()).expect("submit");
    assert_eq!(
        waiters.decide(
            question.conversation,
            &job,
            "call-1",
            QuestionAnswer::Option {
                id: "invented".to_owned(),
                label: "Invented".to_owned()
            },
        ),
        Err(QuestionError::Invalid)
    );
    assert_eq!(
        waiters.decide(
            question.conversation,
            &job,
            "call-1",
            QuestionAnswer::FreeText {
                text: "x".repeat(super::MAXIMUM_ANSWER_BYTES + 1)
            },
        ),
        Err(QuestionError::Bound)
    );
    assert_eq!(
        waiters.decide(
            question.conversation,
            &job,
            "call-2",
            QuestionAnswer::Cancelled,
        ),
        Err(QuestionError::Invalid)
    );
    waiters
        .decide(
            question.conversation,
            &job,
            "call-1",
            QuestionAnswer::FreeText {
                text: "docs".to_owned(),
            },
        )
        .expect("answer");
    assert!(job.is_running());
    assert_eq!(
        waiters.decide(
            question.conversation,
            &job,
            "call-1",
            QuestionAnswer::Cancelled,
        ),
        Err(QuestionError::Missing)
    );
}

#[tokio::test]
async fn job_stop_does_not_complete_the_question() {
    let waiters = QuestionWaiters::new();
    let job = Job::for_conversation(job_id(), conversation());
    let question = PendingQuestion {
        conversation: conversation(),
        job: job.id(),
        session: session(),
        tool_call: "call-1".to_owned(),
        prompt: "Need a path?".to_owned(),
        options: Vec::new(),
        allow_free_text: true,
    };
    waiters.submit(question.clone()).expect("submit");
    job.request_cancel();
    assert_eq!(
        waiters.wait(question.conversation, &job, "call-1").await,
        Err(QuestionError::JobCancelled)
    );
    assert!(
        waiters
            .pending_for(question.conversation, job.id())
            .is_none()
    );
}
