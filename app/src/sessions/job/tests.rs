const JOB_ID_LENGTH: usize = 32;

use super::{ConversationId, Job, JobEvent, JobId, JobStatus};
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

    pub(crate) fn new(id: JobId, _run_id: RunId, assistant_index: usize) -> Arc<Self> {
        Self::for_conversation(
            id,
            ConversationId::generate().expect("conversation id"),
            assistant_index,
        )
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
