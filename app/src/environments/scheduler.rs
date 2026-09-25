use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

use super::catalogue::EnvironmentCatalogue;
use super::id::{EnvironmentId, PreparationId};
use super::preparation::{
    FailureCategory, PreparationLogRecord, PreparationPhase, PreparationRecord, PreparationState,
};
use super::snapshot::{EnvironmentSnapshotRepository, PreparedSnapshot, create_prepared_snapshot};
use crate::sandbox::{self, GuestExec};
use crate::storage::BoundedLogger;

const SETUP_DEADLINE: Duration = if cfg!(test) {
    Duration::from_millis(50)
} else {
    Duration::from_secs(30 * 60)
};
const LIFETIME_SECS: u64 = if cfg!(test) { 2 } else { 35 * 60 };

pub(crate) struct EnvironmentPreparationScheduler {
    catalogue: Arc<EnvironmentCatalogue>,
    snapshots: Arc<EnvironmentSnapshotRepository>,
    notify: Arc<Notify>,
    stop: Arc<AtomicBool>,
    #[cfg(feature = "dev")]
    executing: AtomicBool,
    runtime: PreparationRuntime,
}

#[derive(Clone)]
enum PreparationRuntime {
    Microsandbox,
    #[cfg(test)]
    Scripted(ScriptedRuntime),
}

#[cfg(test)]
#[derive(Clone)]
struct ScriptedRuntime {
    inner: Arc<std::sync::Mutex<ScriptedInner>>,
}

#[cfg(test)]
struct ScriptedInner {
    fail_at: Option<PreparationPhase>,
    last_exec: Option<GuestExec>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparationGuestSpec {
    pub(crate) name: String,
    pub(crate) image: String,
    pub(crate) workdir: String,
    pub(crate) user: String,
    pub(crate) mounts: usize,
    pub(crate) ports: usize,
    pub(crate) secrets: usize,
    pub(crate) network_public: bool,
    pub(crate) network_allows_host: bool,
    pub(crate) network_allows_private: bool,
    pub(crate) managed_root_disk: bool,
    pub(crate) labels: Vec<(String, String)>,
}

impl EnvironmentPreparationScheduler {
    pub(crate) fn start(
        catalogue: Arc<EnvironmentCatalogue>,
        snapshots: Arc<EnvironmentSnapshotRepository>,
    ) -> Arc<Self> {
        Self::spawn(catalogue, snapshots, PreparationRuntime::Microsandbox)
    }

