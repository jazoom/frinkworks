use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::workflows::{AttemptId, RunId};

mod access;
mod command;

#[cfg(test)]
mod tests;

pub(crate) use crate::agents::GUEST_PROJECT;
pub(crate) use access::public_network_policy;
pub(crate) use command::{CommandEvent, CommandSession};

pub(crate) const SANDBOX_OWNER_LABEL: &str = "works.frinkworks.owner";
pub(crate) const SANDBOX_OWNER_VALUE: &str = "frinkworks";
pub(crate) const SANDBOX_KIND_LABEL: &str = "works.frinkworks.guest-kind";
pub(crate) const GUEST_KIND_PREPARATION: &str = "preparation";
pub(crate) const GUEST_KIND_WORKFLOW_RUN: &str = "workflow-run";
pub(crate) const GUEST_KIND_WORKFLOW_ATTEMPT: &str = "workflow-attempt";
pub(crate) const SANDBOX_ENVIRONMENT_LABEL: &str = "works.frinkworks.environment";
pub(crate) const SANDBOX_PREPARATION_LABEL: &str = "works.frinkworks.preparation";
pub(crate) const SANDBOX_RUN_LABEL: &str = "works.frinkworks.run";
pub(crate) const SANDBOX_ATTEMPT_LABEL: &str = "works.frinkworks.attempt";
pub(crate) const SANDBOX_SNAPSHOT_LABEL: &str = "works.frinkworks.snapshot";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GuestStatus {
    Running,
    Starting,
    Stopped,
    Crashed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MissingRuntime {
    Msb,
    Libkrunfw,
    Both,
}

impl MissingRuntime {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Msb => {
                "Frinkworks cannot find the microsandbox program (`msb`). Install the microsandbox runtime, then start Frinkworks again."
            }
            Self::Libkrunfw => {
                "Frinkworks cannot find the microsandbox library (`libkrunfw`). Install the microsandbox runtime, then start Frinkworks again."
            }
            Self::Both => {
                "Frinkworks cannot find the microsandbox runtime (`msb` and `libkrunfw`). Install the microsandbox runtime, then start Frinkworks again."
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MountSpec {
    pub(crate) guest: String,
    pub(crate) host: PathBuf,
    pub(crate) read_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SandboxSpec {
    pub(crate) mounts: Vec<MountSpec>,
    pub(crate) workdir: String,
    pub(crate) network: crate::agents::NetworkAccess,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OrphanSandbox {
    pub(crate) name: String,
}

#[derive(Default)]
pub(crate) struct TransientGuestRecovery {
    pub(crate) inventory_complete: bool,
    pub(crate) attempts_remaining: BTreeSet<AttemptId>,
    pub(crate) runs_remaining: BTreeSet<RunId>,
}

#[derive(Debug)]
pub(crate) enum SandboxError {
    Missing(MissingRuntime),
    Busy,
    Start,
    Inspect,
    Ownership,
    NeedProject,
    DirectoryMissing,
    NotADirectory,
    DirectoryAccess,
    StaleMount,
    Active,
    NotRunning,
    Exec,
    Stop,
    Remove,
    UserProjectWrite,
}

impl SandboxError {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::Missing(missing) => missing.message(),
            Self::Busy => "Wait until the sandbox finishes starting.",
            Self::Start => "Frinkworks could not start the sandbox. Try again.",
            Self::Inspect => "Frinkworks could not read the sandbox status. Try again.",
            Self::Ownership => {
                "Frinkworks cannot use the sandbox name because another sandbox owns it."
            }
            Self::NeedProject => "Choose a project directory.",
            Self::DirectoryMissing => "That directory does not exist.",
            Self::NotADirectory => "That path is not a directory.",
            Self::DirectoryAccess => "Frinkworks cannot access that directory.",
            Self::StaleMount => "A granted directory is no longer at the saved path.",
            Self::Active => "Wait until the running command finishes.",
            Self::NotRunning => "Start the sandbox.",
            Self::Exec => "Frinkworks could not run the command. Try again.",
            Self::Stop => "Frinkworks could not stop the sandbox. Try again.",
            Self::Remove => "Frinkworks could not remove the sandbox. Try again.",
            Self::UserProjectWrite => "The host directory lacks Direct write authority.",
        }
    }
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for SandboxError {}

struct RuntimePrep {
    missing: Mutex<Option<MissingRuntime>>,
}

struct Live {
    overlay: Mutex<Overlay>,
    progress: Mutex<String>,
    last_error: Mutex<Option<&'static str>>,
    spec: Mutex<Option<SandboxSpec>>,
    notify: Notify,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Overlay {
    Idle,
    Starting,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GuestExec {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) stdin: Option<Vec<u8>>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) cwd: String,
}

impl GuestExec {
    pub(crate) fn shell(command: &str) -> Self {
        Self {
            program: "sh".to_owned(),
            args: vec!["-c".to_owned(), command.to_owned()],
            stdin: None,
            env: Vec::new(),
            cwd: GUEST_PROJECT.to_owned(),
        }
    }

    pub(crate) fn command(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            stdin: None,
            env: Vec::new(),
            cwd: GUEST_PROJECT.to_owned(),
        }
    }

    pub(crate) fn with_stdin(mut self, stdin: Vec<u8>) -> Self {
        self.stdin = Some(stdin);
        self
    }

    pub(crate) fn in_dir(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = cwd.into();
        self
    }

    pub(crate) fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }
}

pub(crate) struct SandboxFleet {
    runtime: Arc<RuntimePrep>,
    attempt_handles: Mutex<HashMap<AttemptId, Arc<GuestSandbox>>>,
    orphans: Mutex<Vec<OrphanSandbox>>,
    scripted: bool,
    #[cfg(test)]
    hang_command: Mutex<bool>,
}

pub(crate) struct GuestSandbox {
    run_id: RunId,
    attempt_id: AttemptId,
    name: String,
    runtime: Arc<RuntimePrep>,
    inner: Inner,
    snapshot_digest: Mutex<Option<String>>,
}

enum Inner {
    Microsandbox(MicrosandboxGuest),
    #[cfg(test)]
    Scripted(ScriptedGuest),
}

struct MicrosandboxGuest {
    live: Arc<Live>,
    lock: Arc<AsyncMutex<()>>,
}

#[cfg(test)]
struct ScriptedGuest {
    live: Arc<Live>,
    status: Mutex<GuestStatus>,
    hang_command: Mutex<bool>,
    fail_command: Mutex<bool>,
    fail_stop: Mutex<bool>,
    fail_remove: Mutex<bool>,
    start_count: Mutex<usize>,
    last_exec: Mutex<Option<String>>,
    exec_log: Mutex<Vec<GuestExec>>,
    lock: Arc<AsyncMutex<()>>,
}

impl Live {
    fn new() -> Self {
        Self {
            overlay: Mutex::new(Overlay::Idle),
            progress: Mutex::new(String::new()),
            last_error: Mutex::new(None),
            spec: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    fn overlay(&self) -> Overlay {
        *lock_mutex(&self.overlay)
    }

    fn begin_start(&self, missing: Option<MissingRuntime>) -> Result<bool, SandboxError> {
        if let Some(missing) = missing {
            return Err(SandboxError::Missing(missing));
        }
        let mut overlay = lock_mutex(&self.overlay);
        if *overlay == Overlay::Starting {
            return Ok(false);
        }
        *overlay = Overlay::Starting;
        drop(overlay);
        *lock_mutex(&self.progress) = "Starting the virtual machine".to_owned();
        *lock_mutex(&self.last_error) = None;
        self.notify.notify_waiters();
        Ok(true)
    }

    fn finish_start(&self, result: Result<(), SandboxError>) {
        *lock_mutex(&self.overlay) = Overlay::Idle;
        *lock_mutex(&self.progress) = String::new();
        match result {
            Ok(()) => *lock_mutex(&self.last_error) = None,
            Err(SandboxError::Missing(missing)) => {
                *lock_mutex(&self.last_error) = Some(missing.message());
            }
            Err(error) => *lock_mutex(&self.last_error) = Some(error.message()),
        }
        self.notify.notify_waiters();
    }

    fn set_progress(&self, progress: String) {
        *lock_mutex(&self.progress) = progress;
        self.notify.notify_waiters();
    }
}

impl SandboxFleet {
    pub(crate) async fn prepare() -> Self {
        if !microsandbox::setup::is_installed() {
            tracing::info!("installing microsandbox runtime");
            if microsandbox::setup::install().await.is_err() {
                tracing::error!(
                    operation = "install microsandbox runtime",
                    "operational request failure"
                );
            }
        }
        let runtime = Arc::new(RuntimePrep {
            missing: Mutex::new(inspect_runtime()),
        });
        Self {
            runtime,
            attempt_handles: Mutex::new(HashMap::new()),
            orphans: Mutex::new(Vec::new()),
            scripted: false,
            #[cfg(test)]
            hang_command: Mutex::new(false),
        }
    }

    pub(crate) fn missing(&self) -> Option<MissingRuntime> {
        *lock_mutex(&self.runtime.missing)
    }

    pub(crate) fn missing_message(&self) -> &'static str {
        self.missing()
            .map(MissingRuntime::message)
            .unwrap_or_default()
    }

    pub(crate) fn attempt_handle(&self, run: RunId, attempt: AttemptId) -> Arc<GuestSandbox> {
        let mut handles = lock_mutex(&self.attempt_handles);
        handles
            .entry(attempt)
            .or_insert_with(|| Arc::new(self.new_handle(run, attempt)))
            .clone()
    }

    pub(crate) fn drop_attempt(&self, attempt: AttemptId) {
        lock_mutex(&self.attempt_handles).remove(&attempt);
    }

    pub(crate) fn expose_orphan(&self, name: String) {
        let mut orphans = lock_mutex(&self.orphans);
        if !orphans.iter().any(|orphan| orphan.name == name) {
            orphans.push(OrphanSandbox { name });
        }
    }

    pub(crate) fn orphans(&self) -> Vec<OrphanSandbox> {
        lock_mutex(&self.orphans).clone()
    }

    pub(crate) async fn recover_transient_guests(&self) -> TransientGuestRecovery {
        let mut recovery = TransientGuestRecovery::default();
        let Ok(handles) = list_owned().await else {
            return recovery;
        };
        recovery.inventory_complete = true;
        for handle in handles {
            let Ok(config) = handle.config() else {
                continue;
            };
            let kind = config
                .spec
                .labels
                .get(SANDBOX_KIND_LABEL)
                .map(String::as_str);
            let attempt = config
                .spec
                .labels
                .get(SANDBOX_ATTEMPT_LABEL)
                .and_then(|value| AttemptId::parse(value));
            let run = config
                .spec
                .labels
                .get(SANDBOX_RUN_LABEL)
                .and_then(|value| RunId::parse(value));
            if !matches!(
                kind,
                Some(GUEST_KIND_PREPARATION)
                    | Some(GUEST_KIND_WORKFLOW_RUN)
                    | Some(GUEST_KIND_WORKFLOW_ATTEMPT)
            ) {
                self.expose_orphan(handle.name().to_owned());
                continue;
            }
            let name = handle.name().to_owned();
            if remove_named(&name).await.is_err() {
                if kind != Some(GUEST_KIND_PREPARATION) {
                    self.expose_orphan(name);
                }
                if let Some(attempt) = attempt {
                    recovery.attempts_remaining.insert(attempt);
                }
                if let Some(run) = run {
                    recovery.runs_remaining.insert(run);
                }
            }
        }
        recovery
    }

    pub(crate) async fn remove_orphan(&self, name: &str) -> Result<(), SandboxError> {
        let allowed = lock_mutex(&self.orphans)
            .iter()
            .any(|orphan| orphan.name == name);
        if !allowed {
            return Err(SandboxError::Ownership);
        }
        remove_named(name).await?;
        lock_mutex(&self.orphans).retain(|orphan| orphan.name != name);
        Ok(())
    }

    fn new_handle(&self, run: RunId, attempt: AttemptId) -> GuestSandbox {
        let name = format!("frinkworks-attempt-{}", attempt.as_hex());
        let live = Arc::new(Live::new());
        let inner = if self.scripted {
            #[cfg(test)]
            {
                Inner::Scripted(ScriptedGuest {
                    live,
                    status: Mutex::new(GuestStatus::Stopped),
                    hang_command: Mutex::new(*lock_mutex(&self.hang_command)),
                    fail_command: Mutex::new(false),
                    fail_stop: Mutex::new(false),
                    fail_remove: Mutex::new(false),
                    start_count: Mutex::new(0),
                    last_exec: Mutex::new(None),
                    exec_log: Mutex::new(Vec::new()),
                    lock: Arc::new(AsyncMutex::new(())),
                })
            }
            #[cfg(not(test))]
            {
                Inner::Microsandbox(MicrosandboxGuest {
                    live,
                    lock: Arc::new(AsyncMutex::new(())),
                })
            }
        } else {
            Inner::Microsandbox(MicrosandboxGuest {
                live,
                lock: Arc::new(AsyncMutex::new(())),
            })
        };
        GuestSandbox {
            run_id: run,
            attempt_id: attempt,
            name,
            runtime: self.runtime.clone(),
            inner,
            snapshot_digest: Mutex::new(None),
        }
    }
}

impl GuestSandbox {
    pub(crate) fn missing(&self) -> Option<MissingRuntime> {
        *lock_mutex(&self.runtime.missing)
    }

    pub(crate) async fn start_from_snapshot(
        &self,
        artifact: &std::path::Path,
        digest: &str,
        spec: SandboxSpec,
    ) -> Result<(), SandboxError> {
        confirm_mounts(&spec)?;
        let _ = self.remove().await;
        match &self.inner {
            Inner::Microsandbox(guest) => {
                if let Some(missing) = self.missing() {
                    return Err(SandboxError::Missing(missing));
                }
                if !guest.live.begin_start(None)? {
                    return Ok(());
                }
                let live = guest.live.clone();
                let name = self.name.clone();
                let id = self.attempt_id;
                let run_id = self.run_id;
                let artifact = artifact.to_path_buf();
                let result =
                    create_detached(&live, run_id, id, &name, &spec, &artifact, digest).await;
                if result.is_ok() {
                    *lock_mutex(&self.snapshot_digest) = Some(digest.to_owned());
                }
                match &result {
                    Ok(()) => live.finish_start(Ok(())),
                    Err(SandboxError::Missing(missing)) => {
                        live.finish_start(Err(SandboxError::Missing(*missing)));
                    }
                    Err(_) => live.finish_start(Err(SandboxError::Start)),
                }
                result
            }
            #[cfg(test)]
            Inner::Scripted(guest) => {
                guest.start(spec, self.missing())?;
                *lock_mutex(&guest.status) = GuestStatus::Running;
                guest.live.finish_start(Ok(()));
                *lock_mutex(&self.snapshot_digest) = Some(digest.to_owned());
                Ok(())
            }
        }
    }

    pub(crate) async fn exec_cmd(
        &self,
        request: GuestExec,
    ) -> Result<CommandSession, SandboxError> {
        match &self.inner {
            Inner::Microsandbox(guest) => {
                guest
                    .exec(&self.name, self.attempt_id, request, self.missing())
                    .await
            }
            #[cfg(test)]
            Inner::Scripted(guest) => guest.exec(request, self.missing()),
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) async fn stop(&self) -> Result<(), SandboxError> {
        match &self.inner {
            Inner::Microsandbox(guest) => {
                guest
                    .stop(&self.name, self.attempt_id, self.missing())
                    .await
            }
            #[cfg(test)]
            Inner::Scripted(guest) => {
                if *lock_mutex(&guest.fail_stop) {
                    *lock_mutex(&guest.fail_stop) = false;
                    return Err(SandboxError::Stop);
                }
                guest.stop(self.missing())
            }
        }
    }

    pub(crate) async fn remove(&self) -> Result<(), SandboxError> {
        match &self.inner {
            Inner::Microsandbox(guest) => {
                guest
                    .remove(&self.name, self.attempt_id, self.missing())
                    .await
            }
            #[cfg(test)]
            Inner::Scripted(guest) => guest.remove(self.missing()),
        }
    }
}

impl MicrosandboxGuest {
    async fn exec(
        &self,
        name: &str,
        kind: AttemptId,
        request: GuestExec,
        missing: Option<MissingRuntime>,
    ) -> Result<CommandSession, SandboxError> {
        if let Some(missing) = missing {
            return Err(SandboxError::Missing(missing));
        }
        if self.live.overlay() == Overlay::Starting {
            return Err(SandboxError::Busy);
        }
        let lifecycle = self
            .lock
            .clone()
            .try_lock_owned()
            .map_err(|_| SandboxError::Active)?;
        Ok(exec_command(name, kind, request)
            .await?
            .with_lifecycle(lifecycle))
    }

    async fn stop(
        &self,
        name: &str,
        kind: AttemptId,
        missing: Option<MissingRuntime>,
    ) -> Result<(), SandboxError> {
        if let Some(missing) = missing {
            return Err(SandboxError::Missing(missing));
        }
        if self.live.overlay() == Overlay::Starting {
            return Err(SandboxError::Busy);
        }
        let _guard = self.lock.try_lock().map_err(|_| SandboxError::Active)?;
        stop_owned(name, kind).await
    }

    async fn remove(
        &self,
        name: &str,
        kind: AttemptId,
        missing: Option<MissingRuntime>,
    ) -> Result<(), SandboxError> {
        if let Some(missing) = missing {
            return Err(SandboxError::Missing(missing));
        }
        if self.live.overlay() == Overlay::Starting {
            return Err(SandboxError::Busy);
        }
        let _guard = self.lock.try_lock().map_err(|_| SandboxError::Active)?;
        remove_owned(name, kind).await
    }
}

pub(crate) fn confirm_host_write_access(
    spec: &SandboxSpec,
    host_root: &Path,
    access: crate::agents::AccessMode,
) -> Result<(), SandboxError> {
    for mount in &spec.mounts {
        if !mount.read_only && mount.host == host_root && !access.is_writable() {
            return Err(SandboxError::UserProjectWrite);
        }
    }
    Ok(())
}

fn confirm_mounts(spec: &SandboxSpec) -> Result<(), SandboxError> {
    if spec.mounts.is_empty() {
        return Err(SandboxError::NeedProject);
    }
    for mount in &spec.mounts {
        let resolved = resolve_dir(&mount.host)?;
        if resolved != mount.host {
            return Err(SandboxError::StaleMount);
        }
    }
    Ok(())
}

fn resolve_dir(path: &Path) -> Result<PathBuf, SandboxError> {
    let metadata = std::fs::metadata(path).map_err(map_fs_error)?;
    if !metadata.is_dir() {
        return Err(SandboxError::NotADirectory);
    }
    std::fs::canonicalize(path).map_err(map_fs_error)
}

fn map_fs_error(error: std::io::Error) -> SandboxError {
    match error.kind() {
        std::io::ErrorKind::NotFound => SandboxError::DirectoryMissing,
        std::io::ErrorKind::NotADirectory => SandboxError::NotADirectory,
        _ => SandboxError::DirectoryAccess,
    }
}

async fn current_status(name: &str, kind: AttemptId) -> Result<GuestStatus, SandboxError> {
    match microsandbox::Sandbox::get(name).await {
        Ok(handle) => {
            ensure_owned(&handle, kind)?;
            Ok(map_status(handle.status_snapshot()))
        }
        Err(microsandbox::MicrosandboxError::SandboxNotFound(_)) => Ok(GuestStatus::Stopped),
        Err(error) => Err(map_error(error, SandboxError::Inspect)),
    }
}

async fn create_detached(
    live: &Live,
    run_id: RunId,
    attempt_id: AttemptId,
    name: &str,
    spec: &SandboxSpec,
    snapshot: &Path,
    digest: &str,
) -> Result<(), SandboxError> {
    *lock_mutex(&live.spec) = Some(spec.clone());
    let mut builder = microsandbox::Sandbox::builder(name)
        .from_snapshot(snapshot.to_string_lossy().into_owned())
        .label(SANDBOX_SNAPSHOT_LABEL, digest);
    builder = apply_owner_labels(builder, run_id, attempt_id)
        .workdir(&spec.workdir)
        .network(|network| network.policy(access::network_policy(&spec.network)))
        .detached(true);
    for mount in &spec.mounts {
        let guest = mount.guest.clone();
        let host_path = mount.host.clone();
        let read_only = mount.read_only;
        builder = builder.volume(guest, move |volume| {
            let bound = volume.bind(host_path);
            if read_only { bound.readonly() } else { bound }
        });
    }
    let (mut progress, task) = match builder.create_detached_with_pull_progress() {
        Ok(started) => started,
        Err(error) => return Err(map_error(error, SandboxError::Start)),
    };
    let mut layers = 0usize;
    while let Some(event) = progress.recv().await {
        live.set_progress(pull_copy(&event, &mut layers));
    }
    match task.await {
        Ok(Ok(sandbox)) => {
            sandbox.detach().await;
            Ok(())
        }
        Ok(Err(error)) => Err(map_error(error, SandboxError::Start)),
        Err(_) => Err(SandboxError::Start),
    }
}

fn apply_owner_labels(
    builder: microsandbox::sandbox::SandboxBuilder,
    run_id: RunId,
    attempt_id: AttemptId,
) -> microsandbox::sandbox::SandboxBuilder {
    builder
        .label(SANDBOX_OWNER_LABEL, SANDBOX_OWNER_VALUE)
        .label(SANDBOX_KIND_LABEL, GUEST_KIND_WORKFLOW_ATTEMPT)
        .label(SANDBOX_RUN_LABEL, run_id.as_hex())
        .label(SANDBOX_ATTEMPT_LABEL, attempt_id.as_hex())
}

async fn exec_command(
    name: &str,
    kind: AttemptId,
    request: GuestExec,
) -> Result<CommandSession, SandboxError> {
    let handle = match microsandbox::Sandbox::get(name).await {
        Ok(handle) => handle,
        Err(microsandbox::MicrosandboxError::SandboxNotFound(_)) => {
            return Err(SandboxError::NotRunning);
        }
        Err(error) => return Err(map_error(error, SandboxError::Exec)),
    };
    ensure_owned(&handle, kind)?;
    if map_status(handle.status_snapshot()) != GuestStatus::Running {
        return Err(SandboxError::NotRunning);
    }
    let sandbox = handle
        .connect()
        .await
        .map_err(|error| map_error(error, SandboxError::Exec))?;
    let cwd = request.cwd;
    let program = request.program;
    let args = request.args;
    let stdin = request.stdin;
    let env = request.env;
    match sandbox
        .exec_stream_with(program, |options| {
            let options = options.args(args).cwd(cwd).envs(env);
            match stdin {
                Some(bytes) => options.stdin_bytes(bytes),
                None => options,
            }
        })
        .await
    {
        Ok(exec) => Ok(CommandSession::microsandbox(sandbox, exec)),
        Err(error) => {
            sandbox.detach().await;
            Err(command::map_exec_error(error))
        }
    }
}

async fn stop_owned(name: &str, kind: AttemptId) -> Result<(), SandboxError> {
    match microsandbox::Sandbox::get(name).await {
        Ok(handle) => {
            ensure_owned(&handle, kind)?;
            if matches!(
                map_status(handle.status_snapshot()),
                GuestStatus::Running | GuestStatus::Starting
            ) {
                handle
                    .stop()
                    .await
                    .map_err(|error| map_error(error, SandboxError::Stop))?;
            }
            match current_status(name, kind).await {
                Ok(GuestStatus::Stopped) | Ok(GuestStatus::Crashed) => Ok(()),
                Ok(_) => Err(SandboxError::Stop),
                Err(_) => Err(SandboxError::Stop),
            }
        }
        Err(microsandbox::MicrosandboxError::SandboxNotFound(_)) => Ok(()),
        Err(error) => Err(map_error(error, SandboxError::Stop)),
    }
}

async fn remove_owned(name: &str, kind: AttemptId) -> Result<(), SandboxError> {
    match microsandbox::Sandbox::get(name).await {
        Ok(handle) => {
            ensure_owned(&handle, kind)?;
            if matches!(
                map_status(handle.status_snapshot()),
                GuestStatus::Running | GuestStatus::Starting
            ) {
                handle
                    .stop()
                    .await
                    .map_err(|error| map_error(error, SandboxError::Remove))?;
            }
            match handle.remove().await {
                Ok(()) | Err(microsandbox::MicrosandboxError::SandboxNotFound(_)) => Ok(()),
                Err(error) => Err(map_error(error, SandboxError::Remove)),
            }
        }
        Err(microsandbox::MicrosandboxError::SandboxNotFound(_)) => Ok(()),
        Err(error) => Err(map_error(error, SandboxError::Remove)),
    }
}

async fn remove_named(name: &str) -> Result<(), SandboxError> {
    match microsandbox::Sandbox::get(name).await {
        Ok(handle) => {
            if !owns_owner(&handle) {
                return Err(SandboxError::Ownership);
            }
            if matches!(
                map_status(handle.status_snapshot()),
                GuestStatus::Running | GuestStatus::Starting
            ) {
                handle
                    .stop()
                    .await
                    .map_err(|error| map_error(error, SandboxError::Remove))?;
            }
            match handle.remove().await {
                Ok(()) | Err(microsandbox::MicrosandboxError::SandboxNotFound(_)) => Ok(()),
                Err(error) => Err(map_error(error, SandboxError::Remove)),
            }
        }
        Err(microsandbox::MicrosandboxError::SandboxNotFound(_)) => Ok(()),
        Err(error) => Err(map_error(error, SandboxError::Remove)),
    }
}

async fn list_owned() -> Result<Vec<microsandbox::sandbox::SandboxHandle>, SandboxError> {
    let mut cursor = None;
    let mut sandboxes = Vec::new();
    loop {
        let page = microsandbox::Sandbox::list_with(|list| {
            let list = list
                .label(SANDBOX_OWNER_LABEL, SANDBOX_OWNER_VALUE)
                .limit(100);
            match cursor.clone() {
                Some(value) => list.cursor(value),
                None => list,
            }
        })
        .await
        .map_err(|error| map_error(error, SandboxError::Inspect))?;
        sandboxes.extend(page.sandboxes);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Ok(sandboxes)
}

fn map_status(status: microsandbox::sandbox::SandboxStatus) -> GuestStatus {
    match status {
        microsandbox::sandbox::SandboxStatus::Running
        | microsandbox::sandbox::SandboxStatus::Draining => GuestStatus::Running,
        microsandbox::sandbox::SandboxStatus::Starting => GuestStatus::Starting,
        microsandbox::sandbox::SandboxStatus::Crashed => GuestStatus::Crashed,
        microsandbox::sandbox::SandboxStatus::Created
        | microsandbox::sandbox::SandboxStatus::Paused
        | microsandbox::sandbox::SandboxStatus::Stopped => GuestStatus::Stopped,
    }
}

fn ensure_owned(
    handle: &microsandbox::sandbox::SandboxHandle,
    id: AttemptId,
) -> Result<(), SandboxError> {
    let config = handle.config().map_err(|_| SandboxError::Inspect)?;
    let owned = owns_sandbox_owner(&config.spec.labels)
        && config
            .spec
            .labels
            .get(SANDBOX_KIND_LABEL)
            .map(String::as_str)
            == Some(GUEST_KIND_WORKFLOW_ATTEMPT)
        && config
            .spec
            .labels
            .get(SANDBOX_ATTEMPT_LABEL)
            .map(String::as_str)
            == Some(&id.as_hex());
    if owned {
        Ok(())
    } else {
        Err(SandboxError::Ownership)
    }
}

fn owns_owner(handle: &microsandbox::sandbox::SandboxHandle) -> bool {
    handle
        .config()
        .ok()
        .is_some_and(|config| owns_sandbox_owner(&config.spec.labels))
}

fn owns_sandbox_owner(labels: &BTreeMap<String, String>) -> bool {
    labels.get(SANDBOX_OWNER_LABEL).map(String::as_str) == Some(SANDBOX_OWNER_VALUE)
}

pub(crate) fn map_error(
    error: microsandbox::MicrosandboxError,
    failed: SandboxError,
) -> SandboxError {
    match error {
        microsandbox::MicrosandboxError::LibkrunfwNotFound(_) => {
            SandboxError::Missing(MissingRuntime::Libkrunfw)
        }
        _ => failed,
    }
}

fn pull_copy(event: &microsandbox::sandbox::PullProgress, layers: &mut usize) -> String {
    use microsandbox::sandbox::PullProgress;
    match event {
        PullProgress::Resolving { .. } => "Resolving the image".to_owned(),
        PullProgress::Resolved { layer_count, .. } => {
            *layers = *layer_count;
            format!("Resolved {layer_count} layers")
        }
        PullProgress::LayerDownloadProgress {
            layer_index,
            downloaded_bytes,
            total_bytes,
            ..
        } => match total_bytes {
            Some(total) if *total > 0 => format!(
                "Downloading {}, {}%",
                layer_phrase(*layer_index, *layers),
                downloaded_bytes.saturating_mul(100) / total
            ),
            _ => format!("Downloading {}", layer_phrase(*layer_index, *layers)),
        },
        PullProgress::LayerDownloadComplete { layer_index, .. }
        | PullProgress::LayerDownloadVerifying { layer_index, .. } => {
            format!("Verifying {}", layer_phrase(*layer_index, *layers))
        }
        PullProgress::LayerMaterializeStarted { layer_index, .. }
        | PullProgress::LayerMaterializeProgress { layer_index, .. }
        | PullProgress::LayerMaterializeWriting { layer_index, .. }
        | PullProgress::LayerMaterializeComplete { layer_index, .. } => {
            format!("Writing {}", layer_phrase(*layer_index, *layers))
        }
        PullProgress::StitchMergingTrees { .. }
        | PullProgress::StitchWritingFsmeta
        | PullProgress::StitchWritingVmdk
        | PullProgress::StitchComplete => "Combining image layers".to_owned(),
        PullProgress::Complete { .. } => "Starting the virtual machine".to_owned(),
    }
}

fn layer_phrase(index: usize, layers: usize) -> String {
    let number = index + 1;
    if layers == 0 {
        format!("layer {number}")
    } else {
        format!("layer {number} of {layers}")
    }
}

pub(crate) fn inspect_runtime() -> Option<MissingRuntime> {
    if microsandbox::setup::is_installed() {
        return None;
    }
    let diagnosis = microsandbox::setup::diagnose();
    let mut msb = false;
    let mut libkrunfw = false;
    for section in diagnosis.sections {
        for check in section.checks {
            if check.state != microsandbox::setup::CheckState::Fail {
                continue;
            }
            if check.label == "msb" {
                msb = true;
            }
            if check.label == "libkrunfw" {
                libkrunfw = true;
            }
        }
    }
    match (msb, libkrunfw) {
        (true, false) => Some(MissingRuntime::Msb),
        (false, true) => Some(MissingRuntime::Libkrunfw),
        _ => Some(MissingRuntime::Both),
    }
}

pub(super) fn lock_mutex<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
