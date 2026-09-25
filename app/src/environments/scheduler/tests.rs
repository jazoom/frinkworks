impl ScriptedRuntime {
    pub(super) fn create_guest(&self, spec: &PreparationGuestSpec) -> Result<(), FailureCategory> {
        let _ = spec;
        self.phase(PreparationPhase::CreatingGuest)
    }

    pub(super) fn run_setup(
        &self,
        script: &str,
        logger: &mut BoundedLogger,
    ) -> Result<(), FailureCategory> {
        let mut inner = self.inner.lock().expect("lock");
        inner.last_exec = Some(setup_exec(script));
        drop(inner);
        let _ = logger.append(b"setup output\n");
        self.phase(PreparationPhase::RunningSetup)
    }

    pub(super) fn create_snapshot(
        &self,
        id: PreparationId,
    ) -> Result<PreparedSnapshot, FailureCategory> {
        self.phase(PreparationPhase::CreatingSnapshot)?;
        Ok(crate::tests::sample_snapshot(id))
    }

    pub(super) fn mark_available(
        &self,
        snapshots: &EnvironmentSnapshotRepository,
        snapshot: &PreparedSnapshot,
    ) {
        snapshots.mark(
            snapshot.artifact_key.clone(),
            crate::environments::snapshot::SnapshotAvailability::Available,
        );
    }

    pub(super) fn phase(&self, phase: PreparationPhase) -> Result<(), FailureCategory> {
        let inner = self.inner.lock().expect("lock");
        if inner.fail_at == Some(phase) {
            return Err(match phase {
                PreparationPhase::CreatingGuest => FailureCategory::GuestCreate,
                PreparationPhase::RunningSetup => FailureCategory::SetupExit,
                PreparationPhase::StoppingGuest => FailureCategory::GuestStop,
                PreparationPhase::CreatingSnapshot => FailureCategory::SnapshotCreate,
                PreparationPhase::VerifyingSnapshot => FailureCategory::SnapshotIntegrity,
                PreparationPhase::RemovingGuest => FailureCategory::GuestRemove,
                _ => FailureCategory::GuestCreate,
            });
        }
        Ok(())
    }
}

use super::*;

impl super::EnvironmentPreparationScheduler {
    pub(crate) fn idle(
        catalogue: Arc<EnvironmentCatalogue>,
        snapshots: Arc<EnvironmentSnapshotRepository>,
    ) -> Arc<Self> {
        Arc::new(Self {
            catalogue,
            snapshots,
            notify: Arc::new(Notify::new()),
            stop: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "dev")]
            executing: AtomicBool::new(false),
            runtime: PreparationRuntime::Scripted(ScriptedRuntime {
                inner: Arc::new(std::sync::Mutex::new(ScriptedInner {
                    fail_at: None,
                    last_exec: None,
                })),
            }),
        })
    }
    pub(crate) fn fail_at(&self, phase: PreparationPhase) {
        if let PreparationRuntime::Scripted(runtime) = &self.runtime {
            runtime.inner.lock().expect("lock").fail_at = Some(phase);
        }
    }
    pub(crate) fn fail_snapshot_removal(&self) {
        self.snapshots.fail_removal();
    }
    pub(crate) fn last_exec(&self) -> Option<GuestExec> {
        match &self.runtime {
            PreparationRuntime::Microsandbox => None,
            PreparationRuntime::Scripted(runtime) => {
                runtime.inner.lock().expect("lock").last_exec.clone()
            }
        }
    }
}

use std::sync::Arc;

use super::super::preparation::{FailureCategory, PreparationPhase, PreparationState};
use super::{EnvironmentPreparationScheduler, preparation_guest_spec, setup_exec};
use crate::environments::catalogue::EnvironmentCatalogue;
use crate::environments::recipe::EnvironmentDraft;
use crate::environments::snapshot::EnvironmentSnapshotRepository;

fn draft(name: &str, script: &str) -> EnvironmentDraft {
    EnvironmentDraft {
        name: name.to_owned(),
        oci_image: "alpine/git".to_owned(),
        setup_script: script.to_owned(),
    }
}

fn scheduler() -> (
    Arc<EnvironmentCatalogue>,
    Arc<EnvironmentPreparationScheduler>,
) {
    let catalogue = Arc::new(EnvironmentCatalogue::in_memory());
    let snapshots = Arc::new(EnvironmentSnapshotRepository::in_memory());
    let scheduler = EnvironmentPreparationScheduler::idle(catalogue.clone(), snapshots);
    (catalogue, scheduler)
}

#[tokio::test]
async fn a_successful_preparation_becomes_ready() {
    let (catalogue, scheduler) = scheduler();
    let (record, preparation) = catalogue.create(draft("Alpine Git", "")).expect("create");
    assert!(scheduler.prepare_next().await);
    let ready = catalogue.preparation(&preparation.id).expect("prep");
    assert_eq!(ready.state, PreparationState::Ready);
    assert_eq!(
        catalogue.get(&record.id).expect("env").ready_preparation,
        Some(preparation.id)
    );
}

