use std::time::{Duration, Instant};

use super::{
    MAXIMUM_MODEL_REPLY_BYTES, MAXIMUM_THINKING_PROGRESS_BYTES, THINKING_INITIAL_DELAY,
    THINKING_PROGRESS_INTERVAL, ThinkingProgress, append_model_piece, publish_reply_before_tools,
    visible_tool_output,
};
use crate::{
    providers::{AssistantReply, ToolOutput},
    sessions::{Job, JobEventKind, JobId},
    workflows::RunId,
};

#[test]
fn streamed_credentials_remain_redacted_at_every_utf8_split() {
    let secret = "sk-sécret";
    let input = format!("Before {secret} after {secret}.");
    for split in input.char_indices().map(|(index, _)| index) {
        let mut redactor = super::StreamRedactor::new(Some(secret));
        let mut output = redactor.push(&input[..split]);
        assert!(!output.contains(secret));
        output.push_str(&redactor.push(&input[split..]));
        output.push_str(&redactor.finish());
        assert_eq!(output, "Before [redacted] after [redacted].");
    }
    let mut redactor = super::StreamRedactor::new(Some(secret));
    let mut output = String::new();
    for character in input.chars() {
        output.push_str(&redactor.push(&character.to_string()));
    }
    output.push_str(&redactor.finish());
    assert_eq!(output, "Before [redacted] after [redacted].");
    let mut redactor = super::StreamRedactor::new(Some(secret));
    assert_eq!(redactor.push("Before sk-"), "Before ");
    assert_eq!(redactor.finish_boundary(), "[redacted]");
    assert_eq!(redactor.push("Next block"), "Next block");
}

#[test]
fn a_large_tool_result_does_not_consume_the_model_reply_limit() {
    let mut visible_tool_bytes = 0;
    let tool = visible_tool_output(
        "read `/project/large.txt`".to_owned(),
        &"x".repeat(crate::tools::MAXIMUM_TOOL_BYTES),
        None,
        &mut visible_tool_bytes,
    )
    .expect("visible tool output");

    let mut reply = AssistantReply::default();
    let mut model_reply_bytes = 0;
    assert!(!append_model_piece(
        &mut reply,
        &"a".repeat(MAXIMUM_MODEL_REPLY_BYTES),
        &mut model_reply_bytes,
    ));
    assert!(tool.output.ends_with("[output truncated]"));
    assert_eq!(reply.text.len(), MAXIMUM_MODEL_REPLY_BYTES);
}

#[test]
fn command_status_survives_an_exhausted_display_budget() {
    use crate::execution::command::{
        CommandChunk, CommandResult, CommandStream, CommandTermination,
    };
    let mut bytes = super::MAXIMUM_VISIBLE_TOOL_BYTES;
    let tool = visible_tool_output(
        "run".to_owned(),
        "partial output",
        Some(CommandResult::new(
            vec![CommandChunk {
                stream: CommandStream::Stderr,
                text: "partial output".to_owned(),
            }],
            CommandTermination::Cancelled,
        )),
        &mut bytes,
    )
    .expect("retain the outcome even without output space");
    assert_eq!(bytes, super::MAXIMUM_VISIBLE_TOOL_BYTES);
    let mut reply = AssistantReply::default();
    reply.push_tool(tool);
    let reply = super::bound_reply(&reply);
    let command = reply.tools[0].command.as_ref().unwrap();
    assert_eq!(command.termination, CommandTermination::Cancelled);
    assert!(command.chunks.is_empty());
}

fn job() -> std::sync::Arc<Job> {
    Job::new(
        JobId::generate().expect("job id"),
        RunId::generate().expect("run id"),
        1,
    )
}

fn thinking_deltas(job: &Job) -> Vec<String> {
    job.events_after(0)
        .into_iter()
        .filter_map(|event| match event.kind {
            JobEventKind::Thinking { delta } => Some(delta),
            _ => None,
        })
        .collect()
}

