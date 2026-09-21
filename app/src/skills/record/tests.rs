use super::*;
use crate::execution::resources::MAXIMUM_SKILL_BODY_BYTES;

#[test]
fn validation_bounds_the_complete_file_and_rejects_invalid_input() {
    let header = "---\nname: example\ndescription: Example task\n---\n";
    let exact = format!(
        "{header}{}",
        "x".repeat(MAXIMUM_SKILL_BODY_BYTES - header.len())
    );
    assert!(SkillRecord::parse("example".to_owned(), exact.clone()).is_ok());
    assert_eq!(
        SkillRecord::parse("example".to_owned(), format!("{exact}x")),
        Err(SkillError::Bound)
    );
    for markdown in [
        "body without frontmatter",
        "---\nname: example\n---\nbody",
        "---\nname: example\nname: other\ndescription: task\n---\nbody",
        "---\nname: example\ndescription: [one, two]\n---\nbody",
        "---\nname: example\ndescription: task\n---\nbody\0",
    ] {
        assert_eq!(
            SkillRecord::parse("example".to_owned(), markdown.to_owned()),
            Err(SkillError::Metadata)
        );
    }
}

#[test]
fn ordinary_yaml_and_body_are_preserved_without_private_metadata() {
    let markdown = "---\nname: 'example'\ndescription: >\n  Review code\n  for defects.\nmetadata:\n  author: Someone\n---\n# Instructions\n\nKeep **this** content.\n";
    let record = SkillRecord::parse("copied-folder".to_owned(), markdown.to_owned()).unwrap();
    assert_eq!(record.name, "example");
    assert_eq!(record.description, "Review code for defects.");
    assert_eq!(record.markdown, markdown);
    // Unknown YAML keys cannot introduce a separate persisted revision parser.
    let supplied = markdown.replace("metadata:", "revision: 2\nmetadata:");
    let record = SkillRecord::parse("copied-folder".to_owned(), supplied.clone()).unwrap();
    assert_eq!(record.markdown, supplied);
}
