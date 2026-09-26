use std::sync::Arc;
use std::time::Duration;

use crate::sessions::{Job, JobId};

use super::super::command::{
    CommandCapture, CommandChunk, CommandResult, CommandStream, CommandTermination,
};
use super::{COMMAND_TIMEOUT, run_shell};

fn job() -> Arc<Job> {
    Job::for_conversation(
        JobId::generate().expect("job"),
        crate::conversations::ConversationId::generate().expect("conversation"),
    )
}

#[tokio::test]
async fn host_output_is_bounded_even_when_both_pipes_produce_output() {
    let directory = tempfile::tempdir().unwrap();
    let result = run_shell(
        "head -c 262144 /dev/zero; head -c 262144 /dev/zero >&2",
        directory.path(),
        &job(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let bytes = result
        .chunks
        .iter()
        .map(|chunk| chunk.text.len())
        .sum::<usize>();
    assert!(bytes <= crate::execution::output::MAXIMUM_RETAINED_BYTES);
    assert_eq!(result.termination, CommandTermination::ResourceLimit);
    let (bounded, _) = result.bounded(crate::tools::MAXIMUM_TOOL_BYTES);
    assert!(bounded.is_bounded());
    assert!(result.report().contains("resource limit"));
}

#[tokio::test]
async fn workflow_commands_require_success_without_output_limit_termination() {
    let directory = tempfile::tempdir().unwrap();
    for command in [
        "touch changed; exit 7",
        "touch changed; head -c 262144 /dev/zero",
        "touch changed; head -c 262144 /dev/zero >&2",
    ] {
        assert!(
            super::run_workflow_shell(command, directory.path(), &job(), Duration::from_secs(5))
                .await
                .is_err()
        );
        assert!(directory.path().join("changed").exists());
        std::fs::remove_file(directory.path().join("changed")).unwrap();
    }
    let result = super::run_workflow_shell(
        "printf diagnostic",
        directory.path(),
        &job(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(result.report(), "diagnostic");
}

#[tokio::test]
async fn timeout_covers_descendant_pipes_after_the_shell_exits() {
    let directory = tempfile::tempdir().unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        run_shell(
            "sleep 30 & exit 0",
            directory.path(),
            &job(),
            Duration::from_millis(50),
        ),
    )
    .await
    .expect("descendant pipes must not bypass the deadline");
    let failure = result.unwrap_err();
    assert_eq!(failure.message, "The command exceeded the time limit.");
    assert_eq!(failure.result.termination, CommandTermination::TimedOut);
}

#[tokio::test]
async fn host_timeout_stops_descendant_side_effects() {
    let directory = tempfile::tempdir().unwrap();
    let failure = run_shell(
        "(sleep 0.2; touch escaped) & wait",
        directory.path(),
        &job(),
        Duration::from_millis(50),
    )
    .await
    .unwrap_err();
    assert_eq!(failure.message, "The command exceeded the time limit.");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!directory.path().join("escaped").exists());
}

#[tokio::test]
async fn slow_commands_publish_replayable_progress_and_retain_cancelled_output() {
    use crate::execution::output::{OutputKey, OutputScope, OutputStore};

    let directory = tempfile::tempdir().unwrap();
    let scope =
        OutputScope::conversation(crate::conversations::ConversationId::generate().unwrap());
    let store = OutputStore::open(directory.path().join("output")).unwrap();
    let job = job();
    job.start_tool("slow".to_owned(), "run".to_owned(), serde_json::Value::Null);
    let reporter = super::CommandReporter {
        tool_call: "slow",
        secret: Some("secret"),
        outputs: &store,
        output_key: OutputKey {
            scope: scope.clone(),
            job: job.id(),
            tool_call: "slow".to_owned(),
            model_hidden: false,
        },
    };
    let command = super::run_shell_reported(
        "printf 'before secret'; printf diagnostic >&2; sleep 5",
        directory.path(),
        &job,
        Duration::from_secs(10),
        false,
        &reporter,
    );
    let observe = async {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let snapshot = job.snapshot();
                if !snapshot.output.progress.is_empty() {
                    assert!(
                        !snapshot
                            .output
                            .progress
                            .iter()
                            .any(|piece| piece.text.contains("secret"))
                    );
                    assert_eq!(job.output_up_to(snapshot.latest_seq), snapshot.output);
                    break;
                }
                job.wait_after(snapshot.latest_seq, Duration::from_millis(100))
                    .await;
            }
        })
        .await
        .expect("progress before exit");
        job.request_cancel();
    };
    let (result, ()) = tokio::join!(command, observe);
    let failure = result.expect_err("cancelled command");
    assert_eq!(failure.result.termination, CommandTermination::Cancelled);
    let reopened = OutputStore::open(directory.path().join("output")).unwrap();
    let page = reopened
        .model_page(
            failure.result.retained_reference().unwrap(),
            &scope,
            crate::tools::read::parse_request(None, None).expect("page"),
        )
        .unwrap();
    assert_eq!(page.chunks, failure.result.chunks);
    assert!(!page.chunks.is_empty());
}

