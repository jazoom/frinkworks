use serde::Deserialize;

use super::MAXIMUM_WRITE_BYTES;

pub(crate) const MAXIMUM_EDIT_COUNT: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct Change {
    pub search: String,
    pub replace: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EditError {
    EmptyBatch,
    TooMany,
    EmptySearch,
    InvalidText,
    OversizedInput,
    OversizedResult,
    NotFound,
    Ambiguous,
    Overlap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Span {
    start: usize,
    end: usize,
    replace: usize,
}

impl EditError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::EmptyBatch => "Enter at least one edit.",
            Self::TooMany => "That edit batch is too large.",
            Self::EmptySearch => "Search text cannot be empty.",
            Self::InvalidText => "Those edits contain invalid text.",
            Self::OversizedInput => "Those edits are too large.",
            Self::OversizedResult => "The edited file exceeds the size limit.",
            Self::NotFound => "Search text was not found.",
            Self::Ambiguous => "Search text must occur exactly once.",
            Self::Overlap => "Those edits overlap. Apply each change against the original file.",
        }
    }
}

pub(crate) fn apply(original: &str, edits: &[Change]) -> Result<String, EditError> {
    if edits.is_empty() {
        return Err(EditError::EmptyBatch);
    }
    if edits.len() > MAXIMUM_EDIT_COUNT {
        return Err(EditError::TooMany);
    }
    if original.len() > MAXIMUM_WRITE_BYTES {
        return Err(EditError::OversizedInput);
    }
    if invalid_edit_text(original) {
        return Err(EditError::InvalidText);
    }
    let mut spans = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        validate_change(edit)?;
        let range = unique_range(original, &edit.search)?;
        spans.push(Span {
            start: range.start,
            end: range.end,
            replace: index,
        });
    }
    spans.sort_by_key(|span| span.start);
    for pair in spans.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(EditError::Overlap);
        }
    }
    let mut output = String::new();
    let mut cursor = 0usize;
    for span in spans {
        output.push_str(&original[cursor..span.start]);
        output.push_str(&edits[span.replace].replace);
        if output.len() > MAXIMUM_WRITE_BYTES {
            return Err(EditError::OversizedResult);
        }
        cursor = span.end;
    }
    output.push_str(&original[cursor..]);
    if output.len() > MAXIMUM_WRITE_BYTES {
        return Err(EditError::OversizedResult);
    }
    Ok(output)
}

fn validate_change(edit: &Change) -> Result<(), EditError> {
    if edit.search.is_empty() {
        return Err(EditError::EmptySearch);
    }
    if edit.search.len() > MAXIMUM_WRITE_BYTES || edit.replace.len() > MAXIMUM_WRITE_BYTES {
        return Err(EditError::OversizedInput);
    }
    if invalid_edit_text(&edit.search) || invalid_edit_text(&edit.replace) {
        return Err(EditError::InvalidText);
    }
    Ok(())
}

fn invalid_edit_text(text: &str) -> bool {
    text.chars().any(|character| {
        character == '\0' || (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    })
}

fn unique_range(original: &str, search: &str) -> Result<std::ops::Range<usize>, EditError> {
    let Some(start) = original.find(search) else {
        return Err(EditError::NotFound);
    };
    let end = start + search.len();
    let after_first = start
        + original[start..]
            .chars()
            .next()
            .expect("non-empty search")
            .len_utf8();
    if original[after_first..].contains(search) {
        return Err(EditError::Ambiguous);
    }
    Ok(start..end)
}

#[cfg(test)]
mod tests;
