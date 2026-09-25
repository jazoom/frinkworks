//! Bounded file suggestions for the at-sign composer control.
//!
//! A suggestion contains a scope label and a model-visible path. It never
//! contains file contents. Search reads only approved host roots or an
//! immutable candidate view; it never starts a sandbox and never follows a
//! symbolic link or the private Frinkworks data directory.

use std::path::Path;

use askama::Template;
use serde::Serialize;

use crate::execution::resources::{
    EffectiveRoot, MAXIMUM_FILE_DEPTH, MAXIMUM_FILE_PATH_BYTES, MAXIMUM_FILE_QUERY_BYTES,
    MAXIMUM_FILE_RESULTS, MAXIMUM_FILE_TRAVERSAL_ENTRIES,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Suggestion {
    pub(super) path: String,
    pub(super) scope: String,
    pub(super) directory: bool,
}

#[derive(Default)]
pub(super) struct Search {
    pub(super) suggestions: Vec<Suggestion>,
    pub(super) partial: bool,
    pub(super) truncated: bool,
    pub(super) unavailable: usize,
    pub(super) foreign: bool,
}

/// Search every effective root within one traversal and result budget.
pub(super) fn search(
    roots: &[EffectiveRoot],
    query: &str,
    data_root: &Path,
    grants: &[crate::execution::DirectoryGrant],
) -> Search {
    let mut result = Search::default();
    let mut scored: Vec<(u32, Suggestion)> = Vec::new();
    let mut budget = 0usize;
    for root in roots {
        if !root.candidate() {
            if let Some(path) = root.host_path.as_deref() {
                let Some(grant) = grants.iter().find(|grant| grant.host_path == path) else {
                    result.unavailable += 1;
                    continue;
                };
                traverse(
                    grant,
                    root,
                    query,
                    data_root,
                    &mut budget,
                    &mut scored,
                    &mut result,
                );
            }
            if result.partial {
                break;
            }
            continue;
        }
        for entry in &root.candidate_paths {
            budget += 1;
            if budget > MAXIMUM_FILE_TRAVERSAL_ENTRIES {
                result.partial = true;
                break;
            }
            let Some((relative, directory)) = candidate_entry(entry) else {
                continue;
            };
            if grants.iter().any(|grant| {
                grant.alias == root.scope && overlaps(&grant.host_path.join(relative), data_root)
            }) {
                continue;
            }
            if let Some(score) = match_score(query, &root.scope, relative) {
                push_scored(&mut scored, score, root, relative, directory);
            }
        }
    }
    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.path.cmp(&right.1.path))
    });
    result.truncated = scored.len() > MAXIMUM_FILE_RESULTS;
    result.suggestions = scored
        .into_iter()
        .take(MAXIMUM_FILE_RESULTS)
        .map(|(_, suggestion)| suggestion)
        .collect();
    result
}

/// Absolute prefixes must match a selected model-visible root, not a host substitute.
pub(super) fn complete(
    roots: &[EffectiveRoot],
    query: &str,
    data_root: &Path,
    grants: &[crate::execution::DirectoryGrant],
) -> Result<Search, &'static str> {
    if query.len() > MAXIMUM_FILE_QUERY_BYTES {
        return Err("The file path is too long.");
    }
    if query.chars().any(char::is_control) {
        return Err("The file path is not valid.");
    }
    // A parent component can leave the selected root and is never completed.
    if query.split('/').any(|part| part == "..") {
        return Err("The file path is not valid.");
    }
    // Reject empty components so filesystem joins cannot reinterpret a relative suffix as absolute.
    if query.contains("//") {
        return Err("The file path is not valid.");
    }
    let mut result = Search::default();
    let mut budget = 0usize;
    if query.starts_with('/') {
        // Match the most specific selected root. A string prefix that does not
        // end on a path component must not select a different root.
        let mut matched: Option<(&EffectiveRoot, String)> = None;
        for root in roots {
            let model = root.model_path.trim_end_matches('/');
            let relative = if query == model {
                String::new()
            } else if let Some(rest) = query.strip_prefix(&format!("{model}/")) {
                rest.to_owned()
            } else {
                continue;
            };
            let more_specific = matched
                .as_ref()
                .is_none_or(|(current, _)| current.model_path.len() < root.model_path.len());
            if more_specific {
                matched = Some((root, relative));
            }
        }
        let Some((root, relative)) = matched else {
            result.foreign = true;
            return Ok(result);
        };
        complete_root(root, &relative, data_root, grants, &mut budget, &mut result);
    } else {
        for root in roots {
            complete_root(root, query, data_root, grants, &mut budget, &mut result);
        }
    }
    result.suggestions.sort_by(|left, right| {
        right
            .directory
            .cmp(&left.directory)
            .then_with(|| left.path.cmp(&right.path))
    });
    result
        .suggestions
        .dedup_by(|left, right| left.path == right.path);
    if result.suggestions.len() > MAXIMUM_FILE_RESULTS {
        result.truncated = true;
        result.suggestions.truncate(MAXIMUM_FILE_RESULTS);
    }
    Ok(result)
}