    fn spawn(
        catalogue: Arc<EnvironmentCatalogue>,
        snapshots: Arc<EnvironmentSnapshotRepository>,
        runtime: PreparationRuntime,
    ) -> Arc<Self> {
        let scheduler = Arc::new(Self {
            catalogue,
            snapshots,
            notify: Arc::new(Notify::new()),
            stop: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "dev")]
            executing: AtomicBool::new(false),
            runtime,
        });
        let worker = scheduler.clone();
        tokio::spawn(async move { worker.run().await });
        scheduler
    }

    pub(crate) fn wake(&self) {
        self.notify.notify_one();
    }

    async fn wait_for_environment_deletion(&self, id: &EnvironmentId) {
        loop {
            let notified = self.notify.notified();
            if self.catalogue.get(id).is_none() {
                return;
            }
            notified.await;
        }
    }

    async fn run(&self) {
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return;
            }
            while self.prepare_next().await {}
            let notified = self.notify.notified();
            if self.stop.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    #[cfg(feature = "dev")]
    pub(crate) fn has_active_work(&self) -> bool {
        self.catalogue.has_active_preparations() || self.executing.load(Ordering::SeqCst)
    }

    pub(crate) async fn prepare_next(&self) -> bool {
        let claimed = match self.catalogue.claim_oldest_queued() {
            Ok(Some(record)) => record,
            Ok(None) => return false,
            Err(_) => return false,
        };
        // Retain the active marker through guest cleanup, after the catalogue reaches a terminal state.
        #[cfg(feature = "dev")]
        self.executing.store(true, Ordering::SeqCst);
        self.execute(claimed).await;
        #[cfg(feature = "dev")]
        self.executing.store(false, Ordering::SeqCst);
        true
    }

    async fn execute(&self, record: PreparationRecord) {
        let mut logger = match self.catalogue.open_logger(&record.id) {
            Ok(logger) => logger,
            Err(_) => {
                let _ = self.catalogue.finish_failed(
                    &record.id,
                    FailureCategory::CataloguePersist,
                    PreparationLogRecord::empty(),
                );
                return;
            }
        };
        let environment = match self.catalogue.get(&record.environment_id) {
            Some(environment) => environment,
            None => {
                self.fail(&record, FailureCategory::EnvironmentDeleted, &mut logger)
                    .await;
                return;
            }
        };
        if let Err(category) = self
            .advance(&record, PreparationPhase::CreatingGuest, &mut logger)
            .await
        {
            self.fail(&record, category, &mut logger).await;
            return;
        }
        let spec = preparation_guest_spec(
            &environment.id,
            &record.id,
            environment.recipe.oci_image.as_str(),
        );
        let sandbox_name = spec.name.clone();
        if let Err(category) = self.create_guest(&spec).await {
            self.cleanup_guest(&sandbox_name).await;
            self.fail(&record, category, &mut logger).await;
            return;
        }
        if !self.catalogue.is_current(&record) {
            self.cleanup_guest(&sandbox_name).await;
            self.supersede(&record, &mut logger).await;
            return;
        }
        if let Err(category) = self
            .advance(&record, PreparationPhase::RunningSetup, &mut logger)
            .await
        {
            self.cleanup_guest(&sandbox_name).await;
            self.fail(&record, category, &mut logger).await;
            return;
        }
        let setup_result = {
            let setup = self.run_setup(&spec, &environment.recipe.setup_script, &mut logger);
            tokio::pin!(setup);
            tokio::select! {
                result = &mut setup => Some(result),
                () = self.wait_for_environment_deletion(&record.environment_id) => None,
            }
        };
        match setup_result {
            Some(Ok(())) => {}
            Some(Err(category)) => {
                self.cleanup_guest(&sandbox_name).await;
                self.fail(&record, category, &mut logger).await;
                return;
            }
            None => {
                self.cleanup_guest(&sandbox_name).await;
                return;
            }
        }
        if !self.catalogue.is_current(&record) {
            self.cleanup_guest(&sandbox_name).await;
            self.supersede(&record, &mut logger).await;
            return;
        }
        if let Err(category) = self
            .advance(&record, PreparationPhase::StoppingGuest, &mut logger)
            .await
        {
            self.cleanup_guest(&sandbox_name).await;
            self.fail(&record, category, &mut logger).await;
            return;
        }
        if let Err(category) = self.stop_guest(&sandbox_name).await {
            self.cleanup_guest(&sandbox_name).await;
            self.fail(&record, category, &mut logger).await;
            return;
        }
        if !self.catalogue.is_current(&record) {
            self.cleanup_guest(&sandbox_name).await;
            self.supersede(&record, &mut logger).await;
            return;
        }
        if let Err(category) = self
            .advance(&record, PreparationPhase::CreatingSnapshot, &mut logger)
            .await
        {
            self.cleanup_guest(&sandbox_name).await;
            self.fail(&record, category, &mut logger).await;
            return;
        }
        if !self.catalogue.is_current(&record) {
            self.cleanup_guest(&sandbox_name).await;
            self.supersede(&record, &mut logger).await;
            return;
        }
        let snapshot = match self
            .create_snapshot(&environment.id, &record, &sandbox_name)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(category) => {
                self.cleanup_guest(&sandbox_name).await;
                self.fail(&record, category, &mut logger).await;
                return;
            }
        };
        if !self.catalogue.is_current(&record) {
            if let Err(category) = self.discard_snapshot(&snapshot).await {
                self.cleanup_guest(&sandbox_name).await;
                self.fail(&record, category, &mut logger).await;
                return;
            }
            self.cleanup_guest(&sandbox_name).await;
            self.supersede(&record, &mut logger).await;
            return;
        }
        if let Err(mut category) = self
            .advance(&record, PreparationPhase::VerifyingSnapshot, &mut logger)
            .await
        {
            if let Err(remove) = self.discard_snapshot(&snapshot).await {
                category = remove;
            }
            self.cleanup_guest(&sandbox_name).await;
            self.fail(&record, category, &mut logger).await;
            return;
        }
        if let Err(mut category) = self.verify_snapshot(&snapshot).await {
            if let Err(remove) = self.discard_snapshot(&snapshot).await {
                category = remove;
            }
            self.cleanup_guest(&sandbox_name).await;
            self.fail(&record, category, &mut logger).await;
            return;
        }
        if let Err(mut category) = self
            .advance(&record, PreparationPhase::RemovingGuest, &mut logger)
            .await
        {
            if let Err(remove) = self.discard_snapshot(&snapshot).await {
                category = remove;
            }
            self.cleanup_guest(&sandbox_name).await;
            self.fail(&record, category, &mut logger).await;
            return;
        }
        if let Err(mut category) = self.remove_guest(&sandbox_name).await {
            if let Err(remove) = self.discard_snapshot(&snapshot).await {
                category = remove;
            }
            self.fail(&record, category, &mut logger).await;
            return;
        }
        if !self.catalogue.is_current(&record) {
            if let Err(category) = self.discard_snapshot(&snapshot).await {
                self.fail(&record, category, &mut logger).await;
                return;
            }
            self.supersede(&record, &mut logger).await;
            return;
        }
        let log = PreparationLogRecord::from_state(logger.state());
        match self
            .catalogue
            .finish_ready(&record.id, snapshot.clone(), log)
        {
            Ok(finished) if finished.state == PreparationState::Ready =>
            {
                #[cfg(test)]
                if let PreparationRuntime::Scripted(runtime) = &self.runtime {
                    runtime.mark_available(&self.snapshots, &snapshot);
                }
            }
            Ok(_) => {
                let _ = self.discard_snapshot(&snapshot).await;
            }
            Err(_) => {
                let stale = !self.catalogue.is_current(&record);
                match self.discard_snapshot(&snapshot).await {
                    Err(category) => {
                        let _ = self.catalogue.finish_failed(&record.id, category, log);
                    }
                    Ok(()) if stale => {
                        let _ = self.catalogue.finish_superseded(&record.id, log);
                    }
                    Ok(()) => {
                        let _ = self.catalogue.finish_failed(
                            &record.id,
                            FailureCategory::CataloguePersist,
                            log,
                        );
                    }
                }
            }
        }
    }

    async fn advance(
        &self,
        record: &PreparationRecord,
        phase: PreparationPhase,
        logger: &mut BoundedLogger,
    ) -> Result<(), FailureCategory> {
        if logger.append(phase.log_line().as_bytes()).is_err() {
            return Err(FailureCategory::CataloguePersist);
        }
        self.catalogue.bump_refresh();
        self.catalogue
            .set_phase(
                &record.id,
                phase,
                PreparationLogRecord::from_state(logger.state()),
            )
            .map(|_| ())
            .map_err(|_| FailureCategory::CataloguePersist)
    }

    async fn fail(
        &self,
        record: &PreparationRecord,
        category: FailureCategory,
        logger: &mut BoundedLogger,
    ) {
        let log = PreparationLogRecord::from_state(logger.state());
        let _ = self.catalogue.finish_failed(&record.id, category, log);
    }

    async fn supersede(&self, record: &PreparationRecord, logger: &mut BoundedLogger) {
        let log = PreparationLogRecord::from_state(logger.state());
        let _ = self.catalogue.finish_superseded(&record.id, log);
    }

    async fn create_guest(&self, spec: &PreparationGuestSpec) -> Result<(), FailureCategory> {
        match &self.runtime {
            PreparationRuntime::Microsandbox => create_microsandbox_guest(spec).await,
            #[cfg(test)]
            PreparationRuntime::Scripted(runtime) => runtime.create_guest(spec),
        }
    }

    async fn run_setup(
        &self,
        spec: &PreparationGuestSpec,
        script: &str,
        logger: &mut BoundedLogger,
    ) -> Result<(), FailureCategory> {
        match &self.runtime {
            PreparationRuntime::Microsandbox => {
                run_microsandbox_setup(&spec.name, script, logger).await
            }
            #[cfg(test)]
            PreparationRuntime::Scripted(runtime) => runtime.run_setup(script, logger),
        }
    }

    async fn stop_guest(&self, name: &str) -> Result<(), FailureCategory> {
        match &self.runtime {
            PreparationRuntime::Microsandbox => stop_named(name).await,
            #[cfg(test)]
            PreparationRuntime::Scripted(runtime) => runtime.phase(PreparationPhase::StoppingGuest),
        }
    }

    async fn create_snapshot(
        &self,
        environment_id: &EnvironmentId,
        record: &PreparationRecord,
        sandbox_name: &str,
    ) -> Result<PreparedSnapshot, FailureCategory> {
        match &self.runtime {
            PreparationRuntime::Microsandbox => {
                create_prepared_snapshot(
                    &self.snapshots,
                    *environment_id,
                    record.id,
                    record.recipe_version,
                    sandbox_name,
                )
                .await
            }
            #[cfg(test)]
            PreparationRuntime::Scripted(runtime) => runtime.create_snapshot(record.id),
        }
    }

    async fn verify_snapshot(&self, snapshot: &PreparedSnapshot) -> Result<(), FailureCategory> {
        match &self.runtime {
            PreparationRuntime::Microsandbox => self
                .snapshots
                .verify(snapshot)
                .await
                .map_err(|_| FailureCategory::SnapshotIntegrity),
            #[cfg(test)]
            PreparationRuntime::Scripted(runtime) => {
                runtime.phase(PreparationPhase::VerifyingSnapshot)
            }
        }
    }

    async fn remove_guest(&self, name: &str) -> Result<(), FailureCategory> {
        match &self.runtime {
            PreparationRuntime::Microsandbox => remove_named(name).await,
            #[cfg(test)]
            PreparationRuntime::Scripted(runtime) => {
                let _ = name;
                runtime.phase(PreparationPhase::RemovingGuest)
            }
        }
    }

    async fn cleanup_guest(&self, name: &str) {
        let _ = self.remove_guest(name).await;
    }

    async fn discard_snapshot(&self, snapshot: &PreparedSnapshot) -> Result<(), FailureCategory> {
        self.snapshots
            .remove_unpublished(&snapshot.artifact_key)
            .await
            .map_err(|_| FailureCategory::SnapshotRemove)
    }
}

