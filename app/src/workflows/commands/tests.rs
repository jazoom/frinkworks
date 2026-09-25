use super::*;

#[test]
fn retired_file_commands_are_not_registered() {
    for name in ["apply-changes", "commit-candidate", "rollback", "unknown"] {
        assert!(SystemCommandId::parse(name).is_none());
    }
    let command = SystemCommandId::parse("repository-status").unwrap();
    assert!(command.contract().accepts(&[], &[]));
    assert!(!command.contract().accepts(&[ArtefactKind::Plan], &[]));
    assert!(!command.contract().accepts(&[], &[OutputKind::Plan]));
}
