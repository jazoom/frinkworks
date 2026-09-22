use std::path::Path;

use super::*;

fn write_template(directory: &Path, name: &str, markdown: &str) {
    std::fs::create_dir_all(directory).expect("prompts directory");
    std::fs::write(directory.join(format!("{name}.md")), markdown).expect("template file");
}

#[test]
fn frontmatter_is_optional_and_carries_description_and_hint() {
    let document = parse_document(
        "---\ndescription: Review a change.\nargument-hint: <path>\n---\n\nBody $1.\n",
    )
    .expect("parse");
    assert_eq!(document.description, "Review a change.");
    assert_eq!(document.argument_hint, "<path>");
    assert_eq!(document.body, "Body $1.");

    let plain = parse_document("Plain body.").expect("parse");
    assert_eq!(plain.description, "");
    assert_eq!(plain.argument_hint, "");
    assert_eq!(plain.body, "Plain body.");
}

#[test]
fn malformed_frontmatter_is_rejected() {
    assert_eq!(
        parse_document("---\ndescription: [unclosed\n---\nBody.").unwrap_err(),
        PromptError::Malformed
    );
    assert_eq!(
        parse_document("---\ndescription: Missing close\nBody.").unwrap_err(),
        PromptError::Malformed
    );
    assert_eq!(
        parse_document("---\ndescription: Only frontmatter\n---\n").unwrap_err(),
        PromptError::Malformed
    );
    assert_eq!(
        parse_document("Body with \0 a nul.").unwrap_err(),
        PromptError::Malformed
    );
}

#[test]
fn positional_and_all_argument_forms_render() {
    let rendered = |body: &str| render(body, "alpha beta gamma").expect("render");
    assert_eq!(rendered("$1-$2-$3-$4"), "alpha-beta-gamma-");
    assert_eq!(rendered("$@"), "alpha beta gamma");
    assert_eq!(rendered("$ARGUMENTS"), "alpha beta gamma");
    assert_eq!(rendered("${1:-fallback} ${4:-fallback}"), "alpha fallback");
    assert_eq!(rendered("${@:-fallback}"), "alpha beta gamma");
    assert_eq!(rendered("${ARGUMENTS:-fallback}"), "alpha beta gamma");
    assert_eq!(rendered("${@:2}"), "beta gamma");
    assert_eq!(rendered("${@:2:1}"), "beta");
    assert_eq!(rendered("${@:4}"), "");
    assert_eq!(rendered("${@:1:0}"), "");
    assert_eq!(
        render("${1:-fallback}", "\"\"").expect("render"),
        "fallback"
    );
}

#[test]
fn quoted_groups_are_single_arguments_without_interpolation() {
    let rendered = render("[${1}][${2}]", "  \"one two\"  'three four'  ").expect("render");
    assert_eq!(rendered, "[one two][three four]");
    let escaped = render("[$1]", "\"a \\\"quote\\\"\"").expect("render");
    assert_eq!(escaped, "[a \"quote\"]");
    let dollar = render("[$1]", "'$(touch /tmp/x) `id`'").expect("render");
    assert_eq!(dollar, "[$(touch /tmp/x) `id`]");
}

#[test]
fn substitution_is_non_recursive_and_non_executable() {
    let rendered = render("$1", "$2").expect("render");
    assert_eq!(rendered, "$2");
    let nested = render("$1", "${2}").expect("render");
    assert_eq!(nested, "${2}");
    let command = render("cost $(id) and `whoami`", "").expect("render");
    assert_eq!(command, "cost $(id) and `whoami`");
    assert_eq!(render("price \\$5", "ignored").expect("render"), "price $5");
}

#[test]
fn malformed_arguments_are_rejected() {
    assert_eq!(
        render("$1", "\"unterminated").unwrap_err(),
        PromptError::Malformed
    );
    assert_eq!(
        render("$1", "trailing\\").unwrap_err(),
        PromptError::Malformed
    );
    assert_eq!(render("${@:0}", "").unwrap_err(), PromptError::Malformed);
    assert_eq!(render("${1:2}", "").unwrap_err(), PromptError::Malformed);
    assert_eq!(render("${nope}", "").unwrap_err(), PromptError::Malformed);
    assert_eq!(render("${", "").unwrap_err(), PromptError::Malformed);
}

#[test]
fn argument_count_and_output_size_stay_bounded() {
    let too_many = vec!["a"; MAXIMUM_PROMPT_ARGUMENTS + 1].join(" ");
    assert_eq!(render("$@", &too_many).unwrap_err(), PromptError::Bound);
    let blow_up = "$1".repeat(64);
    let argument = "x".repeat(super::super::MAXIMUM_MESSAGE_BYTES);
    assert_eq!(render(&blow_up, &argument).unwrap_err(), PromptError::Bound);
}