#[tokio::test]
async fn setup_failure_keeps_a_prior_ready_pointer() {
    let (catalogue, scheduler) = scheduler();
    let (record, first) = catalogue.create(draft("Alpine Git", "")).expect("create");
    assert!(scheduler.prepare_next().await);
    let updated = catalogue
        .update(
            &record.id,
            catalogue.get(&record.id).expect("env").revision,
            draft("Alpine Git", "false\n"),
        )
        .expect("replace");
    let replacement = updated.preparation.expect("queued");
    scheduler.fail_at(PreparationPhase::RunningSetup);
    assert!(scheduler.prepare_next().await);
    let failed = catalogue.preparation(&replacement.id).expect("failed");
    assert_eq!(failed.state, PreparationState::Failed);
    assert_eq!(
        failed.failure.expect("failure").category,
        FailureCategory::SetupExit
    );
    assert_eq!(
        catalogue.get(&record.id).expect("env").ready_preparation,
        Some(first.id)
    );
}

#[tokio::test]
async fn deletion_wakes_the_active_preparation_waiter() {
    let (catalogue, scheduler) = scheduler();
    let (record, _) = catalogue.create(draft("Alpine Git", "")).expect("create");
    let environment_id = record.id;
    let revision = record.revision;
    let waiter = {
        let scheduler = scheduler.clone();
        tokio::spawn(async move {
            scheduler
                .wait_for_environment_deletion(&environment_id)
                .await;
        })
    };
    tokio::task::yield_now().await;

    catalogue.delete(&environment_id, revision).expect("delete");
    scheduler.wake();

    tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("wake")
        .expect("waiter");
}

#[tokio::test]
async fn snapshot_removal_failure_owns_the_terminal_category() {
    let (catalogue, scheduler) = scheduler();
    let (_, preparation) = catalogue.create(draft("Alpine Git", "")).expect("create");
    scheduler.fail_at(PreparationPhase::VerifyingSnapshot);
    scheduler.fail_snapshot_removal();

    assert!(scheduler.prepare_next().await);

    let failed = catalogue.preparation(&preparation.id).expect("failed");
    assert_eq!(failed.state, PreparationState::Failed);
    assert_eq!(
        failed.failure.expect("failure").category,
        FailureCategory::SnapshotRemove
    );
}

#[tokio::test]
async fn stale_work_is_superseded_without_a_ready_snapshot() {
    let (catalogue, _scheduler) = scheduler();
    let (record, first) = catalogue.create(draft("One", "")).expect("create");
    catalogue
        .update(&record.id, record.revision, draft("One", "apk add curl\n"))
        .expect("replace");
    assert_eq!(
        catalogue.preparation(&first.id).expect("first").state,
        PreparationState::Superseded
    );
    assert!(
        catalogue
            .preparation(&first.id)
            .expect("first")
            .snapshot
            .is_none()
    );
}

#[tokio::test]
async fn setup_text_reaches_standard_input_not_the_argument_list() {
    let (catalogue, scheduler) = scheduler();
    catalogue
        .create(draft("Alpine Git", "apk add git\n"))
        .expect("create");
    assert!(scheduler.prepare_next().await);
    let exec = scheduler.last_exec().expect("exec");
    assert_eq!(exec.program, "/bin/sh");
    assert_eq!(exec.args, vec!["-eu".to_owned()]);
    assert_eq!(exec.stdin.as_deref(), Some(b"apk add git\n".as_slice()));
    assert!(!exec.args.iter().any(|arg| arg.contains("apk add git")));
}

#[test]
fn preparation_guests_have_no_project_mount_or_secret() {
    let spec = preparation_guest_spec(
        &crate::environments::id::EnvironmentId::generate().expect("env"),
        &crate::environments::id::PreparationId::generate().expect("prep"),
        "alpine/git",
    );
    assert_eq!(spec.mounts, 0);
    assert_eq!(spec.ports, 0);
    assert_eq!(spec.secrets, 0);
    assert_eq!(spec.workdir, "/");
    assert_eq!(spec.user, "root");
    assert!(spec.managed_root_disk);
    assert!(spec.network_public);
    assert!(!spec.network_allows_host);
    assert!(!spec.network_allows_private);
    assert!(spec.name.starts_with("frinkworks-prep-"));
    assert!(!spec.name.contains("alpine"));
}

#[test]
fn setup_exec_does_not_use_a_host_shell() {
    let exec = setup_exec("echo secret");
    assert_eq!(exec.program, "/bin/sh");
    assert_eq!(exec.args, ["-eu"]);
    assert_eq!(exec.stdin.as_deref(), Some(b"echo secret".as_slice()));
}
