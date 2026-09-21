use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

/// Ordinary work can exceed twelve model rounds. Stream, output and storage
/// bounds stay in their own collectors.
pub(crate) const MAXIMUM_MODEL_REQUESTS: u32 = 50;
pub(crate) const MAXIMUM_TOOL_DISPATCHES: u32 = 200;
pub(crate) const MAXIMUM_ELAPSED: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BudgetPolicy {
    pub(crate) model_requests: u32,
    pub(crate) tool_dispatches: u32,
    pub(crate) elapsed: Duration,
}

impl BudgetPolicy {
    pub(crate) const fn ordinary() -> Self {
        Self {
            model_requests: MAXIMUM_MODEL_REQUESTS,
            tool_dispatches: MAXIMUM_TOOL_DISPATCHES,
            elapsed: MAXIMUM_ELAPSED,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BudgetReason {
    ModelRequests,
    ToolDispatches,
    Elapsed,
}

impl BudgetReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ModelRequests => "model-requests",
            Self::ToolDispatches => "tool-dispatches",
            Self::Elapsed => "elapsed",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "model-requests" => Some(Self::ModelRequests),
            "tool-dispatches" => Some(Self::ToolDispatches),
            "elapsed" => Some(Self::Elapsed),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ModelRequests => "model request limit",
            Self::ToolDispatches => "tool dispatch limit",
            Self::Elapsed => "elapsed time limit",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BudgetSnapshot {
    pub(crate) model_requests: u32,
    pub(crate) model_request_limit: u32,
    pub(crate) tool_dispatches: u32,
    pub(crate) tool_dispatch_limit: u32,
    pub(crate) elapsed_ms: u64,
    pub(crate) elapsed_limit_ms: u64,
    pub(crate) reason: BudgetReason,
}

impl BudgetSnapshot {
    pub(crate) fn valid(self) -> bool {
        self.model_request_limit > 0
            && self.tool_dispatch_limit > 0
            && self.elapsed_limit_ms > 0
            && self.model_requests <= self.model_request_limit
            && self.tool_dispatches <= self.tool_dispatch_limit
            && match self.reason {
                BudgetReason::ModelRequests => self.model_requests >= self.model_request_limit,
                BudgetReason::ToolDispatches => self.tool_dispatches >= self.tool_dispatch_limit,
                BudgetReason::Elapsed => self.elapsed_ms >= self.elapsed_limit_ms,
            }
    }
}

pub(crate) struct Budget {
    policy: BudgetPolicy,
    started: Instant,
    model_requests: u32,
    tool_dispatches: u32,
}

impl Budget {
    pub(crate) fn start(policy: BudgetPolicy) -> Self {
        Self {
            policy,
            started: Instant::now(),
            model_requests: 0,
            tool_dispatches: 0,
        }
    }

    pub(crate) fn next_request_block(&self) -> Option<BudgetReason> {
        if self.model_requests >= self.policy.model_requests {
            Some(BudgetReason::ModelRequests)
        } else {
            self.next_tool_block()
        }
    }

    pub(crate) fn next_tool_block(&self) -> Option<BudgetReason> {
        if self.tool_dispatches >= self.policy.tool_dispatches {
            Some(BudgetReason::ToolDispatches)
        } else if self.started.elapsed() >= self.policy.elapsed {
            Some(BudgetReason::Elapsed)
        } else {
            None
        }
    }

    pub(crate) fn record_model_request(&mut self) {
        self.model_requests = self.model_requests.saturating_add(1);
    }

    pub(crate) fn record_tool_dispatches(&mut self, count: u32) {
        self.tool_dispatches = self.tool_dispatches.saturating_add(count);
    }

    pub(crate) fn snapshot(&self, reason: BudgetReason) -> BudgetSnapshot {
        BudgetSnapshot {
            model_requests: self.model_requests,
            model_request_limit: self.policy.model_requests,
            tool_dispatches: self.tool_dispatches,
            tool_dispatch_limit: self.policy.tool_dispatches,
            elapsed_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            elapsed_limit_ms: u64::try_from(self.policy.elapsed.as_millis()).unwrap_or(u64::MAX),
            reason,
        }
    }
}
