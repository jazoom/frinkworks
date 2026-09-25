use askama::Template;
use serde::Deserialize;

use crate::conversations::prompts::{PromptRecord, PromptStore, PromptStoreError};

pub(super) const TITLE: &str = "Prompts | Frinkworks";
const NEW_PROMPT: &str =
    "State the task for the model.\n\nUse $1 for the first argument and $@ for every argument.\n";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Editor {
    pub(super) name: String,
    pub(super) fingerprint: String,
    pub(super) description: String,
    pub(super) argument_hint: String,
    pub(super) body: String,
    #[serde(skip)]
    pub(super) problem: Option<PromptStoreError>,
}

impl Editor {
    pub(super) fn new() -> Self {
        Self {
            body: NEW_PROMPT.to_owned(),
            ..Self::default()
        }
    }
}

impl From<PromptRecord> for Editor {
    fn from(record: PromptRecord) -> Self {
        Self {
            name: record.name,
            fingerprint: record.fingerprint,
            description: record.description,
            argument_hint: record.argument_hint,
            body: record.body,
            problem: record.problem,
        }
    }
}

#[derive(Template)]
#[template(path = "prompts/templates/index.html")]
pub(super) struct PromptsPage {
    pub(super) rows: Vec<PromptRecord>,
    pub(super) unavailable: Vec<String>,
    pub(super) global_path: String,
    pub(super) error: String,
    pub(super) editor: Option<Editor>,
}

impl PromptsPage {
    pub(super) fn new(store: &PromptStore, editor: Option<Editor>, mut error: String) -> Self {
        let listing = match store.list() {
            Ok(listing) => listing,
            Err(problem) => {
                if !error.is_empty() {
                    error.push(' ');
                }
                error.push_str(problem.message());
                Default::default()
            }
        };
        Self {
            rows: listing.records,
            unavailable: listing
                .unavailable
                .into_iter()
                .map(|(file, problem)| format!("{file}: {}", problem.message()))
                .collect(),
            global_path: store.host_dir().display().to_string(),
            error,
            editor,
        }
    }
}
