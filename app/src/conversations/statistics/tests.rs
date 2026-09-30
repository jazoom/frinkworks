use super::*;
use crate::conversations::RequestId;
use crate::providers::{AuthMethod, ModelUsage, ProviderKind};

fn request() -> RequestUsage {
    let mut usage = ModelUsage::new(ProviderKind::Deepseek, "deepseek-flash");
    usage.input_tokens = Some(14_003);
    usage.output_tokens = Some(521);
    usage.cache_read_tokens = Some(13_312);
    RequestUsage {
        id: RequestId::generate().unwrap(),
        usage,
        auth: AuthMethod::ApiKey,
        prices: None,
        sources: Vec::new(),
        advertised: Vec::new(),
    }
}

#[test]
fn normalised_cache_counts_do_not_inflate_total_tokens() {
    let mut totals = UsageTotals::default();
    totals.add(&request());
    assert_eq!(totals.input.known, 14_003);
    assert_eq!(totals.cache_read.known, 13_312);
    assert_eq!(totals.tokens.known, 14_524);
    assert!(!totals.tokens.incomplete);
    assert!(totals.cost.incomplete);
    assert_eq!(totals.cost.known_micros, None);
}

#[test]
fn missing_malformed_and_overflowed_reports_stay_incomplete() {
    let mut totals = UsageTotals::default();
    totals.add(&request());
    let mut missing = request();
    missing.usage.input_tokens = None;
    missing.usage.output_tokens = None;
    missing.usage.cache_read_tokens = None;
    totals.add(&missing);
    assert_eq!(totals.tokens.known, 14_524);
    assert!(totals.tokens.incomplete && totals.input.incomplete && totals.cache_read.incomplete);

    let mut malformed = request();
    malformed.usage.cache_read_tokens = Some(20_000);
    let mut totals = UsageTotals::default();
    totals.add(&malformed);
    assert!(!totals.cache_read.reported);
    assert!(totals.cache_read.incomplete);
    let mut huge = request();
    huge.usage.input_tokens = Some(u64::MAX);
    totals.add(&huge);
    assert!(totals.input.incomplete && totals.tokens.incomplete);
}
