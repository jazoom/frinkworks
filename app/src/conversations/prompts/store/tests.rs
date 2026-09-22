use super::*;

impl PromptRecord {
    fn template(self) -> super::super::PromptTemplate {
        super::super::PromptTemplate {
            name: self.name.clone(),
            description: self.description,
            argument_hint: self.argument_hint,
            body: self.body,
            source: crate::execution::ResourceSource::new(
                crate::execution::ResourceKind::Prompt,
                super::super::PROMPTS_SCOPE,
                format!(
                    "{}/{}.{}",
                    super::super::PROMPTS_DIRECTORY,
                    self.name,
                    PROMPT_EXTENSION
                ),
                self.markdown.as_bytes(),
            ),
        }
    }
}

impl PromptStore {
    pub(crate) fn temporary() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Self::open(directory.path().join("prompts")).unwrap();
        store._temporary = Some(directory);
        store
    }
}

fn markdown(description: &str, body: &str) -> String {
    super::super::compose(description, "", body)
}

#[test]
fn create_get_and_update_round_trip_the_file() {
    let store = PromptStore::temporary();
    let record = store
        .create("review", markdown("Review a change.", "Review $1."))
        .unwrap();
    assert_eq!(record.name, "review");
    assert_eq!(record.description, "Review a change.");
    assert_eq!(record.body, "Review $1.");
    assert!(record.fingerprint.starts_with("sha256:"));
    assert_eq!(store.get("review").unwrap(), record);

    let updated = store
        .update(
            "review",
            &record.fingerprint,
            markdown("Review carefully.", "Review $@."),
        )
        .unwrap();
    assert_eq!(updated.description, "Review carefully.");
    let file = store.dir.join("review.md");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "---\ndescription: Review carefully.\n---\n\nReview $@.\n"
    );

    // A stale fingerprint never replaces a newer file.
    assert_eq!(
        store.update("review", &record.fingerprint, markdown("Stale.", "Stale.")),
        Err(PromptStoreError::Conflict)
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), updated.markdown);
}

#[test]
fn create_refuses_case_insensitive_duplicate_names() {
    let store = PromptStore::temporary();
    let original = store.create("review", markdown("", "Review.")).unwrap();
    for name in ["review", "Review", "REVIEW"] {
        assert_eq!(
            store.create(name, markdown("", "Other.")),
            Err(PromptStoreError::Conflict)
        );
    }
    assert_eq!(store.get("review").unwrap(), original);
    assert_eq!(
        super::super::discover(store.dir.parent().unwrap())
            .templates
            .len(),
        1
    );
}

#[test]
fn names_and_bodies_are_bounded_before_writes() {
    let store = PromptStore::temporary();
    for name in [".", "..", "skill", "bad name", "bad/name", &"x".repeat(129)] {
        assert_eq!(
            store.create(name, markdown("", "Body.")),
            Err(PromptStoreError::Name),
            "{name}"
        );
    }
    assert_eq!(
        store.create("review", markdown("", "")),
        Err(PromptStoreError::Malformed)
    );
    let oversized = format!("{}\n", "x".repeat(MAXIMUM_PROMPT_BODY_BYTES));
    assert_eq!(
        store.create("review", oversized),
        Err(PromptStoreError::Bound)
    );
    assert!(store.list().unwrap().records.is_empty());
}

#[test]
fn a_created_template_is_a_plain_global_markdown_file() {
    let store = PromptStore::temporary();
    let record = store
        .create("review", markdown("Review a change.", "Review $1."))
        .unwrap();
    let file = store.dir.join("review.md");
    assert!(file.is_file());
    let template = record.template();
    assert_eq!(template.name, "review");
    assert_eq!(template.source.scope, "global");
    assert_eq!(template.source.path, "prompts/review.md");
    // Discovery must read the file the store wrote.
    let catalogue = super::super::discover(store.dir.parent().unwrap());
    assert_eq!(catalogue.templates.len(), 1);
    assert_eq!(catalogue.templates[0], template);
}

