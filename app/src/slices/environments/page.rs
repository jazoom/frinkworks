use askama::Template;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::environments::{
    EnvironmentCatalogue, EnvironmentRecord, PreparationRecord, PreparationState, PreparedSnapshot,
    RefreshCursor, SnapshotAvailability,
};

use super::forms::{EnvironmentFormState, FormErrors};

pub(super) const INDEX_TITLE: &str = "Environments | Frinkworks";
pub(super) const NEW_TITLE: &str = "New environment | Frinkworks";
pub(super) const CONFIG_TITLE: &str = "Configure environment | Frinkworks";
const HISTORY_LIMIT: usize = 20;

pub(super) struct CatalogueItem {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) image: String,
    pub(super) readiness: &'static str,
    pub(super) readiness_tone: &'static str,
    pub(super) prepared: String,
}

#[derive(Template)]
#[template(path = "environments/templates/index.html")]
pub(super) struct CatalogueView {
    pub(super) environments: Vec<CatalogueItem>,
}

impl CatalogueView {
    pub(super) async fn from_records(
        records: &[EnvironmentRecord],
        catalogue: &EnvironmentCatalogue,
        snapshots: &crate::environments::EnvironmentSnapshotRepository,
    ) -> Self {
        let mut environments = Vec::new();
        for record in records {
            let latest = catalogue.preparation(&record.latest_preparation);
            let ready = record
                .ready_preparation
                .and_then(|id| catalogue.preparation(&id));
            let availability = match ready.as_ref().and_then(|record| record.snapshot.as_ref()) {
                Some(snapshot) => snapshots.inspect(snapshot).await,
                None => SnapshotAvailability::Missing,
            };
            let readiness = readiness_label(record, latest.as_ref(), availability);
            environments.push(CatalogueItem {
                id: record.id.as_hex(),
                name: record.name.clone(),
                image: record.recipe.oci_image.as_str().to_owned(),
                readiness: readiness.label,
                readiness_tone: readiness.tone,
                prepared: latest
                    .as_ref()
                    .map(|record| format_time(record.requested_at_ms))
                    .unwrap_or_else(|| "unknown".to_owned()),
            });
        }
        Self { environments }
    }
}

pub(super) struct HistoryRow {
    pub(super) ordinal: u64,
    pub(super) version: String,
    pub(super) status: &'static str,
    pub(super) timestamp: String,
    pub(super) digest: String,
}

pub(super) struct SnapshotView {
    pub(super) digest: String,
    pub(super) image: String,
    pub(super) integrity: String,
    pub(super) size: u64,
}

#[derive(Template)]
#[template(path = "environments/templates/form.html")]
pub(super) struct EnvironmentFormView {
    pub(super) title: &'static str,
    pub(super) action: String,
    pub(super) submit: &'static str,
    pub(super) name: String,
    pub(super) name_error: &'static str,
    pub(super) image: String,
    pub(super) image_error: &'static str,
    pub(super) script: String,
    pub(super) script_error: &'static str,
    pub(super) revision: String,
    pub(super) recipe_version: String,
    pub(super) summary_error: &'static str,
    pub(super) environment_id: String,
    pub(super) show_status: bool,
    pub(super) show_delete: bool,
    pub(super) delete_error: &'static str,
    pub(super) affected_workflows: Vec<AffectedWorkflow>,
    pub(super) status_html: String,
}

pub(super) struct AffectedWorkflow {
    pub(super) name: String,
    pub(super) href: String,
}

#[derive(Template)]
#[template(path = "environments/templates/form.html", block = "environment_form")]
pub(super) struct EnvironmentFormContents<'a> {
    pub(super) action: &'a str,
    pub(super) submit: &'static str,
    pub(super) name: &'a str,
    pub(super) name_error: &'static str,
    pub(super) image: &'a str,
    pub(super) image_error: &'static str,
    pub(super) script: &'a str,
    pub(super) script_error: &'static str,
    pub(super) revision: &'a str,
    pub(super) recipe_version: &'a str,
    pub(super) summary_error: &'static str,
    pub(super) environment_id: &'a str,
    pub(super) show_delete: bool,
    pub(super) delete_error: &'static str,
    pub(super) affected_workflows: &'a [AffectedWorkflow],
}