impl Drop for EnvironmentPreparationScheduler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

pub(crate) fn preparation_guest_name(id: &PreparationId) -> String {
    format!("frinkworks-prep-{}", id.as_hex())
}

pub(crate) fn preparation_guest_spec(
    environment_id: &EnvironmentId,
    preparation_id: &PreparationId,
    image: &str,
) -> PreparationGuestSpec {
    let policy = crate::sandbox::public_network_policy();
    let debug = format!("{policy:?}");
    PreparationGuestSpec {
        name: preparation_guest_name(preparation_id),
        image: image.to_owned(),
        workdir: "/".to_owned(),
        user: "root".to_owned(),
        mounts: 0,
        ports: 0,
        secrets: 0,
        network_public: debug.contains("Public"),
        network_allows_host: false,
        network_allows_private: debug.contains("Group(Private)"),
        managed_root_disk: true,
        labels: vec![
            (
                sandbox::SANDBOX_OWNER_LABEL.to_owned(),
                sandbox::SANDBOX_OWNER_VALUE.to_owned(),
            ),
            (
                sandbox::SANDBOX_KIND_LABEL.to_owned(),
                sandbox::GUEST_KIND_PREPARATION.to_owned(),
            ),
            (
                sandbox::SANDBOX_ENVIRONMENT_LABEL.to_owned(),
                environment_id.as_hex(),
            ),
            (
                sandbox::SANDBOX_PREPARATION_LABEL.to_owned(),
                preparation_id.as_hex(),
            ),
        ],
    }
}

