impl super::GuestExec {
    fn display(&self) -> String {
        let mut line = self.program.clone();
        for arg in &self.args {
            line.push(' ');
            line.push_str(arg);
        }
        line
    }
}

impl ScriptedGuest {
    pub(super) fn start(
        &self,
        spec: SandboxSpec,
        missing: Option<MissingRuntime>,
    ) -> Result<(), SandboxError> {
        if spec.mounts.is_empty() {
            return Err(SandboxError::NeedProject);
        }
        if *lock_mutex(&self.status) == GuestStatus::Running && self.live.overlay() == Overlay::Idle
        {
            return Ok(());
        }
        if self.live.overlay() == Overlay::Starting {
            return Ok(());
        }
        let _guard = self.lock.try_lock().map_err(|_| SandboxError::Active)?;
        *lock_mutex(&self.live.spec) = Some(spec);
        *lock_mutex(&self.start_count) += 1;
        self.live.begin_start(missing).map(|_| ())
    }

    pub(super) fn exec(
        &self,
        request: GuestExec,
        missing: Option<MissingRuntime>,
    ) -> Result<CommandSession, SandboxError> {
        if let Some(missing) = missing {
            return Err(SandboxError::Missing(missing));
        }
        if self.live.overlay() == Overlay::Starting {
            return Err(SandboxError::Busy);
        }
        if lock_mutex(&self.live.spec).is_none() {
            return Err(SandboxError::NeedProject);
        }
        if *lock_mutex(&self.status) != GuestStatus::Running {
            return Err(SandboxError::NotRunning);
        }
        let lifecycle = self
            .lock
            .clone()
            .try_lock_owned()
            .map_err(|_| SandboxError::Active)?;
        *lock_mutex(&self.last_exec) = Some(request.display());
        lock_mutex(&self.exec_log).push(request.clone());
        let session = if *lock_mutex(&self.hang_command) {
            *lock_mutex(&self.hang_command) = false;
            command::ScriptedCommand::hang()
        } else if *lock_mutex(&self.fail_command) {
            *lock_mutex(&self.fail_command) = false;
            command::ScriptedCommand::output(request.display(), 1)
        } else if request.args.iter().any(|arg| arg == "skill-list") {
            command::ScriptedCommand::output(String::new(), 3)
        } else {
            command::ScriptedCommand::output(request.display(), 0)
        };
        Ok(CommandSession::scripted(session).with_lifecycle(lifecycle))
    }

    pub(super) fn stop(&self, missing: Option<MissingRuntime>) -> Result<(), SandboxError> {
        if let Some(missing) = missing {
            return Err(SandboxError::Missing(missing));
        }
        if self.live.overlay() == Overlay::Starting {
            return Err(SandboxError::Busy);
        }
        let _guard = self.lock.try_lock().map_err(|_| SandboxError::Active)?;
        *lock_mutex(&self.status) = GuestStatus::Stopped;
        Ok(())
    }

    pub(super) fn remove(&self, missing: Option<MissingRuntime>) -> Result<(), SandboxError> {
        if *lock_mutex(&self.fail_remove) {
            *lock_mutex(&self.fail_remove) = false;
            return Err(SandboxError::Remove);
        }
        self.stop(missing)
    }
}

use super::*;

impl super::SandboxFleet {
    pub(crate) fn scripted() -> Self {
        Self {
            runtime: Arc::new(RuntimePrep {
                missing: Mutex::new(None),
            }),
            attempt_handles: Mutex::new(HashMap::new()),
            orphans: Mutex::new(Vec::new()),
            scripted: true,
            hang_command: Mutex::new(false),
        }
    }

    pub(crate) fn set_missing_runtime(&self, missing: MissingRuntime) {
        *lock_mutex(&self.runtime.missing) = Some(missing);
    }

    pub(crate) fn guest_named(&self, attempt: AttemptId) -> bool {
        lock_mutex(&self.attempt_handles).contains_key(&attempt)
    }
}

impl super::GuestSandbox {
    pub(crate) fn scripted() -> Self {
        SandboxFleet::scripted().new_handle(
            RunId::generate().expect("run"),
            AttemptId::generate().expect("attempt"),
        )
    }

    pub(crate) fn fail_next_command(&self) {
        match &self.inner {
            Inner::Microsandbox(_) => {}
            Inner::Scripted(guest) => *lock_mutex(&guest.fail_command) = true,
        }
    }
    pub(crate) fn start_count(&self) -> usize {
        match &self.inner {
            Inner::Microsandbox(_) => 0,
            Inner::Scripted(guest) => *lock_mutex(&guest.start_count),
        }
    }
    pub(crate) fn last_exec(&self) -> Option<String> {
        match &self.inner {
            Inner::Microsandbox(_) => None,
            Inner::Scripted(guest) => lock_mutex(&guest.last_exec).clone(),
        }
    }
    pub(crate) fn exec_log(&self) -> Vec<GuestExec> {
        match &self.inner {
            Inner::Microsandbox(_) => Vec::new(),
            Inner::Scripted(guest) => lock_mutex(&guest.exec_log).clone(),
        }
    }
    pub(crate) fn fail_next_stop(&self) {
        match &self.inner {
            Inner::Microsandbox(_) => {}
            Inner::Scripted(guest) => *lock_mutex(&guest.fail_stop) = true,
        }
    }
    pub(crate) fn fail_next_remove(&self) {
        match &self.inner {
            Inner::Microsandbox(_) => {}
            Inner::Scripted(guest) => *lock_mutex(&guest.fail_remove) = true,
        }
    }
}

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{
    GuestSandbox, MountSpec, SANDBOX_OWNER_LABEL, SANDBOX_OWNER_VALUE, SandboxError, SandboxSpec,
    owns_sandbox_owner,
};
use crate::agents::NetworkAccess;