#[test]
fn initial_thinking_tokens_are_coalesced_before_publication() {
    let job = job();
    let started = Instant::now();
    let mut progress = ThinkingProgress::default();
    let mut thinking = "First".to_owned();

    progress.note_pending(&thinking, started);
    assert!(!progress.publish_due(&job, &thinking, started));
    thinking.push_str(" thought");
    progress.note_pending(&thinking, started + Duration::from_millis(20));
    assert!(!progress.publish_due(
        &job,
        &thinking,
        started + THINKING_INITIAL_DELAY - Duration::from_millis(1),
    ));
    assert!(progress.publish_due(&job, &thinking, started + THINKING_INITIAL_DELAY));

    assert_eq!(thinking_deltas(&job), vec!["First thought"]);
}

#[test]
fn steady_thinking_updates_follow_the_progress_interval() {
    let job = job();
    let started = Instant::now();
    let mut progress = ThinkingProgress::default();
    let mut thinking = "First thought".to_owned();

    progress.note_pending(&thinking, started);
    let first_emit = started + THINKING_INITIAL_DELAY;
    assert!(progress.publish_due(&job, &thinking, first_emit));
    thinking.push_str(" and the next thought");
    progress.note_pending(&thinking, first_emit + Duration::from_millis(1));
    assert!(!progress.publish_due(
        &job,
        &thinking,
        first_emit + THINKING_PROGRESS_INTERVAL - Duration::from_millis(1),
    ));
    assert!(progress.publish_due(&job, &thinking, first_emit + THINKING_PROGRESS_INTERVAL,));

    assert_eq!(
        thinking_deltas(&job),
        vec!["First thought", " and the next thought"]
    );
}

#[test]
fn thinking_backlog_catches_up_in_bounded_updates() {
    let job = job();
    let started = Instant::now();
    let mut progress = ThinkingProgress::default();
    let thinking = "x".repeat(MAXIMUM_THINKING_PROGRESS_BYTES * 3 + 7);

    progress.note_pending(&thinking, started);
    let first_emit = started + THINKING_INITIAL_DELAY;
    assert!(progress.publish_due(&job, &thinking, first_emit));
    assert!(progress.published < thinking.len());
    assert!(progress.publish_due(&job, &thinking, first_emit + THINKING_PROGRESS_INTERVAL,));

    let deltas = thinking_deltas(&job);
    assert_eq!(deltas.len(), 2);
    assert!(
        deltas
            .iter()
            .all(|delta| delta.len() <= MAXIMUM_THINKING_PROGRESS_BYTES)
    );
    assert_eq!(deltas.concat(), thinking[..progress.published]);
}

#[test]
fn pending_thinking_is_published_before_a_tool() {
    let job = job();
    let started = Instant::now();
    let mut progress = ThinkingProgress::default();
    let reply = AssistantReply {
        thinking: "thought ".repeat(MAXIMUM_THINKING_PROGRESS_BYTES / 2),
        ..AssistantReply::default()
    };
    let mut published_response = 0;

    progress.note_pending(&reply.thinking, started);
    assert!(progress.publish_due(&job, &reply.thinking, started + THINKING_INITIAL_DELAY));
    publish_reply_before_tools(&job, &reply, &mut published_response, &mut progress);
    job.push_tool(ToolOutput {
        label: "read `/project/file`".to_owned(),
        output: "contents".to_owned(),
        command: None,
    });

    let events = job.events_after(0);
    let tool_index = events
        .iter()
        .position(|event| matches!(event.kind, JobEventKind::ToolFinished { .. }))
        .expect("tool event");
    assert_eq!(tool_index, events.len() - 1);
    assert!(events[..tool_index].iter().all(|event| {
        matches!(
            &event.kind,
            JobEventKind::Thinking { delta }
                if delta.len() <= MAXIMUM_THINKING_PROGRESS_BYTES
        )
    }));
    assert_eq!(thinking_deltas(&job).concat(), reply.thinking);
}

#[test]
fn model_reply_overflow_remains_an_error() {
    let mut reply = AssistantReply::default();
    let mut model_reply_bytes = 0;
    assert!(append_model_piece(
        &mut reply,
        &"a".repeat(MAXIMUM_MODEL_REPLY_BYTES + 1),
        &mut model_reply_bytes,
    ));
    assert_eq!(reply.text.len(), MAXIMUM_MODEL_REPLY_BYTES);
}
