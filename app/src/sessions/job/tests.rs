const JOB_ID_LENGTH: usize = 32;

use super::{ConversationId, Job, JobEvent, JobEventKind, JobId, JobStatus};
use crate::providers::{AssistantActivity, ToolOutput};
use crate::workflows::RunId;
use std::sync::Arc;

fn job() -> std::sync::Arc<Job> {
    Job::new(
        JobId::generate().expect("job id"),
        RunId::generate().expect("run"),
        1,
    )
}

#[test]
fn generated_job_ids_round_trip() {
    let id = JobId::generate().expect("job id");
    let hex = id.as_hex();
    assert_eq!(hex.len(), JOB_ID_LENGTH);
    assert_eq!(JobId::parse(&hex), Some(id));
}

#[test]
fn output_events_are_monotonic_and_reconstructable() {
    let job = job();
    assert_eq!(job.push_thinking("Plan".to_owned()), Some(1));
    assert_eq!(job.push_response("Hello".to_owned()), Some(2));
    assert_eq!(
        job.push_tool(ToolOutput {
            label: "read `/project/src/lib.rs`".to_owned(),
            output: "source".to_owned(),
            command: None,
        }),
        Some(3)
    );
    assert!(job.output_up_to(0).is_empty());
    assert_eq!(job.output_up_to(1).thinking, "Plan");
    assert_eq!(job.output_up_to(2).text, "Hello");
    assert_eq!(job.output_up_to(3).tools.len(), 1);
    assert_eq!(job.events_after(0).len(), 3);
    assert_eq!(job.events_after(1).len(), 2);
    assert!(job.events_after(3).is_empty());
}

#[test]
fn thinking_after_a_tool_starts_a_new_activity_phase() {
    let job = job();
    job.push_thinking("Inspect".to_owned());
    job.push_thinking(" the project".to_owned());
    job.push_tool(ToolOutput {
        label: "read `/project/src/lib.rs`".to_owned(),
        output: "source".to_owned(),
        command: None,
    });
    job.push_thinking("Use the result".to_owned());

    let output = job.output_up_to(4);
    assert_eq!(output.activity.len(), 3);
    assert_eq!(
        output.activity[0],
        AssistantActivity::Thinking("Inspect the project".to_owned())
    );
    assert!(matches!(output.activity[1], AssistantActivity::Tool(_)));
    assert_eq!(
        output.activity[2],
        AssistantActivity::Thinking("Use the result".to_owned())
    );
}

#[test]
fn finish_is_idempotent() {
    let job = job();
    assert_eq!(job.finish(JobStatus::Completed, None), Some(1));
    assert_eq!(job.finish(JobStatus::Failed, Some("no")), None);
    assert_eq!(job.snapshot().status, JobStatus::Completed);
    assert!(job.push_response("late".to_owned()).is_none());
}

impl super::Job {
    pub(crate) fn push_tool(&self, output: ToolOutput) -> Option<u64> {
        self.finish_tool(String::new(), output)
    }

    pub(crate) fn new(id: JobId, _run_id: RunId, _assistant_index: usize) -> Arc<Self> {
        Self::for_conversation(id, ConversationId::generate().expect("conversation id"))
    }
    pub(crate) fn events_after(&self, cursor: u64) -> Vec<JobEvent> {
        self.lock()
            .events
            .iter()
            .filter(|event| event.seq > cursor)
            .cloned()
            .collect()
    }
}

#[test]
fn tool_progress_is_bounded_and_the_terminal_result_survives() {
    let job = job();
    job.start_tool(
        "call-1".to_owned(),
        "run".to_owned(),
        serde_json::Value::Null,
    );
    let piece = "x".repeat(1024);
    let mut accepted = 0;
    for _ in 0..128 {
        if job
            .push_tool_progress(
                "call-1".to_owned(),
                crate::execution::CommandStream::Stdout,
                piece.clone(),
            )
            .is_some()
        {
            accepted += 1;
        }
    }
    assert!(accepted < 128);
    job.finish_tool(
        "call-1".to_owned(),
        ToolOutput {
            label: "run".to_owned(),
            output: "done".to_owned(),
            command: None,
        },
    );
    let events = job.events_after(0);
    assert!(
        events
            .last()
            .is_some_and(|event| matches!(event.kind, JobEventKind::ToolFinished { .. }))
    );
}

#[test]
fn progress_survives_a_late_observer_without_duplication() {
    let job = job();
    job.start_tool(
        "call-1".to_owned(),
        "run".to_owned(),
        serde_json::Value::Null,
    );
    job.push_tool_progress(
        "call-1".to_owned(),
        crate::execution::CommandStream::Stderr,
        "partial".to_owned(),
    );
    let snapshot = job.output_up_to(job.latest_seq());
    assert_eq!(snapshot.progress.len(), 1);
    assert_eq!(snapshot.progress[0].text, "partial");
    let again = job.output_up_to(job.latest_seq());
    assert_eq!(again.progress.len(), 1);
    assert_eq!(again.progress[0].text, "partial");
}