fn complete_root(
    root: &EffectiveRoot,
    relative: &str,
    data_root: &Path,
    grants: &[crate::execution::DirectoryGrant],
    budget: &mut usize,
    result: &mut Search,
) {
    let (directory, prefix) = match relative.rfind('/') {
        Some(index) => (&relative[..=index], &relative[index + 1..]),
        None => ("", relative),
    };
    if directory.split('/').any(|part| part == "." || part == "..") {
        return;
    }
    if root.candidate() {
        complete_candidate(root, directory, prefix, data_root, grants, budget, result);
    } else if let Some(path) = root.host_path.as_deref() {
        let Some(grant) = grants.iter().find(|grant| grant.host_path == path) else {
            result.unavailable += 1;
            return;
        };
        complete_host(grant, root, directory, prefix, data_root, budget, result);
    }
}

fn complete_candidate(
    root: &EffectiveRoot,
    directory: &str,
    prefix: &str,
    data_root: &Path,
    grants: &[crate::execution::DirectoryGrant],
    budget: &mut usize,
    result: &mut Search,
) {
    for entry in &root.candidate_paths {
        *budget += 1;
        if *budget > MAXIMUM_FILE_TRAVERSAL_ENTRIES {
            result.partial = true;
            return;
        }
        let Some((path, is_directory)) = candidate_entry(entry) else {
            continue;
        };
        let (entry_directory, name) = match path.rfind('/') {
            Some(index) => (&path[..=index], &path[index + 1..]),
            None => ("", path),
        };
        if entry_directory != directory || !name_starts_with(name, prefix) {
            continue;
        }
        if grants.iter().any(|grant| {
            grant.alias == root.scope && overlaps(&grant.host_path.join(path), data_root)
        }) {
            continue;
        }
        push_completion(result, root, directory, name, is_directory);
    }
}

fn complete_host(
    grant: &crate::execution::DirectoryGrant,
    root: &EffectiveRoot,
    directory: &str,
    prefix: &str,
    data_root: &Path,
    budget: &mut usize,
    result: &mut Search,
) {
    let root_path = &grant.host_path;
    let Ok(root_directory) =
        cap_std::fs::Dir::open_ambient_dir(root_path, cap_std::ambient_authority())
    else {
        result.unavailable += 1;
        return;
    };
    use cap_std::fs::MetadataExt;
    let identity_matches = root_directory.dir_metadata().is_ok_and(|metadata| {
        metadata.dev() == grant.identity.device && metadata.ino() == grant.identity.inode
    });
    if !identity_matches || grant.revalidate().is_err() {
        result.unavailable += 1;
        return;
    }
    let mut current = root_directory;
    let mut current_path = root_path.clone();
    for component in directory.trim_end_matches('/').split('/') {
        if component.is_empty() {
            continue;
        }
        if !valid_component(component) || component == ".git" {
            return;
        }
        current_path = current_path.join(component);
        // Private Frinkworks data stays out of previews even below a grant.
        if overlaps(&current_path, data_root) {
            return;
        }
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true);
        use cap_std::fs::OpenOptionsExt;
        // O_NOFOLLOW and O_DIRECTORY prevent traversal through a symlink or non-directory.
        options.custom_flags(0o200000 | 0o400000);
        match current.open_with(component, &options) {
            Ok(file) => current = cap_std::fs::Dir::from_std_file(file.into_std()),
            Err(_) => return,
        }
    }
    let Ok(entries) = current.entries() else {
        result.unavailable += 1;
        return;
    };
    for entry in entries {
        *budget += 1;
        if *budget > MAXIMUM_FILE_TRAVERSAL_ENTRIES {
            result.partial = true;
            return;
        }
        let Ok(entry) = entry else {
            result.unavailable += 1;
            continue;
        };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !valid_component(&name) || name == ".git" || !name_starts_with(&name, prefix) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let path = root_path.join(directory).join(&name);
            if overlaps(path.as_path(), data_root) {
                continue;
            }
            push_completion(result, root, directory, &name, true);
        } else if file_type.is_file() {
            push_completion(result, root, directory, &name, false);
        }
    }
}

