use super::*;

#[tokio::test]
async fn local_read_projections_do_not_require_a_provider() {
    let state = crate::slices::conversations::tests::test_state();
    let id = ConversationId::generate().expect("id");
    state
        .conversations
        .create_saved(id, Some("Local conversation".to_owned()), None, vec![])
        .expect("conversation");
    assert!(!state.vault.has_providers());
    assert!(
        super::super::recent::live(State(state.clone()))
            .await
            .is_ok()
    );
    assert!(live(State(state.clone()), Path(id.as_hex())).await.is_ok());

    let missing = ConversationId::generate().expect("missing id");
    assert!(matches!(
        live(State(state), Path(missing.as_hex())).await,
        Err(LiveReject::Retire)
    ));
}