#[test]
fn names_are_bounded_and_reserved_names_are_rejected() {
    assert!(valid_name("review"));
    assert!(valid_name("review-2.1"));
    assert!(!valid_name(""));
    assert!(!valid_name(" leading"));
    assert!(!valid_name("with space"));
    assert!(!valid_name("slash/name"));
    assert!(!valid_name(&"a".repeat(MAXIMUM_PROMPT_NAME_BYTES + 1)));
    assert!(reserved_name("skill"));
    assert!(reserved_name("Skill"));
    assert!(!reserved_name("review"));
}

#[test]
fn absent_folder_produces_no_templates() {
    let root = tempfile::tempdir().expect("root");
    let catalogue = discover(root.path());
    assert!(catalogue.templates.is_empty());
    assert_eq!(catalogue.unavailable, 0);
}

#[test]
fn discovery_reads_valid_files_and_reports_bad_entries() {
    let root = tempfile::tempdir().expect("root");
    let directory = root.path().join(PROMPTS_DIRECTORY);
    write_template(
        &directory,
        "review",
        "---\ndescription: Review.\nargument-hint: <path>\n---\nReview $1.",
    );
    write_template(&directory, "plain", "Plain body.");
    write_template(&directory, "skill", "Reserved name.");
    write_template(&directory, "Bad Name", "Space in name.");
    write_template(&directory, "empty", "---\ndescription: Empty body\n---\n");
    write_template(
        &directory,
        "broken",
        "---\ndescription: [unclosed\n---\nBody.",
    );
    std::fs::write(directory.join("notes.txt"), "not markdown").expect("note");

    let catalogue = discover(root.path());
    let names = catalogue
        .templates
        .iter()
        .map(|template| template.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["plain", "review"]);
    assert_eq!(catalogue.unavailable, 5);
    let review = &catalogue.templates[1];
    assert_eq!(review.description, "Review.");
    assert_eq!(review.argument_hint, "<path>");
    assert_eq!(review.source.kind, ResourceKind::Prompt);
    assert_eq!(review.source.path, "prompts/review.md");
    assert_eq!(review.source.scope, PROMPTS_SCOPE);
}

#[cfg(unix)]
#[test]
fn discovery_rejects_symbolic_links_and_unusable_folders() {
    let root = tempfile::tempdir().expect("root");
    let outside = tempfile::NamedTempFile::new().expect("outside");
    std::fs::write(outside.path(), "Linked body.").expect("write");
    let directory = root.path().join(PROMPTS_DIRECTORY);
    std::fs::create_dir_all(&directory).expect("directory");
    std::os::unix::fs::symlink(outside.path(), directory.join("linked.md")).expect("link");
    let catalogue = discover(root.path());
    assert!(catalogue.templates.is_empty());
    assert_eq!(catalogue.unavailable, 1);

    let linked_root = tempfile::tempdir().expect("root");
    std::os::unix::fs::symlink(&directory, linked_root.path().join(PROMPTS_DIRECTORY))
        .expect("folder link");
    let catalogue = discover(linked_root.path());
    assert!(catalogue.templates.is_empty());
    assert_eq!(catalogue.unavailable, 1);
}

#[test]
fn discovery_rejects_every_case_insensitive_collision() {
    for names in [["Review", "review"], ["review", "Review"]] {
        let root = tempfile::tempdir().expect("root");
        let directory = root.path().join(PROMPTS_DIRECTORY);
        for name in names {
            write_template(&directory, name, "Review $1.");
        }
        write_template(&directory, "unique", "Unique body.");
        let catalogue = discover(root.path());
        assert_eq!(catalogue.templates.len(), 1);
        assert_eq!(catalogue.templates[0].name, "unique");
        assert_eq!(catalogue.unavailable, 2);
    }
}

#[test]
fn discovery_reports_an_oversized_file() {
    let root = tempfile::tempdir().expect("root");
    let directory = root.path().join(PROMPTS_DIRECTORY);
    write_template(
        &directory,
        "large",
        &"x".repeat(MAXIMUM_PROMPT_BODY_BYTES + 1),
    );
    let catalogue = discover(root.path());
    assert!(catalogue.templates.is_empty());
    assert_eq!(catalogue.unavailable, 1);
}

fn project_root(root: &Path, name: &str, body: &str) -> DirectoryGrant {
    let prompts = root.join(PROJECT_PROMPTS_DIRECTORY);
    std::fs::create_dir_all(&prompts).expect("prompts directory");
    std::fs::write(prompts.join(format!("{name}.md")), body).expect("project template");
    DirectoryGrant::from_selected(root, &[]).expect("grant")
}

fn effective(grant: &DirectoryGrant) -> EffectiveRoot {
    EffectiveRoot {
        scope: grant.alias.clone(),
        model_path: grant.guest_path(),
        host_path: Some(grant.host_path.clone()),
        candidate_paths: Vec::new(),
    }
}