fn name_starts_with(name: &str, prefix: &str) -> bool {
    prefix.is_empty() || name.to_lowercase().starts_with(&prefix.to_lowercase())
}

fn push_completion(
    result: &mut Search,
    root: &EffectiveRoot,
    directory: &str,
    name: &str,
    is_directory: bool,
) {
    let model_path = root.model_path.trim_end_matches('/');
    let path = if is_directory {
        format!("{model_path}/{directory}{name}/")
    } else {
        format!("{model_path}/{directory}{name}")
    };
    result.suggestions.push(Suggestion {
        path,
        scope: root.scope.clone(),
        directory: is_directory,
    });
}

fn traverse(
    grant: &crate::execution::DirectoryGrant,
    root: &EffectiveRoot,
    query: &str,
    data_root: &Path,
    budget: &mut usize,
    scored: &mut Vec<(u32, Suggestion)>,
    result: &mut Search,
) {
    let root_path = &grant.host_path;
    let Ok(root_directory) =
        cap_std::fs::Dir::open_ambient_dir(root_path, cap_std::ambient_authority())
    else {
        result.unavailable += 1;
        return;
    };
    use cap_std::fs::MetadataExt;
    let identity_matches = root_directory.dir_metadata().is_ok_and(|metadata| {
        metadata.dev() == grant.identity.device && metadata.ino() == grant.identity.inode
    });
    if !identity_matches || grant.revalidate().is_err() {
        result.unavailable += 1;
        return;
    }
    let mut stack = vec![(root_directory, String::new(), 0usize)];
    while let Some((directory, relative, depth)) = stack.pop() {
        if depth > MAXIMUM_FILE_DEPTH {
            result.partial = true;
            continue;
        }
        let entries = match directory.entries() {
            Ok(entries) => entries,
            Err(_) => {
                result.unavailable += 1;
                continue;
            }
        };
        for entry in entries {
            *budget += 1;
            if *budget > MAXIMUM_FILE_TRAVERSAL_ENTRIES {
                result.partial = true;
                return;
            }
            let Ok(entry) = entry else {
                result.unavailable += 1;
                continue;
            };
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !valid_component(&name) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // A symbolic link can leave the authorised root and is never read.
            if file_type.is_symlink() {
                continue;
            }
            let child = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if child.len() > MAXIMUM_FILE_PATH_BYTES || name == ".git" {
                continue;
            }
            if file_type.is_dir() {
                let path = root_path.join(&child);
                if overlaps(path.as_path(), data_root) {
                    continue;
                }
                let mut options = cap_std::fs::OpenOptions::new();
                options.read(true);
                use cap_std::fs::OpenOptionsExt;
                // Do not follow a directory replaced by a link after file_type.
                options.custom_flags(0o200000 | 0o400000);
                match entry.open_with(&options) {
                    Ok(file) => stack.push((
                        cap_std::fs::Dir::from_std_file(file.into_std()),
                        child,
                        depth + 1,
                    )),
                    Err(_) => result.unavailable += 1,
                }
            } else if file_type.is_file()
                && let Some(score) = match_score(query, &root.scope, &child)
            {
                push_scored(scored, score, root, &child, false);
            }
        }
    }
}

fn candidate_entry(entry: &str) -> Option<(&str, bool)> {
    let directory = entry.ends_with('/');
    let entry = entry.trim_end_matches('/');
    if entry.is_empty() || entry.len() > MAXIMUM_FILE_PATH_BYTES || entry.contains('\0') {
        return None;
    }
    if entry.starts_with('/') || entry.chars().any(char::is_control) {
        return None;
    }
    for component in entry.split('/') {
        if component.is_empty() || component == "." || component == ".." || component == ".git" {
            return None;
        }
    }
    Some((entry, directory))
}

