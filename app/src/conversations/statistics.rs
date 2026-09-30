use super::RequestUsage;
use super::history::{CostCoverage, request_cost};

#[derive(Clone, Copy, Default)]
pub(crate) struct TokenTotal {
    pub(crate) known: u64,
    pub(crate) reported: bool,
    pub(crate) incomplete: bool,
}

impl TokenTotal {
    fn add(&mut self, value: Option<u64>) {
        match value.and_then(|value| self.known.checked_add(value)) {
            Some(total) => {
                self.known = total;
                self.reported = true;
            }
            None => self.incomplete = true,
        }
    }
}

pub(crate) struct UsageTotals {
    pub(crate) requests: usize,
    pub(crate) input: TokenTotal,
    pub(crate) output: TokenTotal,
    pub(crate) cache_read: TokenTotal,
    pub(crate) tokens: TokenTotal,
    pub(crate) cost: CostCoverage,
}

impl Default for UsageTotals {
    fn default() -> Self {
        Self {
            requests: 0,
            input: TokenTotal::default(),
            output: TokenTotal::default(),
            cache_read: TokenTotal::default(),
            tokens: TokenTotal::default(),
            cost: CostCoverage {
                known_micros: None,
                incomplete: false,
            },
        }
    }
}

impl UsageTotals {
    pub(crate) fn add(&mut self, request: &RequestUsage) {
        self.requests += 1;
        let usage = &request.usage;
        self.input.add(usage.input_tokens);
        self.output.add(usage.output_tokens);
        // Providers normalise cached tokens as a subset of input, not extra usage.
        self.cache_read.add(
            usage
                .cache_read_tokens
                .filter(|cache| usage.input_tokens.is_none_or(|input| *cache <= input)),
        );
        self.tokens.add(usage.input_tokens);
        self.tokens.add(usage.output_tokens);
        let part = request_cost(request);
        self.cost.incomplete |= part.incomplete;
        if let Some(value) = part.known_micros {
            if let Some(total) = self.cost.known_micros.unwrap_or(0).checked_add(value) {
                self.cost.known_micros = Some(total);
            } else {
                self.cost.incomplete = true;
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct WorkTime {
    pub(crate) recorded: bool,
    pub(crate) total_ms: u64,
    pub(crate) turn_ms: u64,
}

#[cfg(test)]
mod tests;
