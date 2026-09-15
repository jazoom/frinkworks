use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

use rand::{rand_core::TryRng, rngs::SysRng};
use sha2::{Digest, Sha256};

use crate::{conversations::ConversationId, sessions::SessionId};

use super::{CanonicalDirectoryIdentity, DirectoryGrant};

const TOKEN_BYTES: usize = 32;
const MAXIMUM_RUNTIME_RECORDS: usize = 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
struct GrantBinding {
    root: PathBuf,
    identity: CanonicalDirectoryIdentity,
    access: super::DirectoryAccess,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Subject {
    Draft {
        nonce: String,
        digest: [u8; 32],
    },
    Conversation {
        id: ConversationId,
        digest: [u8; 32],
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ConsentTarget {
    Grant(GrantBinding),
    Host,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConsentBinding {
    session: SessionId,
    subject: Subject,
    target: ConsentTarget,
}

#[derive(Default)]
pub(crate) struct AccessConsentStore {
    pending: Mutex<HashMap<String, ConsentBinding>>,
    approved: Mutex<HashMap<String, ConsentBinding>>,
    consumed_drafts: Mutex<HashSet<(SessionId, String)>>,
    launch_previews: Mutex<HashMap<String, LaunchConsent>>,
    launches: Mutex<HashMap<crate::workflows::RunId, LaunchConsent>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LaunchConsent {
    session: SessionId,
    conversation: ConversationId,
    settings: Vec<super::ExecutionSettings>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConsentError {
    Random,
    Invalid,
}

impl AccessConsentStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn request_draft(
        &self,
        session: SessionId,
        nonce: &str,
        directories: &[DirectoryGrant],
        grant: &DirectoryGrant,
    ) -> Result<String, ConsentError> {
        self.request(ConsentBinding {
            session,
            subject: Subject::Draft {
                nonce: nonce.to_owned(),
                digest: access_digest(directories),
            },
            target: ConsentTarget::Grant(grant.into()),
        })
    }

    pub(crate) fn request_conversation(
        &self,
        session: SessionId,
        conversation: ConversationId,
        settings: &super::ExecutionSettings,
        grant: &DirectoryGrant,
    ) -> Result<String, ConsentError> {
        self.request(ConsentBinding {
            session,
            subject: Subject::Conversation {
                id: conversation,
                digest: settings_digest(settings),
            },
            target: ConsentTarget::Grant(grant.into()),
        })
    }

    pub(crate) fn approve_draft(
        &self,
        request: &str,
        session: SessionId,
        nonce: &str,
        directories: &[DirectoryGrant],
        grant: &DirectoryGrant,
    ) -> Result<String, ConsentError> {
        self.approve(
            request,
            ConsentBinding {
                session,
                subject: Subject::Draft {
                    nonce: nonce.to_owned(),
                    digest: access_digest(directories),
                },
                target: ConsentTarget::Grant(grant.into()),
            },
        )
    }

    pub(crate) fn approve_conversation(
        &self,
        request: &str,
        session: SessionId,
        conversation: ConversationId,
        settings: &super::ExecutionSettings,
        grant: &DirectoryGrant,
    ) -> Result<String, ConsentError> {
        let digest = settings_digest(settings);
        let reference = self.approve(
            request,
            ConsentBinding {
                session,
                subject: Subject::Conversation {
                    id: conversation,
                    digest,
                },
                target: ConsentTarget::Grant(grant.into()),
            },
        )?;
        lock(&self.approved).retain(|_, binding| {
            !matches!(
                binding.subject,
                Subject::Conversation { id, digest: stored }
                    if id == conversation && stored != digest
            )
        });
        lock(&self.pending).retain(|_, binding| {
            !matches!(binding.subject, Subject::Conversation { id, .. } if id == conversation)
        });
        Ok(reference)
    }

    pub(crate) fn authorised_draft(
        &self,
        reference: &str,
        session: SessionId,
        nonce: &str,
        directories: &[DirectoryGrant],
        grant: &DirectoryGrant,
    ) -> bool {
        let expected = ConsentBinding {
            session,
            subject: Subject::Draft {
                nonce: nonce.to_owned(),
                digest: access_digest(directories),
            },
            target: ConsentTarget::Grant(grant.into()),
        };
        reference
            .split(',')
            .any(|reference| self.approved(reference).as_ref() == Some(&expected))
    }

    pub(crate) fn consume_draft(
        &self,
        reference: &str,
        session: SessionId,
        nonce: &str,
        settings: &super::ExecutionSettings,
        conversation: ConversationId,
        grants: &[DirectoryGrant],
    ) -> Result<(), ConsentError> {
        let subject = Subject::Draft {
            nonce: nonce.to_owned(),
            digest: access_digest(&settings.directories),
        };
        let mut pending = lock(&self.pending);
        let mut approved = lock(&self.approved);
        let mut consumed = lock(&self.consumed_drafts);
        let draft = (
            session,
            nonce.split(':').next().unwrap_or_default().to_owned(),
        );
        if consumed.contains(&draft) || consumed.len() >= MAXIMUM_RUNTIME_RECORDS {
            return Err(ConsentError::Invalid);
        }
        let mut replacements = Vec::new();
        for grant in grants {
            let expected = ConsentBinding {
                session,
                subject: subject.clone(),
                target: ConsentTarget::Grant(grant.into()),
            };
            if !reference
                .split(',')
                .any(|key| approved.get(key) == Some(&expected))
            {
                return Err(ConsentError::Invalid);
            }
            replacements.push((
                fresh_token()?,
                ConsentBinding {
                    session,
                    subject: Subject::Conversation {
                        id: conversation,
                        digest: settings_digest(settings),
                    },
                    target: ConsentTarget::Grant(grant.into()),
                },
            ));
        }
        if settings.location == super::ToolLocation::Host {
            let expected = host_draft_binding(session, nonce, settings);
            if !reference
                .split(',')
                .any(|key| approved.get(key) == Some(&expected))
            {
                return Err(ConsentError::Invalid);
            }
            replacements.push((
                fresh_token()?,
                ConsentBinding {
                    session,
                    subject: Subject::Conversation {
                        id: conversation,
                        digest: settings_digest(settings),
                    },
                    target: ConsentTarget::Host,
                },
            ));
        }
        // Consume the whole draft together so partial transfers cannot authorise another conversation.
        let retain = |_: &String, binding: &mut ConsentBinding| {
            !(binding.session == session
                && matches!(&binding.subject, Subject::Draft { nonce: stored, .. }
                    if stored.split(':').next() == nonce.split(':').next()))
        };
        pending.retain(retain);
        approved.retain(retain);
        approved.extend(replacements);
        consumed.insert(draft);
        Ok(())
    }

    pub(crate) fn retain_sessions(&self, live: impl Fn(&SessionId) -> bool) {
        lock(&self.pending).retain(|_, binding| live(&binding.session));
        lock(&self.approved).retain(|_, binding| live(&binding.session));
        lock(&self.consumed_drafts).retain(|(session, _)| live(session));
        lock(&self.launch_previews).retain(|_, binding| live(&binding.session));
        lock(&self.launches).retain(|_, binding| live(&binding.session));
    }

    pub(crate) fn invalidate_conversation(&self, conversation: ConversationId) {
        self.invalidate_approvals(conversation);
        lock(&self.launch_previews).retain(|_, binding| binding.conversation != conversation);
        lock(&self.launches).retain(|_, binding| binding.conversation != conversation);
        lock(&self.pending).retain(|_, binding| {
            !matches!(binding.subject, Subject::Conversation { id, .. } if id == conversation)
        });
    }

    fn invalidate_approvals(&self, conversation: ConversationId) {
        lock(&self.approved).retain(|_, binding| {
            !matches!(binding.subject, Subject::Conversation { id, .. } if id == conversation)
        });
    }

    pub(crate) fn request_launch(
        &self,
        session: SessionId,
        conversation: ConversationId,
        settings: Vec<super::ExecutionSettings>,
    ) -> Result<String, ConsentError> {
        let mut previews = lock(&self.launch_previews);
        if previews.len() >= MAXIMUM_RUNTIME_RECORDS {
            return Err(ConsentError::Invalid);
        }
        let token = fresh_token()?;
        previews.insert(
            token.clone(),
            LaunchConsent {
                session,
                conversation,
                settings,
            },
        );
        Ok(token)
    }

    pub(crate) fn approve_launch(
        &self,
        request: &str,
        run: crate::workflows::RunId,
        session: SessionId,
        conversation: ConversationId,
        settings: Vec<super::ExecutionSettings>,
    ) -> Result<(), ConsentError> {
        let expected = LaunchConsent {
            session,
            conversation,
            settings,
        };
        let mut previews = lock(&self.launch_previews);
        if previews.get(request) != Some(&expected) {
            return Err(ConsentError::Invalid);
        }
        let mut launches = lock(&self.launches);
        if launches.len() >= MAXIMUM_RUNTIME_RECORDS || launches.contains_key(&run) {
            return Err(ConsentError::Invalid);
        }
        previews.remove(request);
        launches.insert(run, expected);
        Ok(())
    }

    pub(crate) fn approve_handoff(
        &self,
        run: crate::workflows::RunId,
        session: SessionId,
        conversation: ConversationId,
        settings: Vec<super::ExecutionSettings>,
    ) -> Result<(), &'static str> {
        let mut launches = lock(&self.launches);
        if !launches.contains_key(&run) && launches.len() >= MAXIMUM_RUNTIME_RECORDS {
            return Err(
                "The runtime consent limit is full. Restart Power Plant before continuation.",
            );
        }
        launches.insert(
            run,
            LaunchConsent {
                session,
                conversation,
                settings,
            },
        );
        Ok(())
    }

    pub(crate) fn authorised_launch(
        &self,
        run: crate::workflows::RunId,
        session: SessionId,
        conversation: ConversationId,
        settings: &super::ExecutionSettings,
    ) -> bool {
        lock(&self.launches).get(&run).is_some_and(|binding| {
            binding.session == session
                && binding.conversation == conversation
                && binding.settings.contains(settings)
        })
    }

    pub(crate) fn authorised_conversation(
        &self,
        session: SessionId,
        conversation: ConversationId,
        settings: &super::ExecutionSettings,
        grant: &DirectoryGrant,
    ) -> bool {
        self.authorised_target(
            session,
            conversation,
            settings,
            ConsentTarget::Grant(grant.into()),
        )
    }

    pub(crate) fn request_host_draft(
        &self,
        session: SessionId,
        nonce: &str,
        settings: &super::ExecutionSettings,
    ) -> Result<String, ConsentError> {
        self.request(host_draft_binding(session, nonce, settings))
    }

    pub(crate) fn request_host_conversation(
        &self,
        session: SessionId,
        conversation: ConversationId,
        settings: &super::ExecutionSettings,
    ) -> Result<String, ConsentError> {
        self.request(host_conversation_binding(session, conversation, settings))
    }

    pub(crate) fn approve_host_draft(
        &self,
        request: &str,
        session: SessionId,
        nonce: &str,
        settings: &super::ExecutionSettings,
    ) -> Result<String, ConsentError> {
        self.approve(request, host_draft_binding(session, nonce, settings))
    }

    pub(crate) fn approve_host_conversation(
        &self,
        request: &str,
        session: SessionId,
        conversation: ConversationId,
        settings: &super::ExecutionSettings,
    ) -> Result<String, ConsentError> {
        let digest = settings_digest(settings);
        let reference = self.approve(
            request,
            host_conversation_binding(session, conversation, settings),
        )?;
        lock(&self.approved).retain(|_, binding| {
            !(binding.target == ConsentTarget::Host
                && matches!(
                    binding.subject,
                    Subject::Conversation { id, digest: stored }
                        if id == conversation && stored != digest
                ))
        });
        lock(&self.pending).retain(|_, binding| {
            !(binding.target == ConsentTarget::Host
                && matches!(binding.subject, Subject::Conversation { id, .. } if id == conversation))
        });
        Ok(reference)
    }

    pub(crate) fn authorised_host_draft(
        &self,
        reference: &str,
        session: SessionId,
        nonce: &str,
        settings: &super::ExecutionSettings,
    ) -> bool {
        let expected = host_draft_binding(session, nonce, settings);
        reference
            .split(',')
            .any(|reference| self.approved(reference).as_ref() == Some(&expected))
    }

    pub(crate) fn authorised_host_conversation(
        &self,
        session: SessionId,
        conversation: ConversationId,
        settings: &super::ExecutionSettings,
    ) -> bool {
        self.authorised_target(session, conversation, settings, ConsentTarget::Host)
    }

    fn authorised_target(
        &self,
        session: SessionId,
        conversation: ConversationId,
        settings: &super::ExecutionSettings,
        target: ConsentTarget,
    ) -> bool {
        let expected = ConsentBinding {
            session,
            subject: Subject::Conversation {
                id: conversation,
                digest: settings_digest(settings),
            },
            target,
        };
        lock(&self.approved)
            .values()
            .any(|binding| binding == &expected)
    }

    fn request(&self, binding: ConsentBinding) -> Result<String, ConsentError> {
        let token = fresh_token()?;
        let mut pending = lock(&self.pending);
        if let Subject::Draft { nonce, .. } = &binding.subject
            && lock(&self.consumed_drafts).contains(&(
                binding.session,
                nonce.split(':').next().unwrap_or_default().to_owned(),
            ))
        {
            return Err(ConsentError::Invalid);
        }
        pending.retain(|_, stored| stored != &binding);
        if pending.len() >= MAXIMUM_RUNTIME_RECORDS {
            return Err(ConsentError::Invalid);
        }
        pending.insert(token.clone(), binding);
        Ok(token)
    }

    fn approve(&self, request: &str, expected: ConsentBinding) -> Result<String, ConsentError> {
        let mut pending = lock(&self.pending);
        if pending.get(request) != Some(&expected) {
            return Err(ConsentError::Invalid);
        }
        pending.remove(request);
        let reference = fresh_token()?;
        let mut approved = lock(&self.approved);
        approved.retain(|_, stored| stored != &expected);
        if approved.len() >= MAXIMUM_RUNTIME_RECORDS {
            return Err(ConsentError::Invalid);
        }
        approved.insert(reference.clone(), expected);
        Ok(reference)
    }

    fn approved(&self, reference: &str) -> Option<ConsentBinding> {
        lock(&self.approved).get(reference).cloned()
    }
}

impl From<&DirectoryGrant> for GrantBinding {
    fn from(grant: &DirectoryGrant) -> Self {
        Self {
            root: grant.host_path.clone(),
            identity: grant.identity,
            access: grant.access,
        }
    }
}

pub(crate) fn draft_nonce() -> Result<String, ConsentError> {
    fresh_token()
}

fn access_digest(directories: &[DirectoryGrant]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"power-plant-access-consent-v1\0");
    for grant in directories {
        digest.update(grant.id.as_hex());
        digest.update([0]);
        digest.update(grant.host_path.as_os_str().as_encoded_bytes());
        digest.update([0]);
        digest.update(grant.identity.device.to_le_bytes());
        digest.update(grant.identity.inode.to_le_bytes());
        digest.update(grant.alias.as_bytes());
        digest.update([0]);
        digest.update(grant.access.as_str());
        digest.update([0]);
    }
    digest.finalize().into()
}

fn host_draft_binding(
    session: SessionId,
    nonce: &str,
    settings: &super::ExecutionSettings,
) -> ConsentBinding {
    ConsentBinding {
        session,
        subject: Subject::Draft {
            nonce: nonce.to_owned(),
            digest: settings_digest(settings),
        },
        target: ConsentTarget::Host,
    }
}

fn host_conversation_binding(
    session: SessionId,
    conversation: ConversationId,
    settings: &super::ExecutionSettings,
) -> ConsentBinding {
    ConsentBinding {
        session,
        subject: Subject::Conversation {
            id: conversation,
            digest: settings_digest(settings),
        },
        target: ConsentTarget::Host,
    }
}

pub(crate) fn settings_digest(settings: &super::ExecutionSettings) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(access_digest(&settings.directories));
    digest.update(settings.environment.as_hex());
    for tool in &settings.tools {
        digest.update(tool.as_str());
        digest.update([0]);
    }
    digest.update(settings.network.as_str());
    for domain in settings.network.domains() {
        digest.update([0]);
        digest.update(domain);
    }
    digest.update(settings.location.as_str());
    digest.update(settings.host_approval.as_str());
    digest.finalize().into()
}

fn fresh_token() -> Result<String, ConsentError> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| ConsentError::Random)?;
    Ok(crate::hex::encode(&bytes))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests;