#[test]
fn malformed_files_remain_repairable_without_blocking_other_templates() {
    let store = PromptStore::temporary();
    let broken = "---\ndescription: Unfinished\n";
    fs::write(store.dir.join("broken.md"), broken).unwrap();
    let valid = store.create("review", markdown("", "Review.")).unwrap();
    let listing = store.list().unwrap();
    assert_eq!(listing.records.len(), 2);
    assert!(listing.records.contains(&valid));
    let damaged = store.get("broken").unwrap();
    assert_eq!(damaged.problem, Some(PromptStoreError::Malformed));
    assert_eq!(damaged.body, broken);
    assert_eq!(damaged.fingerprint, content_hash(broken.as_bytes()));
    assert_eq!(
        store.create("BROKEN", markdown("", "Replacement.")),
        Err(PromptStoreError::Conflict)
    );
    let repaired = store
        .update("broken", &damaged.fingerprint, markdown("", "Repaired."))
        .unwrap();
    assert_eq!(repaired.problem, None);
    assert_eq!(
        store.update("broken", &damaged.fingerprint, markdown("", "Stale.")),
        Err(PromptStoreError::Conflict)
    );
    assert_eq!(
        super::super::discover(store.dir.parent().unwrap())
            .templates
            .len(),
        2
    );
}

#[test]
fn unreadable_files_are_reported_without_blocking_other_templates() {
    let store = PromptStore::temporary();
    fs::write(
        store.dir.join("large.md"),
        vec![b'x'; MAXIMUM_PROMPT_BODY_BYTES + 1],
    )
    .unwrap();
    fs::write(store.dir.join("binary.md"), [0xff]).unwrap();
    let valid = store.create("review", markdown("", "Review.")).unwrap();
    let listing = store.list().unwrap();
    assert_eq!(listing.records, vec![valid]);
    assert_eq!(
        listing.unavailable,
        vec![
            ("binary.md".to_owned(), PromptStoreError::Malformed),
            ("large.md".to_owned(), PromptStoreError::Bound),
        ]
    );
    assert_eq!(store.get("large"), Err(PromptStoreError::Bound));
}

#[test]
fn copied_case_collisions_are_not_offered_for_invocation() {
    let store = PromptStore::temporary();
    for name in ["review", "Review"] {
        fs::write(store.dir.join(format!("{name}.md")), "Review.").unwrap();
    }
    let listing = store.list().unwrap();
    assert_eq!(listing.records.len(), 2);
    assert!(
        listing
            .records
            .iter()
            .all(|record| record.problem == Some(PromptStoreError::Conflict))
    );
}

#[test]
fn creation_keeps_the_discovery_entry_bound() {
    let store = PromptStore::temporary();
    for index in 0..MAXIMUM_PROMPT_ENTRIES {
        fs::write(store.dir.join(format!("extra-{index}.txt")), "").unwrap();
    }
    assert_eq!(
        store.create("review", markdown("", "Review.")),
        Err(PromptStoreError::Full)
    );
    assert!(!store.dir.join("review.md").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_files_are_rejected_without_external_effects() {
    use std::os::unix::fs::symlink;
    let store = PromptStore::temporary();
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join("review.md");
    fs::write(&external, markdown("External.", "External.")).unwrap();
    symlink(&external, store.dir.join("review.md")).unwrap();
    assert_eq!(store.get("review"), Err(PromptStoreError::Unsafe));
    let listing = store.list().unwrap();
    assert!(listing.records.is_empty());
    assert_eq!(
        listing.unavailable,
        vec![("review.md".to_owned(), PromptStoreError::Unsafe)]
    );
    store.create("safe", markdown("", "Safe.")).unwrap();
    assert_eq!(
        store.create("review", markdown("", "New.")),
        Err(PromptStoreError::Unsafe)
    );
    assert_eq!(
        fs::read_to_string(&external).unwrap(),
        markdown("External.", "External.")
    );
}
