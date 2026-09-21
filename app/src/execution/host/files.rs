use std::{process::Stdio, time::Duration};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

#[cfg(test)]
mod tests;

use crate::{
    execution::{CommandFailure, CommandResult, CommandTermination},
    sandbox::GuestExec,
    sessions::Job,
};

// Only server-defined file commands use this path. Model shell commands require the host_run approval gate.
// Raw bytes preserve binary rejection and exact-match edits before redaction at the model boundary.
pub(crate) async fn capture(
    request: GuestExec,
    job: Option<&Job>,
    timeout: Duration,
    maximum: usize,
) -> Result<std::process::Output, CommandFailure> {
    if job.is_some_and(Job::cancel_requested) {
        return Err(failure(CommandTermination::NotDispatched, "Stopped."));
    }
    let mut command = tokio::process::Command::new(&request.program);
    command
        .args(&request.args)
        .current_dir(&request.cwd)
        .env_clear()
        .envs(super::sanitised_environment())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|_| {
        failure(
            CommandTermination::NotDispatched,
            "Power Plant cannot start the file command.",
        )
    })?;
    let _group = super::ProcessGroup(child.id());
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout pipe");
    let stderr = child.stderr.take().expect("stderr pipe");
    let result = tokio::select! {
        biased;
        _ = async {
            match job {
                Some(job) => job.cancelled().await,
                None => std::future::pending().await,
            }
        } => Err(failure(CommandTermination::Cancelled, "Stopped.")),
        _ = tokio::time::sleep(timeout) => Err(failure(
            CommandTermination::TimedOut, "The file command exceeded the time limit.",
        )),
        result = async {
            let (stdout, stderr, (), status) = tokio::try_join!(
                read_bounded(stdout, maximum),
                read_bounded(stderr, maximum),
                async {
                    if let Some(bytes) = request.stdin {
                        stdin.as_mut().expect("stdin pipe").write_all(&bytes).await
                            .map_err(|_| failure(CommandTermination::Unknown, "Power Plant cannot send the file contents."))?;
                    }
                    drop(stdin);
                    Ok(())
                },
                async {
                    child.wait().await.map_err(|_| failure(
                        CommandTermination::Unknown, "Power Plant lost the file command result.",
                    ))
                },
            )?;
            Ok(std::process::Output { status, stdout, stderr })
        } => result,
    };
    if result.is_err() {
        super::terminate(&mut child);
        let _ = child.wait().await;
    }
    result
}

async fn read_bounded(
    reader: impl AsyncRead + Unpin,
    maximum: usize,
) -> Result<Vec<u8>, CommandFailure> {
    let mut bytes = Vec::new();
    reader
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| {
            failure(
                CommandTermination::Unknown,
                "Power Plant cannot read the file command output.",
            )
        })?;
    if bytes.len() > maximum {
        return Err(failure(
            CommandTermination::ResourceLimit,
            "The file command exceeded the output limit.",
        ));
    }
    Ok(bytes)
}

fn failure(termination: CommandTermination, message: &'static str) -> CommandFailure {
    CommandFailure::new(CommandResult::new(Vec::new(), termination), message)
}