#[test]
fn project_discovery_keeps_same_name_across_roots() {
    let left = tempfile::tempdir().expect("left");
    let right = tempfile::tempdir().expect("right");
    let left_grant = project_root(left.path(), "review", "Left $1.");
    let right_grant = project_root(right.path(), "review", "Right $1.");
    let data_root = tempfile::tempdir().expect("data");
    let (templates, unavailable) = discover_project(
        &[effective(&left_grant), effective(&right_grant)],
        &[left_grant, right_grant],
        data_root.path(),
    );
    assert_eq!(unavailable, 0);
    assert_eq!(templates.len(), 2);
    assert!(templates.iter().all(|template| template.name == "review"));
    assert_ne!(templates[0].source.scope, templates[1].source.scope);
    assert!(
        templates
            .iter()
            .all(|template| template.source.kind == ResourceKind::Prompt)
    );
    assert!(
        templates
            .iter()
            .all(|template| { template.source.path.contains(PROJECT_PROMPTS_DIRECTORY) })
    );
}

#[test]
fn global_project_alias_remains_distinct_from_global_templates() {
    let data = tempfile::tempdir().expect("data");
    std::fs::create_dir(data.path().join(PROMPTS_DIRECTORY)).expect("global prompts");
    std::fs::write(data.path().join("prompts/review.md"), "Global.").expect("global template");
    let parent = tempfile::tempdir().expect("project parent");
    let grant = project_root(&parent.path().join("global"), "review", "Project.");
    let (mut templates, _) = discover_project(&[effective(&grant)], &[grant], data.path());
    templates.extend(discover(data.path()).templates);
    for (command, body) in [
        ("/global/review", "Global."),
        ("/project:global/review", "Project."),
    ] {
        let expansion = crate::conversations::input::expand(command, &[], &templates, None)
            .expect("distinct scope");
        assert_eq!(expansion.expanded, body);
    }
}

#[test]
fn project_discovery_rejects_case_insensitive_collisions_within_one_root() {
    let root = tempfile::tempdir().expect("root");
    let prompts = root.path().join(PROJECT_PROMPTS_DIRECTORY);
    std::fs::create_dir_all(&prompts).expect("prompts directory");
    std::fs::write(prompts.join("Review.md"), "Review $1.").expect("first");
    std::fs::write(prompts.join("review.md"), "Review $1.").expect("second");
    std::fs::write(prompts.join("unique.md"), "Unique.").expect("unique");
    let grant = DirectoryGrant::from_selected(root.path(), &[]).expect("grant");
    let data_root = tempfile::tempdir().expect("data");
    let (templates, unavailable) =
        discover_project(&[effective(&grant)], &[grant], data_root.path());
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0].name, "unique");
    assert_eq!(unavailable, 2);
}

#[test]
fn candidate_backed_templates_report_unavailable_bodies() {
    let data_root = tempfile::tempdir().expect("data");
    let root = EffectiveRoot {
        scope: "project".to_owned(),
        model_path: "/access/project".to_owned(),
        host_path: None,
        candidate_paths: vec![
            ".agents/prompts/review.md".to_owned(),
            ".agents/prompts/review.md".to_owned(),
            ".agents/prompts/nested/other.md".to_owned(),
            "src/main.rs".to_owned(),
        ],
    };
    let (templates, unavailable) = discover_project(&[root], &[], data_root.path());
    assert!(templates.is_empty());
    assert_eq!(unavailable, 1);
}

#[test]
fn project_discovery_skips_the_private_data_directory() {
    let data_root = tempfile::tempdir().expect("data");
    let root = data_root.path().join("project");
    let grant = project_root(&root, "review", "Review $1.");
    let (templates, unavailable) =
        discover_project(&[effective(&grant)], &[grant], data_root.path());
    assert!(templates.is_empty());
    assert_eq!(unavailable, 0);
}

#[cfg(unix)]
#[test]
fn project_discovery_rejects_symbolic_links() {
    let root = tempfile::tempdir().expect("root");
    let outside = tempfile::NamedTempFile::new().expect("outside");
    let prompts = root.path().join(PROJECT_PROMPTS_DIRECTORY);
    std::fs::create_dir_all(&prompts).expect("prompts directory");
    std::os::unix::fs::symlink(outside.path(), prompts.join("linked.md")).expect("link");
    let grant = DirectoryGrant::from_selected(root.path(), &[]).expect("grant");
    let data_root = tempfile::tempdir().expect("data");
    let (templates, unavailable) =
        discover_project(&[effective(&grant)], &[grant], data_root.path());
    assert!(templates.is_empty());
    assert_eq!(unavailable, 1);
}

#[test]
fn compose_round_trips_through_the_parser_and_omits_empty_optional_fields() {
    let composed = compose("Review a change.", "<path>", "Review $1.");
    assert_eq!(
        composed,
        "---\ndescription: Review a change.\nargument-hint: <path>\n---\n\nReview $1.\n"
    );
    let document = parse_document(&composed).expect("parse");
    assert_eq!(document.description, "Review a change.");
    assert_eq!(document.argument_hint, "<path>");
    assert_eq!(document.body, "Review $1.");

    assert_eq!(compose("", "", "Plain body."), "Plain body.\n");
    let plain = parse_document(&compose("", "", "Plain body.")).expect("parse");
    assert_eq!(plain.description, "");
    assert_eq!(plain.argument_hint, "");
    assert_eq!(plain.body, "Plain body.");
}
