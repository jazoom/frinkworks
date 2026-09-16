use std::sync::Arc;

use super::WorkflowExecution;

#[tokio::test]
async fn file_application_excludes_another_application_but_not_model_work() {
    let execution = Arc::new(WorkflowExecution::new());
    let first = execution.acquire().expect("first conversation");
    let application = execution.lock_application().await.expect("application");
    let second = execution.acquire().expect("second conversation");
    let mut waiting = Box::pin(execution.lock_application());
    assert!(futures_util::poll!(&mut waiting).is_pending());
    drop(application);
    let application = waiting.await.expect("next application");
    let mut waiting = Box::pin(execution.lock_application());
    assert!(futures_util::poll!(&mut waiting).is_pending());
    first.require_recovery();
    drop(application);
    assert!(waiting.await.is_err());
    drop((first, second));
    assert!(execution.acquire().is_err());
    assert!(execution.acquire_exclusive().is_err());
    assert!(execution.lock_application().await.is_err());
}

#[test]
fn conversations_can_hold_shared_execution_leases() {
    let execution = Arc::new(WorkflowExecution::new());
    let first = execution.acquire().expect("first");
    let second = execution.acquire().expect("second");
    assert!(execution.acquire_exclusive().is_err());
    drop(first);
    assert!(execution.acquire_exclusive().is_err());
    drop(second);
    let exclusive = execution.acquire_exclusive().expect("exclusive");
    assert!(execution.acquire().is_err());
    drop(exclusive);
    assert!(execution.acquire().is_ok());
}
