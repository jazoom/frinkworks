use std::time::Duration;

use super::{Budget, BudgetPolicy, BudgetReason};

#[test]
fn model_request_limit_blocks_the_next_provider_turn() {
    let mut budget = Budget::start(BudgetPolicy {
        model_requests: 2,
        tool_dispatches: 20,
        elapsed: Duration::from_secs(60),
    });
    budget.record_model_request();
    assert_eq!(budget.next_request_block(), None);
    budget.record_model_request();
    assert_eq!(
        budget.next_request_block(),
        Some(BudgetReason::ModelRequests)
    );
    let snapshot = budget.snapshot(BudgetReason::ModelRequests);
    assert!(snapshot.valid());
    assert_eq!(budget.next_tool_block(), None);
    assert_eq!(snapshot.model_requests, 2);
    assert_eq!(snapshot.model_request_limit, 2);
}

#[test]
fn tool_dispatch_limit_blocks_the_next_provider_turn() {
    let mut budget = Budget::start(BudgetPolicy {
        model_requests: 20,
        tool_dispatches: 3,
        elapsed: Duration::from_secs(60),
    });
    budget.record_model_request();
    budget.record_tool_dispatches(3);
    assert_eq!(
        budget.next_request_block(),
        Some(BudgetReason::ToolDispatches)
    );
    assert!(budget.snapshot(BudgetReason::ToolDispatches).valid());
}

#[test]
fn elapsed_limit_blocks_the_next_provider_turn() {
    let mut budget = Budget::start(BudgetPolicy {
        model_requests: 20,
        tool_dispatches: 20,
        elapsed: Duration::from_millis(1),
    });
    budget.record_model_request();
    std::thread::sleep(Duration::from_millis(2));
    assert_eq!(budget.next_request_block(), Some(BudgetReason::Elapsed));
    assert!(budget.snapshot(BudgetReason::Elapsed).valid());
}

#[test]
fn a_snapshot_rejects_an_unknown_reason_pairing() {
    let snapshot = super::BudgetSnapshot {
        model_requests: 1,
        model_request_limit: 20,
        tool_dispatches: 0,
        tool_dispatch_limit: 20,
        elapsed_ms: 10,
        elapsed_limit_ms: 60_000,
        reason: BudgetReason::ModelRequests,
    };
    assert!(!snapshot.valid());
    assert_eq!(
        BudgetReason::parse("model-requests"),
        Some(BudgetReason::ModelRequests)
    );
    assert_eq!(BudgetReason::parse("unknown"), None);
}
