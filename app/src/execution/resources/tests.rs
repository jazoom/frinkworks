use super::{ResourceError, ResourceKind, ResourceSource, parse_skill_metadata};

#[test]
fn metadata_rejects_ambiguous_fields_and_bounds_only_the_header() {
    for bytes in [
        b"no frontmatter\n".as_slice(),
        b"---\nname: alpha\n",
        b"---\ndescription: only\n---\n",
        b"---\nname: alpha\nname: beta\ndescription: work\n---\n",
        b"---\nname: alpha\ndescription: 'unclosed\n---\n",
        b"---\nname: alpha\ndescription: [one, two]\n---\n",
    ] {
        assert!(parse_skill_metadata(bytes).is_err());
    }
    let mut bytes = b"---\nname: alpha\ndescription: work\n---\n".to_vec();
    bytes.extend(std::iter::repeat_n(b'x', 8192));
    assert_eq!(
        parse_skill_metadata(&bytes)
            .expect("bounded header")
            .description,
        "work"
    );
    let oversized = format!(
        "---\nname: alpha\ndescription: {}\n---\n",
        "x".repeat(super::MAXIMUM_SKILL_DESCRIPTION_BYTES + 1)
    );
    assert!(parse_skill_metadata(oversized.as_bytes()).is_err());
}

#[test]
fn body_reads_reject_changed_identity_and_credentials() {
    let path = "/project/.agents/skills/alpha/SKILL.md";
    let original = ResourceSource::new(ResourceKind::Skill, "project", path, b"original");
    original
        .validate_read(b"original", std::slice::from_ref(&original), None)
        .expect("same source");
    let changed = ResourceSource::new(ResourceKind::Skill, "project", path, b"changed");
    assert_eq!(
        changed.validate_read(b"changed", &[original], None),
        Err(ResourceError::Changed)
    );
    for (scope, path, bytes) in [
        ("sk-secret", path, b"safe".as_slice()),
        (
            "project",
            "/project/sk-secret/AGENTS.md",
            b"safe".as_slice(),
        ),
        ("project", path, b"sk-secret".as_slice()),
    ] {
        let source = ResourceSource::new(ResourceKind::Skill, scope, path, bytes);
        assert_eq!(
            source.validate_read(bytes, &[], Some("sk-secret")),
            Err(ResourceError::Credential)
        );
    }
}

fn shell(root: &std::path::Path, script: &str, args: &[&str]) -> std::process::Output {
    std::process::Command::new("sh")
        .args(["-c", script, "resource-test"])
        .args(args)
        .current_dir(root)
        .output()
        .expect("resource command")
}

#[test]
fn discovery_rejects_links_at_every_component() {
    use std::os::unix::fs::symlink;
    for component in [
        ".agents",
        ".agents/skills",
        ".agents/skills/alpha",
        ".agents/skills/alpha/SKILL.md",
    ] {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let link = root.path().join(component);
        std::fs::create_dir_all(link.parent().expect("parent")).expect("parents");
        symlink(outside.path(), &link).expect("link");
        let listed = shell(root.path(), super::SKILL_LIST_COMMAND, &[]);
        assert_eq!(listed.status.code(), Some(6), "{component}");
        assert!(listed.stdout.is_empty());
        let read = shell(
            root.path(),
            super::SKILL_READ_COMMAND,
            &[".agents/skills/alpha/SKILL.md"],
        );
        assert_eq!(read.status.code(), Some(6), "{component}");
        assert!(read.stdout.is_empty());
    }
}

#[test]
fn discovery_bounds_entries_and_returns_the_full_body_hash() {
    let root = tempfile::tempdir().expect("root");
    let directory = root.path().join(".agents/skills/alpha");
    std::fs::create_dir_all(&directory).expect("skill");
    let bytes = format!(
        "---\nname: alpha\ndescription: work\n---\n{}",
        "x".repeat(8192)
    );
    std::fs::write(directory.join("SKILL.md"), &bytes).expect("body");
    let listed = shell(root.path(), super::SKILL_LIST_COMMAND, &[]);
    assert!(listed.status.success());
    assert_eq!(listed.stdout, b".agents/skills/alpha/SKILL.md\0");
    let read = shell(
        root.path(),
        super::SKILL_READ_COMMAND,
        &[".agents/skills/alpha/SKILL.md"],
    );
    assert!(read.status.success());
    let digest = super::content_hash(bytes.as_bytes());
    assert!(read.stdout.starts_with(&digest.as_bytes()[7..]));
    assert!(read.stdout.len() <= super::MAXIMUM_SKILL_METADATA_BYTES + 68);
    for index in 0..64 {
        std::fs::create_dir(root.path().join(format!(".agents/skills/item-{index}")))
            .expect("entry");
    }
    let listed = shell(root.path(), super::SKILL_LIST_COMMAND, &[]);
    assert_eq!(listed.status.code(), Some(7));
}

#[test]
fn effective_roots_skip_the_private_data_directory() {
    let parent = tempfile::tempdir().expect("parent");
    let data_root = parent.path().join("frinkworks-data");
    std::fs::create_dir_all(&data_root).expect("data root");
    let project = parent.path().join("project");
    std::fs::create_dir_all(&project).expect("project");
    let grant = crate::execution::DirectoryGrant::from_selected(&project, &[]).expect("grant");
    let policy = crate::agents::DirectoryPolicy::from_grants(
        vec![crate::agents::PolicyGrant {
            alias: grant.alias.clone(),
            guest_path: grant.guest_path(),
            host_path: grant.host_path.clone(),
            access: crate::agents::AccessMode::ReadOnly,
        }],
        grant.alias.clone(),
    );
    let inside =
        crate::execution::DirectoryGrant::from_selected(&data_root, &[]).expect("data grant");
    let mut grants = vec![crate::agents::PolicyGrant {
        alias: inside.alias.clone(),
        guest_path: inside.guest_path(),
        host_path: inside.host_path.clone(),
        access: crate::agents::AccessMode::ReadOnly,
    }];
    grants.extend(policy.grants().iter().cloned());
    let combined = crate::agents::DirectoryPolicy::from_grants(grants, grant.alias.clone());
    let roots = super::effective_roots(&combined, &data_root);
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].scope, grant.alias);
}

#[test]
fn effective_roots_use_live_host_paths() {
    let root = tempfile::tempdir().expect("root");
    std::fs::write(root.path().join("host.txt"), b"host").expect("host file");
    let grant = crate::execution::DirectoryGrant::from_selected(root.path(), &[]).expect("grant");
    let policy = crate::agents::DirectoryPolicy::from_grants(
        vec![crate::agents::PolicyGrant {
            alias: grant.alias.clone(),
            guest_path: grant.guest_path(),
            host_path: grant.host_path.clone(),
            access: crate::agents::AccessMode::ReadOnly,
        }],
        grant.alias.clone(),
    );
    let roots = super::effective_roots(&policy, std::path::Path::new("/nonexistent"));
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].host_path.as_deref(), Some(root.path()));
}
