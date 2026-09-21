use askama::Template;
use serde::Deserialize;

use crate::skills::{SkillRecord, SkillStore};

pub(super) const TITLE: &str = "Skills | Power Plant";
const NEW_SKILL: &str = "---\nname: my-skill\ndescription: State what this skill does and when the model must use it.\n---\n\n# My skill\n\n## When to use\n\nDescribe the tasks that need this skill.\n\n## Instructions\n\n1. Write the first instruction.\n";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Editor {
    pub(super) directory: String,
    pub(super) fingerprint: String,
    pub(super) markdown: String,
}

impl Editor {
    pub(super) fn new() -> Self {
        Self {
            markdown: NEW_SKILL.to_owned(),
            ..Self::default()
        }
    }
}

impl From<SkillRecord> for Editor {
    fn from(record: SkillRecord) -> Self {
        Self {
            directory: record.directory,
            fingerprint: record.fingerprint,
            markdown: record.markdown,
        }
    }
}

#[derive(Template)]
#[template(path = "skills/templates/index.html")]
pub(super) struct SkillsPage {
    pub(super) rows: Vec<SkillRecord>,
    pub(super) global_path: String,
    pub(super) error: String,
    pub(super) editor: Option<Editor>,
}

impl SkillsPage {
    pub(super) fn new(store: &SkillStore, editor: Option<Editor>, mut error: String) -> Self {
        let rows = match store.list() {
            Ok(rows) => rows,
            Err(problem) => {
                if !error.is_empty() {
                    error.push(' ');
                }
                error.push_str(problem.message());
                Vec::new()
            }
        };
        Self {
            rows,
            global_path: store
                .host_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            error,
            editor,
        }
    }
}