#[tokio::test]
async fn cancelled_jobs_never_spawn_a_command() {
    let directory = tempfile::tempdir().unwrap();
    let job = job();
    job.request_cancel();
    let failure = run_shell("touch dispatched", directory.path(), &job, COMMAND_TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(failure.message, "Stopped.");
    assert_eq!(
        failure.result.termination,
        CommandTermination::NotDispatched
    );
    assert!(!directory.path().join("dispatched").exists());
}

#[tokio::test]
async fn subprocess_environment_contains_only_allowed_variables() {
    let directory = tempfile::tempdir().unwrap();
    let result = run_shell("printenv", directory.path(), &job(), COMMAND_TIMEOUT)
        .await
        .unwrap();
    let output = result.report();
    let allowed = [
        "PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TERM", "TMPDIR", "PWD", "SHLVL", "_",
    ];
    for line in output.lines() {
        let (name, _) = line.split_once('=').unwrap();
        assert!(
            allowed.contains(&name),
            "unexpected inherited variable: {name}"
        );
    }
    assert!(output.contains("PATH="));
}

#[tokio::test]
async fn stderr_keeps_its_identity_on_a_successful_command() {
    let directory = tempfile::tempdir().unwrap();
    let result = run_shell(
        "printf out; printf err >&2",
        directory.path(),
        &job(),
        COMMAND_TIMEOUT,
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code(), Some(0));
    assert!(
        result
            .chunks
            .iter()
            .any(|chunk| chunk.stream == CommandStream::Stdout && chunk.text == "out")
    );
    assert!(
        result
            .chunks
            .iter()
            .any(|chunk| chunk.stream == CommandStream::Stderr && chunk.text == "err")
    );
    assert!(!result.is_error());
}

#[tokio::test]
async fn a_silent_non_zero_exit_reports_the_code_and_output_state() {
    let directory = tempfile::tempdir().unwrap();
    let result = run_shell("exit 9", directory.path(), &job(), COMMAND_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(result.exit_code(), Some(9));
    assert_eq!(result.report(), "The command exited with code 9.");
    assert!(result.is_error());
    assert_eq!(result.status_text(), "Exit code 9");
}

#[test]
fn capture_preserves_split_utf8_sequences() {
    let mut capture = CommandCapture::new();
    assert!(!capture.push(CommandStream::Stdout, &[0xe2]).overflow);
    assert!(!capture.push(CommandStream::Stdout, &[0x82]).overflow);
    assert!(!capture.push(CommandStream::Stdout, &[0xac]).overflow);
    let chunks = capture.finish();
    let text = chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect::<String>();
    assert_eq!(text, "€");
    assert!(!text.contains('\u{fffd}'));
}

#[test]
fn capture_stops_at_the_resource_limit_and_retains_partial_output() {
    let mut capture = CommandCapture::with_limit(8);
    assert!(!capture.push(CommandStream::Stdout, b"12345").overflow);
    assert!(capture.push(CommandStream::Stdout, b"67890").overflow);
    let chunks = capture.finish();
    let text = chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect::<String>();
    assert_eq!(text, "12345678");
}

#[test]
fn capture_flushes_invalid_tail_and_enforces_chunk_bounds() {
    let mut capture = CommandCapture::new();
    capture.push(CommandStream::Stdout, b"text\0\xe2");
    let result = capture.into_result(CommandTermination::Exited(0));
    assert_eq!(result.combined(), "text\u{fffd}\u{fffd}");
    assert!(result.is_bounded());

    let mut capture = CommandCapture::new();
    for _ in 0..super::super::command::MAXIMUM_COMMAND_CHUNKS {
        assert!(!capture.push(CommandStream::Stderr, b"x").overflow);
    }
    assert!(capture.push(CommandStream::Stdout, b"overflow").overflow);
    let result = capture.into_result(CommandTermination::Exited(0));
    assert!(result.is_bounded());
    assert_eq!(result.termination, CommandTermination::ResourceLimit);
}

#[tokio::test]
async fn cancellation_retains_captured_streams() {
    let directory = tempfile::tempdir().unwrap();
    let job = job();
    let command = run_shell(
        "printf partial; printf diagnostic >&2; sleep 30",
        directory.path(),
        &job,
        Duration::from_secs(5),
    );
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        job.request_cancel();
    };
    let (result, _) = tokio::join!(command, cancel);
    let result = result.unwrap_err().result;
    assert_eq!(result.termination, CommandTermination::Cancelled);
    assert!(
        result
            .chunks
            .iter()
            .any(|chunk| chunk.stream == CommandStream::Stdout && chunk.text == "partial")
    );
    assert!(
        result
            .chunks
            .iter()
            .any(|chunk| chunk.stream == CommandStream::Stderr && chunk.text == "diagnostic")
    );
}

#[test]
fn redaction_preserves_streams_with_interleaved_credentials() {
    let result = CommandResult::new(
        vec![
            CommandChunk {
                stream: CommandStream::Stdout,
                text: "safe sk-".to_owned(),
            },
            CommandChunk {
                stream: CommandStream::Stderr,
                text: "diagnostic".to_owned(),
            },
            CommandChunk {
                stream: CommandStream::Stdout,
                text: "secret tail".to_owned(),
            },
        ],
        CommandTermination::Exited(0),
    )
    .redacted(Some("sk-secret"));
    assert_eq!(result.chunks[0].stream, CommandStream::Stdout);
    assert_eq!(result.chunks[0].text, "safe [redacted]");
    assert_eq!(result.chunks[1].stream, CommandStream::Stderr);
    assert_eq!(result.chunks[1].text, "diagnostic");
    assert_eq!(result.chunks[2].text, "[redacted] tail");
}

#[test]
fn command_results_redact_credentials_across_chunks() {
    let secret = "sk-secret";
    let result = CommandResult::new(
        vec![
            CommandChunk {
                stream: CommandStream::Stdout,
                text: "before sk-".to_owned(),
            },
            CommandChunk {
                stream: CommandStream::Stderr,
                text: "secret after".to_owned(),
            },
        ],
        CommandTermination::Exited(0),
    );
    let redacted = result.redacted(Some(secret));
    let text = redacted
        .chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect::<String>();
    assert!(!text.contains(secret));
    assert!(text.contains("[redacted]"));
}