fn setup_exec(script: &str) -> GuestExec {
    GuestExec {
        program: "/bin/sh".to_owned(),
        args: vec!["-eu".to_owned()],
        stdin: Some(script.as_bytes().to_vec()),
        env: Vec::new(),
        cwd: "/".to_owned(),
    }
}

async fn create_microsandbox_guest(spec: &PreparationGuestSpec) -> Result<(), FailureCategory> {
    if sandbox::inspect_runtime().is_some() {
        return Err(FailureCategory::RuntimeUnavailable);
    }
    let image = spec.image.clone();
    let mut builder = microsandbox::Sandbox::builder(&spec.name)
        .image_with(|image_builder| image_builder.oci(image.clone()).root_disk_with(|disk| disk))
        .user(&spec.user)
        .workdir(&spec.workdir)
        .max_duration(LIFETIME_SECS)
        .network(|network| network.policy(crate::sandbox::public_network_policy()))
        .detached(true);
    for (key, value) in &spec.labels {
        builder = builder.label(key, value);
    }
    match builder.create().await {
        Ok(sandbox) => {
            sandbox.detach().await;
            Ok(())
        }
        Err(error) => Err(map_guest_error(error, FailureCategory::GuestCreate)),
    }
}

async fn run_microsandbox_setup(
    name: &str,
    script: &str,
    logger: &mut BoundedLogger,
) -> Result<(), FailureCategory> {
    let handle = match microsandbox::Sandbox::get(name).await {
        Ok(handle) => handle,
        Err(_) => return Err(FailureCategory::GuestCreate),
    };
    let sandbox = handle
        .connect()
        .await
        .map_err(|error| map_guest_error(error, FailureCategory::GuestCreate))?;
    let request = setup_exec(script);
    let mut exec = match sandbox
        .exec_stream_with(request.program, |options| {
            options
                .args(request.args)
                .cwd(request.cwd)
                .stdin_bytes(request.stdin.unwrap_or_default())
        })
        .await
    {
        Ok(exec) => exec,
        Err(error) => {
            sandbox.detach().await;
            return Err(map_guest_error(error, FailureCategory::GuestCreate));
        }
    };
    let outcome = tokio::time::timeout(SETUP_DEADLINE, async {
        let mut code = None;
        while let Some(event) = exec.recv().await {
            match event {
                microsandbox::ExecEvent::Stdout(data) | microsandbox::ExecEvent::Stderr(data) => {
                    if logger.append(&data).is_err() {
                        return Err(FailureCategory::CataloguePersist);
                    }
                }
                microsandbox::ExecEvent::Exited { code: value } => {
                    code = Some(value);
                }
                microsandbox::ExecEvent::Failed(_) => return Err(FailureCategory::GuestCreate),
                _ => {}
            }
        }
        match code {
            Some(0) => Ok(()),
            Some(_) => Err(FailureCategory::SetupExit),
            None => Err(FailureCategory::GuestCreate),
        }
    })
    .await;
    let _ = exec.kill().await;
    sandbox.detach().await;
    match outcome {
        Ok(result) => result,
        Err(_) => Err(FailureCategory::SetupTimeout),
    }
}