fn spec(dir: &Path) -> SandboxSpec {
    SandboxSpec {
        mounts: vec![MountSpec {
            guest: "/project".to_owned(),
            host: dir.canonicalize().expect("canonical"),
            read_only: false,
        }],
        workdir: "/project".to_owned(),
        network: NetworkAccess::None,
    }
}

#[test]
fn sandbox_ownership_requires_the_owner_label() {
    let mut labels = BTreeMap::new();
    assert!(!owns_sandbox_owner(&labels));
    labels.insert(SANDBOX_OWNER_LABEL.to_owned(), "another-owner".to_owned());
    assert!(!owns_sandbox_owner(&labels));
    labels.insert(
        SANDBOX_OWNER_LABEL.to_owned(),
        SANDBOX_OWNER_VALUE.to_owned(),
    );
    assert!(owns_sandbox_owner(&labels));
}

#[tokio::test]
async fn start_from_snapshot_is_rejected_without_mounts() {
    let sandbox = GuestSandbox::scripted();
    let spec = SandboxSpec {
        mounts: Vec::new(),
        workdir: "/project".to_owned(),
        network: NetworkAccess::None,
    };
    assert!(matches!(
        sandbox
            .start_from_snapshot(Path::new("snapshot"), "sha256:deadbeef", spec)
            .await,
        Err(SandboxError::NeedProject)
    ));
}

#[tokio::test]
async fn a_failed_command_records_the_guest_program() {
    let sandbox = GuestSandbox::scripted();
    let dir = tempfile::tempdir().expect("project");
    sandbox
        .start_from_snapshot(Path::new("snapshot"), "sha256:deadbeef", spec(dir.path()))
        .await
        .expect("start");
    sandbox.fail_next_command();
    let mut session = sandbox
        .exec_cmd(super::GuestExec::command(
            "git",
            vec!["status".to_owned(), "--porcelain=v1".to_owned()],
        ))
        .await
        .expect("exec");
    assert_eq!(
        sandbox.last_exec().as_deref(),
        Some("git status --porcelain=v1")
    );
    assert_eq!(sandbox.exec_log().len(), 1);
    let mut code = None;
    while let Some(event) = session.recv().await {
        if let super::CommandEvent::Exited(value) = event {
            code = Some(value);
        }
    }
    session.close().await;
    assert_eq!(code, Some(1));
}

#[tokio::test]
async fn equal_snapshot_starts_never_reuse_an_attempt_guest() {
    let fleet = super::SandboxFleet::scripted();
    let run = crate::workflows::RunId::generate().expect("run");
    let attempt = crate::workflows::AttemptId::generate().expect("attempt");
    let sandbox = fleet.attempt_handle(run, attempt);
    let dir = tempfile::tempdir().expect("workspace");
    let spec = spec(dir.path());

    sandbox
        .start_from_snapshot(Path::new("snapshot"), "sha256:deadbeef", spec.clone())
        .await
        .expect("start");
    sandbox
        .start_from_snapshot(Path::new("snapshot"), "sha256:deadbeef", spec)
        .await
        .expect("restart");

    assert_eq!(sandbox.start_count(), 2);
}

#[test]
fn writable_user_project_mounts_are_rejected() {
    let project = tempfile::tempdir().expect("project");
    let spec = spec(project.path());
    assert!(matches!(
        super::confirm_host_write_access(
            &spec,
            &spec.mounts[0].host,
            crate::agents::AccessMode::ReadOnly
        ),
        Err(SandboxError::UserProjectWrite)
    ));
    let commit = SandboxSpec {
        mounts: vec![
            MountSpec {
                guest: "/project".to_owned(),
                host: spec.mounts[0].host.clone(),
                read_only: false,
            },
            MountSpec {
                guest: "/project/.git".to_owned(),
                host: spec.mounts[0].host.join(".git"),
                read_only: false,
            },
        ],
        workdir: "/project".to_owned(),
        network: NetworkAccess::None,
    };
    assert!(matches!(
        super::confirm_host_write_access(
            &commit,
            &spec.mounts[0].host,
            crate::agents::AccessMode::ReadOnly
        ),
        Err(SandboxError::UserProjectWrite)
    ));
}

#[test]
fn stale_mounts_are_rejected() {
    let sandbox_spec = SandboxSpec {
        mounts: vec![MountSpec {
            guest: "/project".to_owned(),
            host: PathBuf::from("/no/such/frinkworks-mount"),
            read_only: false,
        }],
        workdir: "/project".to_owned(),
        network: NetworkAccess::None,
    };
    assert!(matches!(
        super::confirm_mounts(&sandbox_spec),
        Err(SandboxError::DirectoryMissing)
    ));
}
