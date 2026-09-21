use crate::execution::command::CommandChunk;

pub(crate) const DEFAULT_PAGE_LINES: usize = 250;
pub(crate) const MAXIMUM_PAGE_LINES: usize = 2_000;
// Reserve space for continuation and truncation notices in the model result.
pub(crate) const MAXIMUM_PAGE_BYTES: usize = super::MAXIMUM_TOOL_BYTES - 1024;
pub(crate) const MAXIMUM_SCAN_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PageRequest {
    pub offset: u64,
    pub limit: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextPage {
    pub text: String,
    pub next: Option<u64>,
    pub line_truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PageError {
    InvalidOffset,
    InvalidLimit,
    PastEnd,
    Binary,
    InvalidText,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LineSpan {
    start: usize,
    end: usize,
    next: Option<u64>,
    line_truncated: bool,
}

impl PageError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::InvalidOffset => "Read offset must be a 1-based line number.",
            Self::InvalidLimit => "Read limit must be a positive line count.",
            Self::PastEnd => "That offset is past the end of the file.",
            Self::Binary => "That file contains binary content.",
            Self::InvalidText => "That file is not valid text.",
        }
    }
}

pub(crate) fn parse_request(
    offset: Option<u64>,
    limit: Option<u64>,
) -> Result<PageRequest, PageError> {
    let offset = offset.unwrap_or(1);
    if offset == 0 {
        return Err(PageError::InvalidOffset);
    }
    let limit = match limit {
        None => DEFAULT_PAGE_LINES,
        Some(0) => return Err(PageError::InvalidLimit),
        Some(value) => usize::try_from(value)
            .unwrap_or(MAXIMUM_PAGE_LINES)
            .min(MAXIMUM_PAGE_LINES),
    };
    Ok(PageRequest { offset, limit })
}

pub(crate) fn decode_text(bytes: &[u8]) -> Result<&str, PageError> {
    if bytes.contains(&0) {
        return Err(PageError::Binary);
    }
    std::str::from_utf8(bytes).map_err(|_| PageError::InvalidText)
}

pub(crate) fn page_text(text: &str, request: PageRequest) -> Result<TextPage, PageError> {
    let span = select_lines(text, request, MAXIMUM_PAGE_BYTES)?;
    Ok(TextPage {
        text: text[span.start..span.end].to_owned(),
        next: span.next,
        line_truncated: span.line_truncated,
    })
}

pub(crate) fn page_chunks(
    chunks: &[CommandChunk],
    request: PageRequest,
) -> Result<(Vec<CommandChunk>, Option<u64>, bool), PageError> {
    let mut flat = String::new();
    for chunk in chunks {
        flat.push_str(&chunk.text);
    }
    let mut budget = MAXIMUM_PAGE_BYTES;
    loop {
        let span = select_lines(&flat, request, budget)?;
        let chunks = slice_chunks(chunks, span.start, span.end);
        // Stream labels also consume the model result's byte allowance.
        let rendered_bytes: usize = chunks
            .iter()
            .map(|chunk| chunk.text.len() + chunk.stream.label().len() + 3)
            .sum();
        if rendered_bytes <= MAXIMUM_PAGE_BYTES {
            return Ok((chunks, span.next, span.line_truncated));
        }
        budget /= 2;
    }
}

pub(crate) fn render_file_page(page: &TextPage) -> String {
    if page.text.is_empty() && page.next.is_none() && !page.line_truncated {
        return "The file is empty.".to_owned();
    }
    let mut output = page.text.clone();
    if page.line_truncated {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("The last included line exceeded the byte limit.");
    }
    if let Some(next) = page.next {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&format!(
            "More content remains. Read again with offset={next}."
        ));
    }
    output
}

fn select_lines(text: &str, request: PageRequest, max_bytes: usize) -> Result<LineSpan, PageError> {
    if text.is_empty() {
        return if request.offset == 1 {
            Ok(LineSpan {
                start: 0,
                end: 0,
                next: None,
                line_truncated: false,
            })
        } else {
            Err(PageError::PastEnd)
        };
    }

    let mut line_no = 1u64;
    let mut index = 0usize;
    let mut start = None;
    let mut end = 0usize;
    let mut remaining_lines = request.limit;
    let mut remaining_bytes = max_bytes;
    let mut line_truncated = false;
    let mut next = None;

    while index < text.len() {
        let line_end = match text[index..].find('\n') {
            Some(relative) => index + relative + 1,
            None => text.len(),
        };
        let line_len = line_end - index;
        if line_no < request.offset {
            index = line_end;
            line_no = match line_no.checked_add(1) {
                Some(value) => value,
                None => return Err(PageError::PastEnd),
            };
            continue;
        }
        if start.is_none() {
            start = Some(index);
        }
        if remaining_lines == 0 {
            next = Some(line_no);
            break;
        }
        if line_len <= remaining_bytes {
            end = line_end;
            remaining_bytes -= line_len;
            remaining_lines -= 1;
            index = line_end;
            line_no = match line_no.checked_add(1) {
                Some(value) => value,
                None => return Err(PageError::PastEnd),
            };
            continue;
        }
        if remaining_bytes == max_bytes {
            let mut take = remaining_bytes;
            while take > 0 && !text[index..line_end].is_char_boundary(take) {
                take -= 1;
            }
            end = index + take;
            line_truncated = true;
            next = (line_end < text.len()).then(|| line_no.saturating_add(1));
            break;
        }
        next = Some(line_no);
        break;
    }

    let Some(start) = start else {
        return Err(PageError::PastEnd);
    };
    Ok(LineSpan {
        start,
        end,
        next,
        line_truncated,
    })
}

fn slice_chunks(chunks: &[CommandChunk], start: usize, end: usize) -> Vec<CommandChunk> {
    let mut consumed = 0usize;
    let mut sliced = Vec::new();
    for chunk in chunks {
        let chunk_end = consumed + chunk.text.len();
        if chunk_end <= start {
            consumed = chunk_end;
            continue;
        }
        if consumed >= end {
            break;
        }
        let from = start.saturating_sub(consumed);
        let to = end.saturating_sub(consumed).min(chunk.text.len());
        if from < to {
            sliced.push(CommandChunk {
                stream: chunk.stream,
                text: chunk.text[from..to].to_owned(),
            });
        }
        consumed = chunk_end;
    }
    sliced
}

#[cfg(test)]
mod tests;
