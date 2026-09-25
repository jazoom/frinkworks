use super::*;
use crate::agents::{NetworkAccess, ToolId};

#[test]
fn private_workspace_uses_the_copied_settings_without_a_live_preset_ceiling() {
    let state = crate::tests::test_state(crate::config::RuntimeConfig::development());
    let preset = state
        .agents
        .create(crate::agents::AgentDraft {
            name: "Offline".to_owned(),
            instructions: String::new(),
            selection: None,
            tools: vec![ToolId::Run],
            network: NetworkAccess::None,
            directories: Vec::new(),
            primary_directory: String::new(),
        })
        .unwrap();
    let selection = crate::providers::ModelSelection::new(
        crate::providers::ProviderKind::Xai,
        "test-model".to_owned(),
        None,
    )
    .unwrap();
    let mut model = crate::conversations::ConversationModelConfiguration::direct(
        selection,
        crate::tests::test_environment_id(),
    );
    model.settings.tools = preset.tools.clone();
    model.settings.network = NetworkAccess::Public;
    model.settings.tools.push(ToolId::Write);
    let record = state
        .conversations
        .create_saved(
            crate::conversations::ConversationId::generate().unwrap(),
            None,
            Some(model),
            Vec::new(),
        )
        .unwrap();
    let authority = crate::execution::ProjectFreeAuthority::from_settings(
        record.revision,
        &record.model.as_ref().unwrap().settings,
    )
    .unwrap();
    assert_eq!(authority.network, NetworkAccess::Public);
    assert_eq!(authority.tools, vec![ToolId::Run, ToolId::Write]);
    assert!(authority.policy.grants().is_empty());
    assert!(record.model.as_ref().unwrap().preset.is_none());
}

#[test]
fn network_intersection_never_expands_a_conversation_selection() {
    assert_eq!(
        intersect_network(
            &NetworkAccess::Public,
            Some(&NetworkAccess::Restricted(vec![
                "api.example.com".to_owned()
            ])),
        ),
        NetworkAccess::Restricted(vec!["api.example.com".to_owned()])
    );
    assert_eq!(
        intersect_network(
            &NetworkAccess::Restricted(vec!["example.com".to_owned()]),
            Some(&NetworkAccess::Restricted(vec![
                "api.example.com".to_owned()
            ])),
        ),
        NetworkAccess::Restricted(vec!["api.example.com".to_owned()])
    );
    assert_eq!(
        intersect_network(
            &NetworkAccess::Restricted(vec!["example.com".to_owned()]),
            Some(&NetworkAccess::Restricted(vec!["other.example".to_owned()])),
        ),
        NetworkAccess::None
    );
}
