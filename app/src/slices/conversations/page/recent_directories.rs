use std::path::Path;

use sha2::{Digest, Sha256};

use crate::state::AppState;

use super::{DirectoryView, directory_view};

pub(super) struct RecentDirectoryView {
    pub(super) id: String,
    pub(super) recent_id: String,
    pub(super) directory: DirectoryView,
    pub(super) selected: bool,
    pub(super) command_start: bool,
    pub(super) disabled_reason: String,
}

pub(super) fn recent_directory_views(
    state: &AppState,
    selected: &[DirectoryView],
) -> Vec<RecentDirectoryView> {
    let mut choices = state
        .preferences
        .recent_directories()
        .into_iter()
        .filter_map(|entry| {
            let current = selected
                .iter()
                .find(|view| view.host_path == entry.host_path.to_string_lossy());
            let directory = match current {
                Some(view) => view.clone(),
                None => directory_view(&entry.grant()?),
            };
            let disabled_reason = if current.is_some() {
                String::new()
            } else if !directory.available {
                "Cannot access this saved path. Choose its current location again.".to_owned()
            } else if selected.len() >= crate::execution::MAXIMUM_DIRECTORY_GRANTS {
                "This conversation has the maximum of eight directories.".to_owned()
            } else if let Some(overlap) = selected.iter().find(|view| {
                let path = Path::new(&view.host_path);
                path.starts_with(&entry.host_path) || entry.host_path.starts_with(path)
            }) {
                format!("Overlaps {}.", overlap.host_path)
            } else {
                String::new()
            };
            Some(RecentDirectoryView {
                id: path_id(&directory.host_path),
                recent_id: entry.id,
                command_start: selected
                    .first()
                    .is_some_and(|first| first.id == directory.id && !directory.transient),
                directory,
                selected: current.is_some(),
                disabled_reason,
            })
        })
        .collect::<Vec<_>>();
    let mut other = selected
        .iter()
        .filter(|view| {
            !choices
                .iter()
                .any(|choice| choice.directory.host_path == view.host_path)
        })
        .collect::<Vec<_>>();
    other.sort_by(|left, right| left.host_path.cmp(&right.host_path));
    choices.extend(other.into_iter().map(|directory| {
        RecentDirectoryView {
            id: path_id(&directory.host_path),
            recent_id: String::new(),
            directory: directory.clone(),
            selected: true,
            command_start: selected
                .first()
                .is_some_and(|first| first.id == directory.id && !directory.transient),
            disabled_reason: String::new(),
        }
    }));
    choices
}

fn path_id(path: &str) -> String {
    crate::hex::encode(&Sha256::digest(path.as_bytes()))
}