fn valid_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('\0')
        && !name.chars().any(char::is_control)
}

fn overlaps(path: &Path, data_root: &Path) -> bool {
    path == data_root || path.starts_with(data_root)
}

fn match_score(query: &str, scope: &str, relative: &str) -> Option<u32> {
    let direct = score(query, relative);
    let scoped = score(query, &format!("{scope}/{relative}"));
    direct.max(scoped)
}

fn score(query: &str, candidate: &str) -> Option<u32> {
    let query = query.to_lowercase();
    let candidate = candidate.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if candidate == query {
        return Some(400);
    }
    if candidate.ends_with(&query) {
        return Some(300);
    }
    if candidate.contains(&query) {
        return Some(200);
    }
    // A subsequence match keeps unrelated results out while tolerating skipped
    // characters. The gap count orders closer matches first.
    let mut characters = candidate.chars();
    let mut gaps = 0u32;
    for wanted in query.chars() {
        loop {
            match characters.next() {
                Some(found) if found == wanted => break,
                Some(_) => gaps += 1,
                None => return None,
            }
        }
    }
    Some(100u32.saturating_sub(gaps.min(99)))
}

fn push_scored(
    scored: &mut Vec<(u32, Suggestion)>,
    score: u32,
    root: &EffectiveRoot,
    relative: &str,
    directory: bool,
) {
    scored.push((
        score,
        Suggestion {
            path: format!("{}/{}", root.model_path.trim_end_matches('/'), relative),
            scope: root.scope.clone(),
            directory,
        },
    ));
}

#[derive(Serialize)]
pub(super) struct SuggestionsPayload {
    pub(super) suggestions: Vec<SuggestionPayload>,
    pub(super) partial: bool,
    pub(super) truncated: bool,
    pub(super) unavailable: usize,
    pub(super) message: String,
}

#[derive(Serialize)]
pub(super) struct SuggestionPayload {
    pub(super) path: String,
    pub(super) scope: String,
    pub(super) directory: bool,
}

pub(super) struct SuggestionsView {
    pub(super) suggestions: Vec<SuggestionRow>,
    pub(super) message: String,
}

pub(super) struct SuggestionRow {
    pub(super) path: String,
    pub(super) display: String,
    pub(super) scope: String,
    pub(super) directory: bool,
}

#[derive(Template)]
#[template(path = "conversations/files/templates/index.html")]
pub(super) struct SuggestionsTemplate<'a> {
    pub(super) view: &'a SuggestionsView,
}

pub(super) fn payload(search: &Search, message: &str) -> SuggestionsPayload {
    SuggestionsPayload {
        suggestions: search
            .suggestions
            .iter()
            .map(|suggestion| SuggestionPayload {
                path: suggestion.path.clone(),
                scope: suggestion.scope.clone(),
                directory: suggestion.directory,
            })
            .collect(),
        partial: search.partial,
        truncated: search.truncated,
        unavailable: search.unavailable,
        message: message.to_owned(),
    }
}

pub(super) fn view(search: &Search, message: &str) -> SuggestionsView {
    SuggestionsView {
        suggestions: search
            .suggestions
            .iter()
            .map(|suggestion| SuggestionRow {
                display: if suggestion.directory && !suggestion.path.ends_with('/') {
                    format!("{}/", suggestion.path)
                } else {
                    suggestion.path.clone()
                },
                path: suggestion.path.clone(),
                scope: suggestion.scope.clone(),
                directory: suggestion.directory,
            })
            .collect(),
        message: message.to_owned(),
    }
}

pub(super) fn explanation(search: &Search, restricted: bool, no_roots: bool) -> &'static str {
    if no_roots {
        return "No authorised directory is available for file search.";
    }
    if search.foreign {
        return "That path is outside the authorised directories.";
    }
    if search.partial {
        return "The search reached its work limit. Results are partial.";
    }
    if restricted {
        return "Some directories are unavailable or need approval before their files appear.";
    }
    if search.unavailable > 0 {
        return "Some directories could not be read.";
    }
    if search.truncated {
        return "Showing the closest matches.";
    }
    ""
}
