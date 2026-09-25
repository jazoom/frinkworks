use crate::{
    agents::ToolId,
    environments::{
        EnvironmentCatalogue, EnvironmentId, EnvironmentSnapshotRepository, PreparationState,
        SnapshotAvailability,
    },
};

pub(crate) fn host_approval_label(policy: crate::execution::HostApprovalPolicy) -> &'static str {
    match policy {
        crate::execution::HostApprovalPolicy::AskEachTime => "Ask each time",
        crate::execution::HostApprovalPolicy::Automatic => "Automatic (YOLO)",
    }
}

pub(crate) fn directory_access_label(access: crate::execution::DirectoryAccess) -> &'static str {
    match access {
        crate::execution::DirectoryAccess::Read => "Read",
        crate::execution::DirectoryAccess::Write => "Write",
    }
}

pub(crate) struct EnvironmentOption {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) readiness: &'static str,
    pub(crate) availability: &'static str,
    pub(crate) selected: bool,
}

pub(crate) fn environment_options(
    catalogue: &EnvironmentCatalogue,
    snapshots: &EnvironmentSnapshotRepository,
    selected: Option<EnvironmentId>,
) -> Vec<EnvironmentOption> {
    let records = catalogue.list();
    let mut options = records
        .iter()
        .map(|record| {
            let (readiness, availability) = environment_status(record, catalogue, snapshots);
            EnvironmentOption {
                id: record.id.as_hex(),
                name: record.name.clone(),
                readiness,
                availability,
                selected: selected == Some(record.id),
            }
        })
        .collect::<Vec<_>>();
    if let Some(id) = selected
        && records.iter().all(|record| record.id != id)
    {
        options.push(EnvironmentOption {
            id: id.as_hex(),
            name: "Selected environment unavailable".to_owned(),
            readiness: "Unavailable",
            availability: "Unavailable",
            selected: true,
        });
    }
    options
}

pub(crate) fn environment_status(
    record: &crate::environments::EnvironmentRecord,
    catalogue: &EnvironmentCatalogue,
    snapshots: &EnvironmentSnapshotRepository,
) -> (&'static str, &'static str) {
    let latest = catalogue.preparation(&record.latest_preparation);
    let readiness = match latest.as_ref().map(|preparation| preparation.state) {
        Some(PreparationState::Ready) => "Ready",
        Some(PreparationState::Queued) => "Queued",
        Some(PreparationState::Preparing) => "Preparing",
        Some(PreparationState::Failed) => "Preparation failed",
        Some(PreparationState::Interrupted) => "Preparation interrupted",
        Some(PreparationState::Cancelled) => "Preparation cancelled",
        Some(PreparationState::Superseded) => "Preparation superseded",
        None => "Not ready",
    };
    let availability = record
        .ready_preparation
        .and_then(|id| catalogue.preparation(&id))
        .and_then(|preparation| preparation.snapshot)
        .map(|snapshot| snapshots.recorded_availability(&snapshot))
        .map_or("No snapshot", |availability| match availability {
            SnapshotAvailability::Available => "Available",
            SnapshotAvailability::Missing => "Snapshot unavailable",
            SnapshotAvailability::Corrupt => "Snapshot corrupt",
        });
    (readiness, availability)
}

pub(crate) struct ToolOption {
    pub(crate) field_name: &'static str,
    pub(crate) value: &'static str,
    pub(crate) label: &'static str,
    pub(crate) detail: &'static str,
    pub(crate) selected: bool,
}

pub(crate) fn initial_tool_ids() -> Vec<ToolId> {
    ToolId::ALL.to_vec()
}

pub(crate) fn tool_options(selected: &[String]) -> Vec<ToolOption> {
    ToolId::ALL
        .into_iter()
        .map(|tool| ToolOption {
            field_name: match tool {
                ToolId::List => "tool_list",
                ToolId::Read => "tool_read",
                ToolId::Edit => "tool_edit",
                ToolId::Write => "tool_write",
                ToolId::Run => "tool_run",
            },
            value: tool.as_str(),
            label: tool.label(),
            detail: match tool {
                ToolId::List => "List files and directories.",
                ToolId::Read => "Read file contents.",
                ToolId::Edit => "Apply exact replacements in existing files.",
                ToolId::Write => "Create files or replace a whole file.",
                ToolId::Run => "Run commands in the sandbox.",
            },
            selected: selected.iter().any(|value| value == tool.as_str()),
        })
        .collect()
}
