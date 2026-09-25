use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

use crate::sessions::Job;

use super::command::{
    CommandCapture, CommandFailure, CommandResult, CommandStream, CommandTermination,
};

pub(crate) mod files;

pub(crate) const COMMAND_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(200)
} else {
    Duration::from_secs(120)
};

/// Live progress routing for one host command.
pub(crate) struct CommandReporter<'a> {
    pub(crate) tool_call: &'a str,
    pub(crate) secret: Option<&'a str>,
    pub(crate) outputs: &'a super::output::OutputStore,
    pub(crate) output_key: super::output::OutputKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostIdentity {
    pub(crate) username: String,
    pub(crate) uid: u32,
    pub(crate) euid: u32,
}

impl HostIdentity {
    pub(crate) fn current() -> Self {
        current_identity()
    }

    pub(crate) fn elevated(&self) -> bool {
        self.euid == 0 || self.euid != self.uid
    }

    pub(crate) fn authority_summary(&self) -> String {
        if self.elevated() {
            format!(
                "Commands run as {} (uid {}, effective uid {}). This process is already elevated. Frinkworks adds no privileges of its own.",
                self.username, self.uid, self.euid
            )
        } else {
            format!(
                "Commands run as {} (uid {}). They have that user's existing host authority. Frinkworks adds no privileges of its own.",
                self.username, self.uid
            )
        }
    }
}

pub(crate) fn command_directory(directories: &[super::DirectoryGrant]) -> PathBuf {
    directories
        .first()
        .map(|grant| grant.host_path.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
}

// Ask each time and Automatic (YOLO) share these bounds. Policy only changes the approval gate.
#[cfg(test)]
pub(crate) async fn run_shell(
    command: &str,
    directory: &Path,
    job: &Job,
    timeout: Duration,
) -> Result<CommandResult, CommandFailure> {
    run_shell_inner(command, directory, job, timeout, false, None).await
}

pub(crate) async fn run_shell_reported(
    command: &str,
    directory: &Path,
    job: &Job,
    timeout: Duration,
    require_success: bool,
    reporter: &CommandReporter<'_>,
) -> Result<CommandResult, CommandFailure> {
    run_shell_inner(
        command,
        directory,
        job,
        timeout,
        require_success,
        Some(reporter),
    )
    .await
}

#[cfg(test)]
pub(crate) async fn run_workflow_shell(
    command: &str,
    directory: &Path,
    job: &Job,
    timeout: Duration,
) -> Result<CommandResult, CommandFailure> {
    run_shell_inner(command, directory, job, timeout, true, None).await
}

async fn run_shell_inner(
    command: &str,
    directory: &Path,
    job: &Job,
    timeout: Duration,
    require_success: bool,
    reporter: Option<&CommandReporter<'_>>,
) -> Result<CommandResult, CommandFailure> {
    if job.cancel_requested() {
        return Err(not_dispatched("Stopped."));
    }
    if command.is_empty() || command.contains('\0') {
        return Err(not_dispatched("Enter a command."));
    }
    if !directory.is_absolute() {
        return Err(not_dispatched("The command directory is not valid."));
    }
    let metadata = match std::fs::metadata(directory) {
        Ok(metadata) => metadata,
        Err(_) => return Err(not_dispatched("The command directory is not available.")),
    };
    if !metadata.is_dir() {
        return Err(not_dispatched("The command directory is not available."));
    }
    let mut capture = match reporter {
        Some(reporter) => {
            CommandCapture::with_output(reporter.secret, reporter.outputs, &reporter.output_key)
                .map_err(|error| not_dispatched(error.message()))?
        }
        None => CommandCapture::with_secret(None),
    };
    let mut child = Command::new("/bin/sh");
    child
        .arg("-c")
        .arg(command)
        .current_dir(directory)
        .env_clear()
        .envs(sanitised_environment())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        child.process_group(0);
    }
    let mut child = match child.spawn() {
        Ok(child) => child,
        Err(_) => {
            return Err(not_dispatched(
                "Frinkworks could not start the command. Try again.",
            ));
        }
    };
    // The group outlives the shell when a descendant retains an output pipe.
    let _group = ProcessGroup(child.id());
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut stdout_buffer = [0_u8; 4096];
    let mut stderr_buffer = [0_u8; 4096];
    let deadline = tokio::time::Instant::now() + timeout;
    let mut status = None;
    let mut progress = super::command::CommandProgress::new();
    let mut progress_tick = tokio::time::interval(Duration::from_millis(100));
    progress_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if status.is_some() && stdout.is_none() && stderr.is_none() {
            break;
        }
        tokio::select! {
            biased;
            _ = job.cancelled() => {
                terminate(&mut child);
                let _ = child.wait().await;
                return Err(command_failure(
                    &mut capture,
                    CommandTermination::Cancelled,
                    "Stopped.",
                ));
            }
            _ = tokio::time::sleep_until(deadline) => {
                terminate(&mut child);
                let _ = child.wait().await;
                return Err(command_failure(
                    &mut capture,
                    CommandTermination::TimedOut,
                    "The command exceeded the time limit.",
                ));
            }
            _ = progress_tick.tick(), if reporter.is_some() => {
                progress.publish(job, reporter.expect("reporter exists").tool_call);
            }
            result = read_pipe(stdout.as_mut(), &mut stdout_buffer) => {
                let count = match result {
                    Ok(count) => count,
                    Err(message) => {
                        terminate(&mut child);
                        let _ = child.wait().await;
                        return Err(command_failure(
                            &mut capture,
                            CommandTermination::Unknown,
                            message,
                        ));
                    }
                };
                match count {
                    None => stdout = None,
                    Some(count) => {
                        let push = capture.push(CommandStream::Stdout, &stdout_buffer[..count]);
                        for chunk in push.safe {
                            progress.push(chunk.stream, chunk.text);
                        }
                        if push.overflow {
                            terminate(&mut child);
                            let _ = child.wait().await;
                            return output_limit(&mut capture, require_success);
                        }
                    }
                }
            }
            result = read_pipe(stderr.as_mut(), &mut stderr_buffer) => {
                let count = match result {
                    Ok(count) => count,
                    Err(message) => {
                        terminate(&mut child);
                        let _ = child.wait().await;
                        return Err(command_failure(
                            &mut capture,
                            CommandTermination::Unknown,
                            message,
                        ));
                    }
                };
                match count {
                    None => stderr = None,
                    Some(count) => {
                        let push = capture.push(CommandStream::Stderr, &stderr_buffer[..count]);
                        for chunk in push.safe {
                            progress.push(chunk.stream, chunk.text);
                        }
                        if push.overflow {
                            terminate(&mut child);
                            let _ = child.wait().await;
                            return output_limit(&mut capture, require_success);
                        }
                    }
                }
            }
            result = child.wait(), if status.is_none() => {
                status = Some(match result {
                    Ok(status) => status,
                    Err(_) => {
                        return Err(command_failure(
                            &mut capture,
                            CommandTermination::Unknown,
                            "Frinkworks lost the command result. Try again.",
                        ));
                    }
                });
            }
        }
    }
    let termination = match status.and_then(|status| status.code()) {
        Some(code) => CommandTermination::Exited(code),
        None => CommandTermination::Unknown,
    };
    let result = capture.into_result(termination);
    if require_success && !result.is_success() {
        return Err(CommandFailure::new(
            result,
            "The host command failed. Earlier host effects remain unchanged.",
        ));
    }
    Ok(result)
}

