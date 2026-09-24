use crate::workflows::gates::{GateRevision, normalise_revision_note};

#[derive(Clone, Debug)]
pub(super) struct DecisionForm {
    pub(super) revision: GateRevision,
    pub(super) candidate: String,
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
        let mut candidate = None;
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
                "candidate" => candidate = Some(value),
                "plan" => plan = Some(value),
                "note" if requires_note => note = normalise_revision_note(&value),
                "surface" if value == "conversation" => conversation_surface = true,
                _ => return Err(FormError::Invalid),
            }
        }
        if requires_note && note.is_none() {
            return Err(FormError::Note);
        }
        let candidate = candidate
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        let plan = plan.filter(|value| !value.is_empty()).unwrap_or_default();
        if candidate.is_empty() == plan.is_empty() {
            return Err(FormError::Invalid);
        }
        Ok(Self {
            revision: revision.ok_or(FormError::Invalid)?,
            candidate,
            plan,
            note,
            conversation_surface,
        })
    }
}

pub(super) struct EnvironmentSwitchDecisionForm {
    pub(super) revision: GateRevision,
    pub(super) candidate: String,
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
        let mut candidate = None;
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
                "candidate" if !value.is_empty() => candidate = Some(value),
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
            candidate: candidate.ok_or(FormError::Invalid)?,
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

#[derive(Clone, Copy, Debug)]
pub(super) struct DiffQuery {
    pub(super) page: usize,
    pub(super) change: Option<usize>,
    pub(super) line: usize,
}

impl DiffQuery {
    pub(super) fn parse(
        page: Option<&str>,
        change: Option<&str>,
        line: Option<&str>,
    ) -> Option<Self> {
        Some(Self {
            page: parse_number(page.unwrap_or("0"))?,
            change: match change {
                Some(value) => Some(parse_number(value)?),
                None => None,
            },
            line: parse_number(line.unwrap_or("0"))?,
        })
    }
}

fn parse_number(raw: &str) -> Option<usize> {
    if raw.is_empty()
        || raw.len() > 10
        || (raw.len() > 1 && raw.starts_with('0'))
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    raw.parse().ok()
}
