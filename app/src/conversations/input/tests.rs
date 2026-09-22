use super::*;
use crate::execution::{ResourceKind, ResourceSource};

fn offer(scope: &str, name: &str, body: &str) -> SkillOffer {
    let path = format!("/.agents/skills/{name}/SKILL.md");
    let source = ResourceSource::new(ResourceKind::Skill, scope, &path, body.as_bytes());
    SkillOffer {
        scope: scope.to_owned(),
        name: name.to_owned(),
        description: format!("Use {name}."),
        body: body.to_owned(),
        source,
        relative_base: format!("/.agents/skills/{name}"),
    }
}

#[test]
fn escaped_prefix_sends_literal_text() {
    let expansion = expand("\\/skill:code review this", &[], None).expect("expand");
    assert_eq!(expansion.expanded, "/skill:code review this");
    assert!(expansion.provenance.is_none());
    assert_eq!(
        expand("\\ /skill:code", &[], None).unwrap().expanded,
        "\\ /skill:code"
    );
    let untouched = expand("\\path here", &[], None).expect("expand");
    assert_eq!(untouched.expanded, "\\path here");
}

#[test]
fn restored_literal_text_escapes_a_leading_command_prefix() {
    assert_eq!(escape_leading("/skill:name"), "\\/skill:name");
    assert_eq!(escape_leading("  !ls"), "  \\!ls");
    assert_eq!(escape_leading("plain text"), "plain text");
    // One round trip restores the exact literal text.
    let restored = escape_leading("/skill:name");
    let expansion = expand(&restored, &[], None).expect("expand");
    assert_eq!(expansion.expanded, "/skill:name");
    assert!(expansion.provenance.is_none());
}

#[test]
fn command_with_instructions_expands_body_and_instructions() {
    let offers = [offer("global", "code-review", "Read the diff.")];
    let expansion =
        expand("/skill:code-review inspect src/main.rs", &offers, None).expect("expand");
    assert_eq!(
        expansion.expanded,
        "Skill directory: /.agents/skills/code-review\n\nRead the diff.\n\ninspect src/main.rs"
    );
    let provenance = expansion.provenance.expect("provenance");
    assert_eq!(provenance.typed, "/skill:code-review inspect src/main.rs");
    assert_eq!(provenance.expanded, expansion.expanded);
    assert_eq!(provenance.relative_base, "/.agents/skills/code-review");
}

#[test]
fn unqualified_duplicate_name_is_ambiguous() {
    let offers = [
        offer("global", "debug", "One."),
        offer("project", "debug", "Two."),
    ];
    assert_eq!(
        expand("/skill:debug", &offers, None).unwrap_err(),
        InputError::Ambiguous
    );
    let scoped = expand("/skill:project/debug", &offers, None).expect("scoped");
    assert!(scoped.expanded.ends_with("Two."));
    assert!(!scoped.expanded.contains("One."));
}

#[test]
fn unknown_and_empty_commands_are_rejected() {
    assert_eq!(expand("/skill:", &[], None).unwrap_err(), InputError::Empty);
    assert_eq!(
        expand("  /unknown instruction", &[], None).unwrap_err(),
        InputError::Unknown
    );
    assert_eq!(
        expand("/skill:missing", &[], None).unwrap_err(),
        InputError::Unknown
    );
}

#[test]
fn changed_source_is_rejected_against_a_preview() {
    let offers = [offer("global", "code-review", "Read the diff.")];
    let path = expand("/skill:code-review", &offers, None)
        .unwrap()
        .provenance
        .unwrap()
        .preview_source();
    let hash = offers[0].source.content_hash.clone();
    expand_checked("/skill:code-review", &offers, None, Some((&path, &hash))).expect("unchanged");
    let mut moved = offers[0].clone();
    moved.source.scope = "other".to_owned();
    assert_eq!(
        expand_checked("/skill:code-review", &[moved], None, Some((&path, &hash))).unwrap_err(),
        InputError::Changed
    );
    let other = offer("global", "code-review", "Read the new diff.");
    assert_eq!(
        expand_checked("/skill:code-review", &[other], None, Some((&path, &hash))).unwrap_err(),
        InputError::Changed
    );
}

#[test]
fn credential_in_a_skill_body_is_rejected() {
    let offers = [offer("global", "leaky", "Use the key sk-secret now.")];
    assert_eq!(
        expand("/skill:leaky", &offers, Some("sk-secret")).unwrap_err(),
        InputError::Credential
    );
    expand("/skill:leaky", &offers, None).expect("no selected credential");
    let oversized = offer(
        "global",
        "large",
        &"x".repeat(crate::execution::resources::MAXIMUM_SKILL_BODY_BYTES + 1),
    );
    assert_eq!(
        expand("/skill:large", &[oversized], None).unwrap_err(),
        InputError::Bound
    );
}
