use super::*;

#[tokio::test]
async fn raw_file_commands_preserve_bytes_and_bound_output_and_time() {
    let output = capture(
        GuestExec::shell("cat; printf '\\377'")
            .in_dir("/tmp")
            .with_stdin(b"input".to_vec()),
        None,
        Duration::from_secs(5),
        64,
    )
    .await
    .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"input\xff");
    let overflow = capture(
        GuestExec::shell("head -c 1024 /dev/zero").in_dir("/tmp"),
        None,
        Duration::from_secs(5),
        64,
    )
    .await
    .unwrap_err();
    assert_eq!(
        overflow.result.termination,
        CommandTermination::ResourceLimit
    );
    let timed_out = capture(
        GuestExec::shell("sleep 10").in_dir("/tmp"),
        None,
        Duration::from_millis(25),
        64,
    )
    .await
    .unwrap_err();
    assert_eq!(timed_out.result.termination, CommandTermination::TimedOut);
}

#[tokio::test]
async fn cancelled_file_commands_do_not_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let job = Job::for_conversation(
        crate::sessions::JobId::generate().unwrap(),
        crate::conversations::ConversationId::generate().unwrap(),
    );
    job.request_cancel();
    let cancelled = capture(
        GuestExec::shell("touch marker").in_dir(root.path().to_string_lossy()),
        Some(&job),
        Duration::from_secs(5),
        64,
    )
    .await
    .unwrap_err();
    assert_eq!(
        cancelled.result.termination,
        CommandTermination::NotDispatched
    );
    assert!(!root.path().join("marker").exists());
}
