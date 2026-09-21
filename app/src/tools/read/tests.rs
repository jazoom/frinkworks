use super::{
    MAXIMUM_PAGE_BYTES, MAXIMUM_PAGE_LINES, PageError, decode_text, page_chunks, page_text,
    parse_request, render_file_page,
};
use crate::execution::command::{CommandChunk, CommandStream};

#[test]
fn offset_is_one_based_and_limit_is_bounded() {
    assert_eq!(parse_request(None, None).expect("default").offset, 1);
    assert_eq!(
        parse_request(Some(0), None).expect_err("zero"),
        PageError::InvalidOffset
    );
    assert_eq!(
        parse_request(Some(1), Some(0)).expect_err("zero limit"),
        PageError::InvalidLimit
    );
    assert_eq!(
        parse_request(Some(4), Some(u64::MAX)).expect("clamp").limit,
        MAXIMUM_PAGE_LINES
    );
}

#[test]
fn successive_pages_reach_later_lines_without_the_start() {
    let text = (1..=20)
        .map(|line| format!("line-{line}\n"))
        .collect::<String>();
    let first = page_text(
        &text,
        parse_request(Some(1), Some(5)).expect("first request"),
    )
    .expect("first page");
    assert_eq!(first.text, "line-1\nline-2\nline-3\nline-4\nline-5\n");
    assert_eq!(first.next, Some(6));
    assert!(!first.text.contains("line-6"));

    let second = page_text(
        &text,
        parse_request(first.next, Some(5)).expect("second request"),
    )
    .expect("second page");
    assert!(second.text.starts_with("line-6\n"));
    assert!(!second.text.contains("line-1\n"));
    assert_eq!(second.next, Some(11));
}

#[test]
fn empty_file_and_end_of_file_are_distinct() {
    let empty = page_text("", parse_request(Some(1), None).expect("empty")).expect("empty file");
    assert_eq!(render_file_page(&empty), "The file is empty.");
    assert_eq!(empty.next, None);
    assert_eq!(
        page_text("", parse_request(Some(2), None).expect("past empty")),
        Err(PageError::PastEnd)
    );

    let text = "only\n";
    let page = page_text(text, parse_request(Some(1), Some(10)).expect("eof")).expect("whole file");
    assert_eq!(page.text, "only\n");
    assert_eq!(page.next, None);
    assert_eq!(
        page_text(text, parse_request(Some(2), None).expect("past eof")),
        Err(PageError::PastEnd)
    );
}

#[test]
fn an_oversized_line_advances_past_the_same_offset() {
    let text = format!("{}\nnext\n", "x".repeat(MAXIMUM_PAGE_BYTES + 8));
    let first = page_text(&text, parse_request(Some(1), Some(10)).expect("request"))
        .expect("truncated line");
    assert_eq!(first.text.len(), MAXIMUM_PAGE_BYTES);
    assert!(first.line_truncated);
    assert_eq!(first.next, Some(2));
    let second = page_text(&text, parse_request(first.next, Some(10)).expect("next"))
        .expect("following line");
    assert_eq!(second.text, "next\n");
    assert_eq!(second.next, None);
}

#[test]
fn byte_bound_does_not_repeat_the_same_line() {
    let text = format!("short\n{}\n", "y".repeat(MAXIMUM_PAGE_BYTES));
    let first = page_text(&text, parse_request(Some(1), Some(10)).expect("request"))
        .expect("first line only");
    assert_eq!(first.text, "short\n");
    assert!(!first.line_truncated);
    assert_eq!(first.next, Some(2));
}

#[test]
fn binary_and_invalid_text_are_rejected() {
    assert_eq!(decode_text(b"ok\n"), Ok("ok\n"));
    assert_eq!(decode_text(b"pre\0post"), Err(PageError::Binary));
    assert_eq!(decode_text(&[0xff, 0xfe]), Err(PageError::InvalidText));
}