fn not_dispatched(message: &'static str) -> CommandFailure {
    CommandFailure::new(
        CommandResult::new(Vec::new(), CommandTermination::NotDispatched),
        message,
    )
}

fn command_failure(
    capture: &mut CommandCapture,
    termination: CommandTermination,
    message: &'static str,
) -> CommandFailure {
    CommandFailure::new(capture.finish_result(termination), message)
}

fn output_limit(
    capture: &mut CommandCapture,
    require_success: bool,
) -> Result<CommandResult, CommandFailure> {
    let result = capture.finish_result(CommandTermination::ResourceLimit);
    if require_success {
        Err(CommandFailure::new(
            result,
            "The command exceeded the output limit. Host effects can remain incomplete.",
        ))
    } else {
        Ok(result)
    }
}

pub(super) fn sanitised_environment() -> Vec<(String, String)> {
    let mut env = Vec::new();
    push_inherited(&mut env, "PATH", "/usr/local/bin:/usr/bin:/bin");
    push_inherited(&mut env, "HOME", "");
    push_inherited(&mut env, "USER", "");
    push_inherited(&mut env, "LOGNAME", "");
    push_inherited(&mut env, "LANG", "C.UTF-8");
    push_inherited(&mut env, "LC_ALL", "");
    env.push(("TERM".to_owned(), "dumb".to_owned()));
    env.push(("TMPDIR".to_owned(), "/tmp".to_owned()));
    env
}

fn push_inherited(env: &mut Vec<(String, String)>, key: &str, fallback: &str) {
    match std::env::var(key) {
        Ok(value) if !value.is_empty() => env.push((key.to_owned(), value)),
        _ if !fallback.is_empty() => env.push((key.to_owned(), fallback.to_owned())),
        _ => {}
    }
}

struct ProcessGroup(Option<u32>);

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(-(pid as i32)),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

fn terminate(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(-(pid as i32)),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    let _ = child.start_kill();
}

async fn read_pipe<R: AsyncRead + Unpin>(
    pipe: Option<&mut R>,
    buffer: &mut [u8],
) -> Result<Option<usize>, &'static str> {
    let Some(pipe) = pipe else {
        return std::future::pending().await;
    };
    let count = pipe
        .read(buffer)
        .await
        .map_err(|_| "Frinkworks lost the command result. Try again.")?;
    if count == 0 {
        return Ok(None);
    }
    Ok(Some(count))
}

#[cfg(unix)]
fn current_identity() -> HostIdentity {
    let uid = nix::unistd::getuid();
    let euid = nix::unistd::geteuid();
    let username = nix::unistd::User::from_uid(euid)
        .ok()
        .flatten()
        .map(|user| user.name)
        .unwrap_or_else(|| format!("uid {}", uid.as_raw()));
    HostIdentity {
        username,
        uid: uid.as_raw(),
        euid: euid.as_raw(),
    }
}

#[cfg(not(unix))]
fn current_identity() -> HostIdentity {
    HostIdentity {
        username: "unknown".to_owned(),
        uid: 0,
        euid: 0,
    }
}

#[cfg(test)]
mod tests;
