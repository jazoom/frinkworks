use crate::workflows::gates::{GateRevision, normalise_revision_note};

#[derive(Clone, Debug)]
pub(super) struct DecisionForm {
    pub(super) revision: GateRevision,
    pub(super) plan: String,
    pub(super) note: Option<String>,
    pub(super) conversation_surface: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FormError {
    Invalid,
    Note,
}

impl DecisionForm {
    pub(super) fn parse(
        pairs: Vec<(String, String)>,
        requires_note: bool,
    ) -> Result<Self, FormError> {
        let mut revision = None;
        let mut plan = None;
        let mut note = None;
        let mut conversation_surface = false;
        let mut seen = Vec::new();
        for (key, value) in pairs {
            if seen.contains(&key) {
                return Err(FormError::Invalid);
            }
            seen.push(key.clone());
            match key.as_str() {
                "gate-revision" => revision = GateRevision::parse(&value),
                "plan" => plan = Some(value),
                "note" if requires_note => note = normalise_revision_note(&value),
                "surface" if value == "conversation" => conversation_surface = true,
                _ => return Err(FormError::Invalid),
            }
        }
        if requires_note && note.is_none() {
            return Err(FormError::Note);
        }
        let plan = plan
            .filter(|value| !value.is_empty())
            .ok_or(FormError::Invalid)?;
        Ok(Self {
            revision: revision.ok_or(FormError::Invalid)?,
            plan,
            note,
            conversation_surface,
        })
    }
}

pub(super) struct EnvironmentSwitchDecisionForm {
    pub(super) revision: GateRevision,
    pub(super) plan: String,
    pub(super) conversation_revision: u32,
    pub(super) environment: crate::environments::EnvironmentId,
    pub(super) location: Option<crate::execution::ToolLocation>,
    pub(super) host_approval: Option<crate::execution::HostApprovalPolicy>,
    pub(super) directory_access: String,
    pub(super) network: String,
    pub(super) network_domains: String,
    pub(super) conversation_surface: bool,
}

impl EnvironmentSwitchDecisionForm {
    pub(super) fn parse(pairs: Vec<(String, String)>) -> Result<Self, FormError> {
        let mut revision = None;
        let mut plan = None;
        let mut conversation_revision = None;
        let mut environment = None;
        let mut location = None;
        let mut host_approval = None;
        let mut directory_access = String::new();
        let mut network = String::new();
        let mut network_domains = String::new();
        let mut conversation_surface = false;
        let mut seen = Vec::new();
        for (key, value) in pairs {
            if seen.contains(&key) {
                return Err(FormError::Invalid);
            }
            seen.push(key.clone());
            match key.as_str() {
                "gate-revision" => revision = GateRevision::parse(&value),
                "plan" if !value.is_empty() => plan = Some(value),
                "conversation-revision" => conversation_revision = value.parse().ok(),
                "environment" => environment = crate::environments::EnvironmentId::parse(&value),
                "directory_access" if value.len() <= 4096 => directory_access = value,
                "network" => network = value,
                "network_domains" => network_domains = value,
                "location" if !value.is_empty() => {
                    location = Some(
                        crate::execution::ToolLocation::parse(&value).ok_or(FormError::Invalid)?,
                    );
                }
                "host_approval" if !value.is_empty() => {
                    host_approval = Some(
                        crate::execution::HostApprovalPolicy::parse(&value)
                            .ok_or(FormError::Invalid)?,
                    );
                }
                "surface" if value == "conversation" => conversation_surface = true,
                _ => return Err(FormError::Invalid),
            }
        }
        Ok(Self {
            revision: revision.ok_or(FormError::Invalid)?,
            plan: plan.ok_or(FormError::Invalid)?,
            conversation_revision: conversation_revision.ok_or(FormError::Invalid)?,
            environment: environment.ok_or(FormError::Invalid)?,
            location,
            host_approval,
            directory_access,
            network,
            network_domains,
            conversation_surface,
        })
    }
}
