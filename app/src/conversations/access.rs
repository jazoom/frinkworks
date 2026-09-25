use crate::agents::{AccessMode, DirectoryPolicy, EffectiveAuthority, NetworkAccess, PolicyGrant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConversationAccessError {
    Path,
    Preset,
}

impl ConversationAccessError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Path => "A granted directory is no longer at the saved path.",
            Self::Preset => "The applied preset no longer permits this directory.",
        }
    }
}

impl std::fmt::Display for ConversationAccessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ConversationAccessError {}

pub(crate) fn intersect_network(
    selected: &NetworkAccess,
    ceiling: Option<&NetworkAccess>,
) -> NetworkAccess {
    let Some(ceiling) = ceiling else {
        return selected.clone();
    };
    match (selected, ceiling) {
        (NetworkAccess::None, _) | (_, NetworkAccess::None) => NetworkAccess::None,
        (NetworkAccess::Public, NetworkAccess::Public) => NetworkAccess::Public,
        (NetworkAccess::Public, NetworkAccess::Restricted(domains))
        | (NetworkAccess::Restricted(domains), NetworkAccess::Public) => {
            NetworkAccess::Restricted(domains.clone())
        }
        (NetworkAccess::Restricted(selected), NetworkAccess::Restricted(ceiling)) => {
            let mut domains = Vec::new();
            for left in selected {
                for right in ceiling {
                    if let Some(narrower) = narrower_domain(left, right)
                        && !domains.contains(&narrower)
                    {
                        domains.push(narrower);
                    }
                }
            }
            if domains.is_empty() {
                NetworkAccess::None
            } else {
                NetworkAccess::Restricted(domains)
            }
        }
    }
}

fn narrower_domain(left: &str, right: &str) -> Option<String> {
    if left == right || left.ends_with(&format!(".{right}")) {
        Some(left.to_owned())
    } else if right.ends_with(&format!(".{left}")) {
        Some(right.to_owned())
    } else {
        None
    }
}

pub(crate) fn apply_settings_ceiling(
    base: &EffectiveAuthority,
    settings: &crate::execution::ExecutionSettings,
) -> Result<EffectiveAuthority, ConversationAccessError> {
    if settings.tools.iter().any(|tool| !base.tools.contains(tool))
        || intersect_network(&settings.network, Some(&base.network)) != settings.network
    {
        return Err(ConversationAccessError::Preset);
    }
    let mut grants = Vec::new();
    for requested in &settings.directories {
        if !requested.is_available() {
            return Err(ConversationAccessError::Path);
        }
        let Some(grant) = base
            .policy
            .grants()
            .iter()
            .find(|grant| grant.host_path == requested.host_path)
        else {
            return Err(ConversationAccessError::Preset);
        };
        let access = match requested.access {
            crate::execution::DirectoryAccess::Read => AccessMode::ReadOnly,
            crate::execution::DirectoryAccess::Write if grant.access.is_writable() => {
                AccessMode::ReadWrite
            }
            crate::execution::DirectoryAccess::Write => {
                return Err(ConversationAccessError::Preset);
            }
        };
        grants.push(PolicyGrant {
            alias: grant.alias.clone(),
            guest_path: grant.guest_path.clone(),
            host_path: grant.host_path.clone(),
            access,
        });
    }
    if !base.grant_alias.is_empty() && !grants.iter().any(|grant| grant.alias == base.grant_alias) {
        return Err(ConversationAccessError::Preset);
    }
    let grant_access = if base.grant_alias.is_empty() {
        AccessMode::ReadOnly
    } else {
        grants
            .iter()
            .find(|grant| grant.alias == base.grant_alias)
            .map(|grant| grant.access)
            .ok_or(ConversationAccessError::Preset)?
    };
    let policy = DirectoryPolicy::from_grants(grants, base.grant_alias.clone());
    policy
        .confirm_hosts()
        .map_err(|_| ConversationAccessError::Path)?;
    Ok(EffectiveAuthority {
        revision: base.revision,
        grant_alias: base.grant_alias.clone(),
        grant_access,
        tools: settings.tools.clone(),
        network: settings.network.clone(),
        policy,
    })
}

#[cfg(test)]
mod tests;