#[test]
fn retained_output_pages_preserve_stream_identity() {
    let chunks = vec![
        CommandChunk {
            stream: CommandStream::Stdout,
            text: "one\ntwo\n".to_owned(),
        },
        CommandChunk {
            stream: CommandStream::Stderr,
            text: "warn\n".to_owned(),
        },
        CommandChunk {
            stream: CommandStream::Stdout,
            text: "three\n".to_owned(),
        },
    ];
    let (first, next, _) = page_chunks(&chunks, parse_request(Some(1), Some(2)).expect("page"))
        .expect("first output page");
    assert_eq!(
        first,
        vec![CommandChunk {
            stream: CommandStream::Stdout,
            text: "one\ntwo\n".to_owned(),
        }]
    );
    assert_eq!(next, Some(3));
    let (second, next, _) = page_chunks(&chunks, parse_request(next, Some(2)).expect("second"))
        .expect("second output page");
    assert_eq!(
        second,
        vec![
            CommandChunk {
                stream: CommandStream::Stderr,
                text: "warn\n".to_owned(),
            },
            CommandChunk {
                stream: CommandStream::Stdout,
                text: "three\n".to_owned(),
            },
        ]
    );
    assert_eq!(next, None);
}

#[test]
fn oversized_final_line_does_not_offer_a_nonexistent_page() {
    let text = "é".repeat(MAXIMUM_PAGE_BYTES);
    let page = page_text(&text, parse_request(None, None).expect("request")).expect("page");
    assert!(page.line_truncated);
    assert_eq!(page.next, None);
    assert!(page.text.len() <= MAXIMUM_PAGE_BYTES);
}

#[test]
fn fragmented_output_reserves_space_for_stream_labels() {
    let chunks = (0..20_000)
        .map(|index| CommandChunk {
            stream: if index % 2 == 0 {
                CommandStream::Stdout
            } else {
                CommandStream::Stderr
            },
            text: if index == 19_999 { "x\nnext\n" } else { "x" }.to_owned(),
        })
        .collect::<Vec<_>>();
    let (page, next, _) =
        page_chunks(&chunks, parse_request(None, Some(2_000)).expect("request")).expect("page");
    let rendered: usize = page
        .iter()
        .map(|chunk| chunk.text.len() + chunk.stream.label().len() + 3)
        .sum();
    assert!(rendered <= MAXIMUM_PAGE_BYTES);
    assert!(next.is_some());
}

#[test]
fn bounded_reader_rejects_directory_and_outside_symlink() {
    let dir = tempfile::tempdir().expect("directory");
    let root = dir.path().join("root");
    std::fs::create_dir(&root).expect("root");
    let outside = dir.path().join("outside");
    std::fs::write(&outside, "private").expect("outside");
    let link = root.join("link");
    std::os::unix::fs::symlink(&outside, &link).expect("link");
    for (target, code) in [(root.clone(), 2), (link, 4), (root.join("../outside"), 4)] {
        let output = std::process::Command::new("sh")
            .args(["-c", super::super::CONFINED_READ_SCRIPT, "read-test"])
            .arg(&root)
            .arg(target)
            .arg("100")
            .output()
            .expect("read");
        assert_eq!(output.status.code(), Some(code));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private"));
    }
}

#[test]
fn bounded_reader_pages_a_large_utf8_file_and_reports_read_failure() {
    let dir = tempfile::tempdir().expect("directory");
    let file = dir.path().join("file");
    let text = (1..=30_000)
        .map(|line| format!("{line}: café\n"))
        .collect::<String>();
    std::fs::write(&file, &text).expect("file");
    let output = std::process::Command::new("sh")
        .args(["-c", super::super::CONFINED_READ_SCRIPT, "read-test"])
        .arg(dir.path())
        .arg(&file)
        .arg((super::MAXIMUM_SCAN_BYTES + 1).to_string())
        .output()
        .expect("read");
    assert!(output.status.success());
    let text = decode_text(&output.stdout).expect("text");
    let first =
        page_text(text, parse_request(Some(20_000), Some(2)).expect("request")).expect("page");
    assert_eq!(first.text, "20000: café\n20001: café\n");
    let second = page_text(text, parse_request(first.next, Some(2)).expect("next")).expect("page");
    assert_eq!(second.text, "20002: café\n20003: café\n");
    let output = std::process::Command::new("sh")
        .args(["-c", super::super::CONFINED_READ_SCRIPT, "read-test"])
        .arg(dir.path())
        .arg(file)
        .arg("invalid-byte-count")
        .output()
        .expect("read failure");
    assert!(!output.status.success());
}
