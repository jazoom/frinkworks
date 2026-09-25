use std::sync::Arc;

use super::WorkflowExecution;

#[test]
fn unresolved_cleanup_blocks_new_execution() {
    let execution = Arc::new(WorkflowExecution::new());
    let first = execution.acquire().unwrap();
    first.require_recovery();
    drop(first);
    assert!(execution.acquire().is_err());
    assert!(execution.acquire_exclusive().is_err());
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