async fn stop_named(name: &str) -> Result<(), FailureCategory> {
    match microsandbox::Sandbox::get(name).await {
        Ok(handle) => handle
            .stop()
            .await
            .map_err(|error| map_guest_error(error, FailureCategory::GuestStop)),
        Err(microsandbox::MicrosandboxError::SandboxNotFound(_)) => Ok(()),
        Err(error) => Err(map_guest_error(error, FailureCategory::GuestStop)),
    }
}

async fn remove_named(name: &str) -> Result<(), FailureCategory> {
    match microsandbox::Sandbox::get(name).await {
        Ok(handle) => {
            let _ = handle.stop().await;
            match handle.remove().await {
                Ok(()) | Err(microsandbox::MicrosandboxError::SandboxNotFound(_)) => Ok(()),
                Err(error) => Err(map_guest_error(error, FailureCategory::GuestRemove)),
            }
        }
        Err(microsandbox::MicrosandboxError::SandboxNotFound(_)) => Ok(()),
        Err(error) => Err(map_guest_error(error, FailureCategory::GuestRemove)),
    }
}

fn map_guest_error(
    error: microsandbox::MicrosandboxError,
    failed: FailureCategory,
) -> FailureCategory {
    match sandbox::map_error(error, crate::sandbox::SandboxError::Start) {
        crate::sandbox::SandboxError::Missing(_) => FailureCategory::RuntimeUnavailable,
        _ => failed,
    }
}

#[cfg(test)]
mod tests;
