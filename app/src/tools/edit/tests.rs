use super::{Change, EditError, apply};

fn change(search: &str, replace: &str) -> Change {
    Change {
        search: search.to_owned(),
        replace: replace.to_owned(),
    }
}

#[test]
fn a_multi_edit_batch_uses_original_offsets() {
    let original = "one two three\n";
    let next = apply(original, &[change("one", "1"), change("three", "3")]).expect("batch");
    assert_eq!(next, "1 two 3\n");
}

#[test]
fn later_replacements_do_not_create_matches_for_earlier_edits() {
    let original = "abc";
    let next = apply(original, &[change("a", "b"), change("bc", "X")]).expect("original spans");
    assert_eq!(next, "bX");
}

#[test]
fn ambiguous_matches_are_rejected() {
    assert_eq!(
        apply("foo foo", &[change("foo", "bar")]),
        Err(EditError::Ambiguous)
    );
}

#[test]
fn overlapping_and_nested_ranges_are_rejected() {
    assert_eq!(
        apply("abcdef", &[change("abc", "X"), change("cde", "Y")]),
        Err(EditError::Overlap)
    );
    assert_eq!(
        apply("abcdef", &[change("abcdef", "X"), change("cd", "Y")]),
        Err(EditError::Overlap)
    );
}

#[test]
fn empty_search_and_empty_batch_are_rejected() {
    assert_eq!(apply("text", &[]), Err(EditError::EmptyBatch));
    assert_eq!(
        apply("text", &[change("", "x")]),
        Err(EditError::EmptySearch)
    );
}

#[test]
fn a_rejected_batch_does_not_change_the_original() {
    let original = "keep me\n";
    let result = apply(original, &[change("missing", "x"), change("keep", "KEEP")]);
    assert_eq!(result, Err(EditError::NotFound));
}

#[test]
fn overlapping_occurrences_are_ambiguous() {
    assert_eq!(
        apply("aaa", &[change("aa", "x")]),
        Err(EditError::Ambiguous)
    );
    assert_eq!(
        apply("ééé", &[change("éé", "x")]),
        Err(EditError::Ambiguous)
    );
}

#[test]
fn nul_in_search_is_invalid_text() {
    assert_eq!(
        apply("ok", &[change("ok\0secret", "x")]),
        Err(EditError::InvalidText)
    );
}

#[test]
fn oversized_replacements_are_rejected() {
    let original = "ab";
    let result = apply(
        original,
        &[change(
            "ab",
            &"x".repeat(crate::tools::MAXIMUM_WRITE_BYTES + 1),
        )],
    );
    assert_eq!(result, Err(EditError::OversizedInput));
}

fn replace_file(
    root: &std::path::Path,
    target: &std::path::Path,
    original: &[u8],
    next: &[u8],
) -> std::process::Output {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("sh")
        .args(["-c", super::super::CONFINED_EDIT_SCRIPT, "edit-test"])
        .arg(root)
        .arg(target)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("shell");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&super::super::edit_payload(original, next))
        .expect("payload");
    child.wait_with_output().expect("exit")
}

#[test]
fn atomic_edit_preserves_mode_and_rejects_a_stale_baseline() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("directory");
    let target = dir.path().join("file");
    let original = "one two three\r\n";
    std::fs::write(&target, original).expect("file");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o751)).expect("mode");
    let next = apply(original, &[change("one", "1"), change("three", "3")]).expect("batch");
    let result = replace_file(dir.path(), &target, original.as_bytes(), next.as_bytes());
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        std::fs::read_to_string(&target).expect("edited"),
        "1 two 3\r\n"
    );
    assert_eq!(
        std::fs::metadata(&target)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o751
    );
    let result = replace_file(dir.path(), &target, original.as_bytes(), b"stale");
    assert_eq!(result.status.code(), Some(5));
    assert_eq!(std::fs::read_to_string(&target).expect("unchanged"), next);
    assert_eq!(std::fs::read_dir(dir.path()).expect("entries").count(), 1);
}

#[test]
fn edit_rejects_symlinks_and_traversal_outside_the_root() {
    let dir = tempfile::tempdir().expect("directory");
    let root = dir.path().join("root");
    std::fs::create_dir(&root).expect("root");
    let secret = dir.path().join("secret");
    std::fs::write(&secret, "secret").expect("secret");
    let link = root.join("link");
    std::os::unix::fs::symlink(&secret, &link).expect("link");
    for target in [link, root.join("../secret")] {
        let result = replace_file(&root, &target, b"secret", b"changed");
        assert_eq!(result.status.code(), Some(4));
    }
    assert_eq!(
        std::fs::read_to_string(secret).expect("unchanged"),
        "secret"
    );
}

#[test]
fn truncated_payload_never_replaces_the_file() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().expect("directory");
    let target = dir.path().join("file");
    std::fs::write(&target, "old").expect("file");
    let mut child = Command::new("sh")
        .args(["-c", super::super::CONFINED_EDIT_SCRIPT, "edit-test"])
        .arg(dir.path())
        .arg(&target)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("shell");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"3\n10\noldshort")
        .expect("payload");
    assert!(!child.wait().expect("exit").success());
    assert_eq!(std::fs::read_to_string(target).expect("unchanged"), "old");
}