#[derive(Template)]
#[template(path = "environments/templates/status.html")]
pub(super) struct EnvironmentStatusView {
    pub(super) environment_id: String,
    pub(super) cursor: String,
    pub(super) active: bool,
    pub(super) readiness: &'static str,
    pub(super) readiness_tone: &'static str,
    pub(super) phase: &'static str,
    pub(super) requested: String,
    pub(super) started: String,
    pub(super) finished: String,
    pub(super) failure: &'static str,
    pub(super) log_tail: String,
    pub(super) log_truncated: bool,
    pub(super) can_retry: bool,
    pub(super) revision: String,
    pub(super) recipe_version: String,
    pub(super) snapshot: Option<SnapshotView>,
    pub(super) history: Vec<HistoryRow>,
    pub(super) omitted: usize,
}

struct Readiness {
    label: &'static str,
    tone: &'static str,
}

impl EnvironmentFormView {
    pub(super) fn create(state: EnvironmentFormState, errors: FormErrors) -> Self {
        Self::from_state(
            "New environment",
            "/environments",
            "Create and prepare",
            state,
            errors,
            "",
            "",
            "",
            false,
            false,
            "",
            String::new(),
        )
    }

    pub(super) fn edit(
        record: &EnvironmentRecord,
        state: EnvironmentFormState,
        errors: FormErrors,
        delete_error: &'static str,
        status: &EnvironmentStatusView,
    ) -> Result<Self, askama::Error> {
        Ok(Self::from_state(
            "Configure environment",
            &format!("/environments/{}/configuration", record.id.as_hex()),
            "Save environment",
            state,
            errors,
            &record.id.as_hex(),
            &record.revision.to_string(),
            &record.recipe_version.as_hex(),
            true,
            true,
            delete_error,
            status.render()?,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn from_state(
        title: &'static str,
        action: &str,
        submit: &'static str,
        state: EnvironmentFormState,
        errors: FormErrors,
        environment_id: &str,
        revision: &str,
        recipe_version: &str,
        show_status: bool,
        show_delete: bool,
        delete_error: &'static str,
        status_html: String,
    ) -> Self {
        let revision = state
            .revision
            .map(|value| value.to_string())
            .unwrap_or_else(|| revision.to_owned());
        Self {
            title,
            action: action.to_owned(),
            submit,
            name: state.name,
            name_error: errors.name,
            image: state.oci_image,
            image_error: errors.image,
            script: state.setup_script,
            script_error: errors.script,
            revision,
            recipe_version: recipe_version.to_owned(),
            summary_error: errors.summary,
            environment_id: environment_id.to_owned(),
            show_status,
            show_delete,
            delete_error,
            affected_workflows: Vec::new(),
            status_html,
        }
    }

    pub(super) fn with_affected(mut self, affected: Vec<AffectedWorkflow>) -> Self {
        self.affected_workflows = affected;
        self
    }

    pub(super) fn contents(&self) -> EnvironmentFormContents<'_> {
        EnvironmentFormContents {
            action: &self.action,
            submit: self.submit,
            name: &self.name,
            name_error: self.name_error,
            image: &self.image,
            image_error: self.image_error,
            script: &self.script,
            script_error: self.script_error,
            revision: &self.revision,
            recipe_version: &self.recipe_version,
            summary_error: self.summary_error,
            environment_id: &self.environment_id,
            show_delete: self.show_delete,
            delete_error: self.delete_error,
            affected_workflows: &self.affected_workflows,
        }
    }
}

impl EnvironmentStatusView {
    pub(super) async fn from_record(
        record: &EnvironmentRecord,
        catalogue: &EnvironmentCatalogue,
        snapshots: &crate::environments::EnvironmentSnapshotRepository,
        cursor: RefreshCursor,
    ) -> Self {
        let latest = catalogue.preparation(&record.latest_preparation);
        let ready = record
            .ready_preparation
            .and_then(|id| catalogue.preparation(&id));
        let availability = match ready.as_ref().and_then(|item| item.snapshot.as_ref()) {
            Some(snapshot) => snapshots.inspect(snapshot).await,
            None => SnapshotAvailability::Missing,
        };
        let readiness = readiness_label(record, latest.as_ref(), availability);
        let (log_tail, durable_truncated, browser_truncated) = latest
            .as_ref()
            .map(|item| catalogue.log_projection(item))
            .unwrap_or_else(|| (String::new(), false, false));
        let mut history = catalogue.preparations_for(&record.id);
        let omitted = history.len().saturating_sub(HISTORY_LIMIT);
        history.truncate(HISTORY_LIMIT);
        let can_retry = latest.as_ref().is_some_and(|item| {
            !item.state.is_active() && item.recipe_version == record.recipe_version
        });
        Self {
            environment_id: record.id.as_hex(),
            cursor: EnvironmentCatalogue::cursor_token(cursor),
            active: latest.as_ref().is_some_and(|item| item.state.is_active()),
            readiness: readiness.label,
            readiness_tone: readiness.tone,
            phase: latest.as_ref().map(|item| item.phase.label()).unwrap_or(""),
            requested: latest
                .as_ref()
                .map(|item| format_time(item.requested_at_ms))
                .unwrap_or_default(),
            started: latest
                .as_ref()
                .and_then(|item| item.started_at_ms)
                .map(format_time)
                .unwrap_or_default(),
            finished: latest
                .as_ref()
                .and_then(|item| item.finished_at_ms)
                .map(format_time)
                .unwrap_or_default(),
            failure: latest
                .as_ref()
                .and_then(|item| item.failure)
                .map(|failure| failure.category.message())
                .unwrap_or(""),
            log_tail,
            log_truncated: durable_truncated || browser_truncated,
            can_retry,
            revision: record.revision.to_string(),
            recipe_version: record.recipe_version.as_hex(),
            snapshot: ready
                .as_ref()
                .and_then(|item| item.snapshot.as_ref())
                .map(snapshot_view),
            history: history.iter().map(history_row).collect(),
            omitted,
        }
    }
}

fn snapshot_view(snapshot: &PreparedSnapshot) -> SnapshotView {
    SnapshotView {
        digest: snapshot.snapshot_digest.as_str().to_owned(),
        image: snapshot.image_reference.clone(),
        integrity: format!(
            "{} {}",
            snapshot.upper_integrity.algorithm, snapshot.upper_integrity.value
        ),
        size: snapshot.upper_size_bytes,
    }
}

fn history_row(record: &PreparationRecord) -> HistoryRow {
    HistoryRow {
        ordinal: record.ordinal,
        version: record.recipe_version.short_hex(),
        status: record.state.as_str(),
        timestamp: format_time(record.requested_at_ms),
        digest: record
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .snapshot_digest
                    .as_str()
                    .chars()
                    .skip(7)
                    .take(12)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn readiness_label(
    record: &EnvironmentRecord,
    latest: Option<&PreparationRecord>,
    availability: SnapshotAvailability,
) -> Readiness {
    if record.ready_preparation.is_some() && availability != SnapshotAvailability::Available {
        return Readiness {
            label: "Snapshot unavailable",
            tone: "warning",
        };
    }
    let Some(latest) = latest else {
        return Readiness {
            label: "Not ready",
            tone: "neutral",
        };
    };
    if latest.state == PreparationState::Queued {
        return if record.ready_preparation.is_some() {
            Readiness {
                label: "Ready · replacement queued",
                tone: "info",
            }
        } else {
            Readiness {
                label: "Queued",
                tone: "info",
            }
        };
    }
    if latest.state == PreparationState::Preparing {
        return Readiness {
            label: "Preparing",
            tone: "info",
        };
    }
    if record.ready_preparation.is_some() {
        if latest.state == PreparationState::Ready && latest.recipe_version == record.recipe_version
        {
            return Readiness {
                label: "Ready",
                tone: "success",
            };
        }
        if matches!(
            latest.state,
            PreparationState::Failed | PreparationState::Interrupted
        ) {
            return Readiness {
                label: "Ready · replacement failed",
                tone: "warning",
            };
        }
        if latest.recipe_version != record.recipe_version {
            return Readiness {
                label: "Needs preparation",
                tone: "warning",
            };
        }
        return Readiness {
            label: "Ready",
            tone: "success",
        };
    }
    Readiness {
        label: "Not ready",
        tone: "neutral",
    }
}

fn format_time(ms: u64) -> String {
    let seconds = i64::try_from(ms / 1000).unwrap_or(0);
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
        .unwrap_or_else(|| "unknown".to_owned())
}
