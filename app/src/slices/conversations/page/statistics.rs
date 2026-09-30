use askama::Template;

use crate::conversations::{ConversationId, statistics::TokenTotal};
use crate::sessions::{JobSnapshot, JobStatus};
use crate::state::AppState;

use super::{ContextView, context_view, format_count, format_usd};

#[derive(Template)]
#[template(path = "conversations/templates/statistics.html")]
pub(in crate::slices::conversations) struct StatisticsView {
    available: bool,
    metrics: Vec<Metric>,
    context: String,
    context_note: &'static str,
    turn_label: &'static str,
    work_key: String,
    turn_ms: u64,
    total_ms: u64,
    turn_time: String,
    total_time: String,
    running: bool,
    paused: bool,
    time_available: bool,
}

struct Metric {
    label: &'static str,
    icon: &'static str,
    value: String,
    note: &'static str,
}

impl StatisticsView {
    pub(in crate::slices::conversations) fn load(
        state: &AppState,
        id: &ConversationId,
        job: Option<&JobSnapshot>,
    ) -> Self {
        let totals = state
            .conversations
            .usage_totals(id, job.map_or(&[], |job| &job.output.usage));
        let time = state
            .conversations
            .work_time(id, job.and_then(|job| job.work_ms.map(|ms| (job.id, ms))));
        let context = ContextView::from_job(job).or_else(|| {
            state
                .conversations
                .context_record(id)
                .and_then(|record| context_view(&record, &state.models_dev, None))
        });
        let mut metrics = Vec::new();
        if let Ok(totals) = &totals {
            for (label, icon, value, note) in [
                (
                    "Input tokens",
                    "tokens-input",
                    totals.input,
                    "Cumulative input across recorded requests. Cached input is included.",
                ),
                (
                    "Output tokens",
                    "tokens-output",
                    totals.output,
                    "Cumulative output across recorded requests.",
                ),
                (
                    "Cache read",
                    "database",
                    totals.cache_read,
                    "Cached input tokens. These tokens are a subset of input.",
                ),
                (
                    "Total tokens",
                    "tokens-total",
                    totals.tokens,
                    "Input plus output across recorded requests, including summaries and other branches. This is not context occupancy.",
                ),
            ] {
                metrics.push(Metric {
                    label,
                    icon,
                    value: count(value, totals.requests),
                    note,
                });
            }
            let hit_rate = if !totals.input.incomplete
                && !totals.cache_read.incomplete
                && totals.input.known > 0
                && totals.cache_read.reported
            {
                percentage(totals.cache_read.known, totals.input.known)
            } else {
                "—".to_owned()
            };
            metrics.push(Metric { label: "Cache hit", icon: "cache-hit", value: hit_rate,
                note: "Total cached read tokens divided by total input tokens. Unknown reports leave the percentage unavailable." });
            metrics.push(Metric {
                icon: "cost",
                label: if totals.cost.incomplete {
                    "Known cost"
                } else if totals.cost.estimated {
                    "Est. cost"
                } else {
                    "Cost"
                },
                value: totals.cost.known_micros.map(|micros| {
                    format!(
                        "{}{}{}",
                        if totals.cost.incomplete { "≥" } else { "" },
                        if totals.cost.estimated { "≈" } else { "" },
                        format_usd(micros)
                    )
                }).unwrap_or_else(|| {
                    if totals.requests == 0 {
                        "$0.00".to_owned()
                    } else {
                        "—".to_owned()
                    }
                }),
                note: if totals.requests == 0 {
                    "No recorded requests."
                } else if totals.cost.incomplete {
                    "Incomplete cost in USD. Some requests have no reported cost or estimate."
                } else if totals.cost.estimated {
                    "Cost in USD. Includes catalogue estimates when the provider reports no usable cost."
                } else {
                    "Provider-reported cost in USD."
                },
            });
        }
        let (context, context_note) = context.map_or(("Unknown".to_owned(), "Context capacity is unavailable."), |context| {
            let prefix = if context.approximate || context.partial { "≈" } else { "" };
            let tokens = format_count(context.input_tokens);
            let label = match context.capacity {
                Some(capacity) if capacity > 0 => format!("{prefix}{tokens} / {} · {}", format_count(capacity), percentage(context.input_tokens, capacity)),
                _ => format!("{prefix}{tokens} · capacity unknown"),
            };
            let note = if context.partial {
                "Estimated next-request input on the active branch. Runtime instructions and tools are excluded. Cumulative usage is separate."
            } else {
                "Input for the current request as a percentage of model context capacity. Cumulative usage and reserved output are separate."
            };
            (label, note)
        });
        let time_available = time.as_ref().is_ok_and(|time| {
            time.recorded
                || state
                    .conversations
                    .metadata_for(id)
                    .is_some_and(|record| record.active_leaf.is_none())
        });
        let time = time.unwrap_or_default();
        let timed_job = job.filter(|job| job.work_ms.is_some());
        Self {
            available: totals.is_ok(),
            metrics,
            context,
            context_note,
            turn_label: if timed_job.is_some() {
                "Turn"
            } else {
                "Last turn"
            },
            work_key: timed_job.map_or_else(|| id.as_hex(), |job| job.id.as_hex()),
            turn_ms: time.turn_ms,
            total_ms: time.total_ms,
            turn_time: duration(time.turn_ms),
            total_time: duration(time.total_ms),
            running: timed_job.is_some_and(|job| job.status == JobStatus::Running),
            paused: timed_job.is_some_and(|job| {
                matches!(
                    job.status,
                    JobStatus::AwaitingDecision | JobStatus::AwaitingQuestion
                )
            }),
            time_available,
        }
    }
}

fn count(total: TokenTotal, requests: usize) -> String {
    if requests == 0 {
        return "0".to_owned();
    }
    if !total.reported {
        return "—".to_owned();
    }
    format!(
        "{}{}",
        if total.incomplete { "≥" } else { "" },
        format_count(total.known)
    )
}

fn percentage(numerator: u64, denominator: u64) -> String {
    let tenths = u128::from(numerator) * 1000 / u128::from(denominator);
    format!("{}.{:01}%", tenths / 10, tenths % 10)
}

fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    let hours = seconds / 3600;
    if hours > 0 {
        format!("{hours}:{:02}:{:02}", seconds / 60 % 60, seconds % 60)
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}
