use super::*;
use crate::{agents::NetworkAccess, config::RuntimeConfig};
use askama::Template;

#[test]
fn escaped_history_keeps_the_latest_message_within_the_patch_bound() {
    let state = crate::tests::test_state(RuntimeConfig::development());
    for provider in crate::providers::ProviderKind::ALL {
        state
            .vault
            .put(crate::providers::ProviderConnection::with_key(
                provider,
                "test-key",
                "test-model",
            ))
            .unwrap();
    }
    let mut record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("record");
    record.messages = (0..8)
        .map(|_| ConversationMessage {
            parent: None,
            id: crate::conversations::MessageId::generate().expect("message id"),
            response: None,
            final_phase: false,
            continuation: Vec::new(),
            role: MessageRole::User,
            text: "\"".repeat(32 * 1024),
            input: None,
            command: None,
            attachments: Vec::new(),
            activity: Vec::new(),
            status: MessageStatus::Complete,
            error: None,
            request: None,
            completion: None,
            requests: Vec::new(),
        })
        .collect();
    let window = crate::conversations::TranscriptWindow {
        messages: record.messages.clone(),
        total: record.messages.len(),
        has_before: false,
        has_after: false,
        before_anchor: record.messages.first().map(|message| message.id),
        after_anchor: record.messages.last().map(|message| message.id),
        live: true,
    };
    let view = ConversationDetailView::from_record(
        &record,
        ModelSources {
            vault: &state.vault,
            preferences: &state.preferences,
            models: &state.models_dev,
            environments: &state.environments,
            environment_snapshots: &state.environment_snapshots,
            presets: &[],
        },
        &[],
        None,
        &record.title,
        "",
        Some(&window),
    );
    assert!(view.omitted_messages > 0);
    assert_eq!(view.omitted_entries().len(), view.omitted_messages);
    let rendered = view.render().expect("page");
    for entry in view.omitted_entries() {
        assert!(rendered.contains(&format!("?around={}\"", entry.0)));
    }
    assert_eq!(
        view.messages.last().expect("latest").id,
        format!(
            "conversation-{}-message-{}",
            record.id.as_hex(),
            record.messages.last().expect("latest").id.as_hex()
        )
    );
    let mut patches = hypergraft::PatchSet::new();
    patches
        .children("conversation-detail", &view.contents())
        .expect("bounded patch");
    patches
        .encode_final(hypergraft::PatchStatus::Ok)
        .expect("bounded envelope");
}

#[test]
fn grouped_response_phases_are_not_missing_entries() {
    let state = crate::tests::test_state(RuntimeConfig::development());
    let mut record = state.conversations.create("Phases".to_owned()).unwrap();
    let mut first = assistant_message("First phase", MessageStatus::Complete);
    first.final_phase = false;
    let mut last = assistant_message("Last phase", MessageStatus::Interrupted);
    last.response = Some(first.id);
    last.parent = Some(first.id);
    let anchor = first.id;

    for phases in [vec![first, last.clone()], vec![last]] {
        record.messages = phases;
        let window = crate::conversations::TranscriptWindow {
            messages: record.messages.clone(),
            total: record.messages.len(),
            has_before: false,
            has_after: false,
            before_anchor: record.messages.first().map(|message| message.id),
            after_anchor: record.messages.last().map(|message| message.id),
            live: true,
        };
        let view = ConversationDetailView::from_record(
            &record,
            ModelSources {
                vault: &state.vault,
                preferences: &state.preferences,
                models: &state.models_dev,
                environments: &state.environments,
                environment_snapshots: &state.environment_snapshots,
                presets: &[],
            },
            &[],
            None,
            &record.title,
            "",
            Some(&window),
        );
        assert_eq!(view.messages.len(), 1);
        assert_eq!(view.messages[0].id, reply_id(&record.id, anchor));
        assert_eq!(view.omitted_messages, 0);
        assert!(view.omitted_entries().is_empty());

        let view = view.with_companion("x".repeat(672 * 1024), "activity");
        assert!(view.messages.is_empty());
        let omitted = view.omitted_entries();
        assert_eq!(omitted.len(), record.messages.len());
        for (entry, message) in omitted.iter().zip(&record.messages) {
            assert_eq!(entry.0, message.id.as_hex());
        }
    }
}

#[test]
fn network_form_preserves_domains_without_a_live_preset_ceiling() {
    let state = crate::tests::test_state(RuntimeConfig::development());
    let mut record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("record");
    record.network =
        NetworkAccess::Restricted(vec!["example.com".to_owned(), "example.org".to_owned()]);
    let view = ConversationDetailView::from_record(
        &record,
        ModelSources {
            vault: &state.vault,
            preferences: &state.preferences,
            models: &state.models_dev,
            environments: &state.environments,
            environment_snapshots: &state.environment_snapshots,
            presets: &[],
        },
        &[],
        None,
        &record.title,
        "",
        None,
    );
    assert_eq!(
        NetworkAccess::parse_form("restricted", &view.network_domains)
            .expect("resubmitted domains"),
        record.network
    );
}

#[test]
fn image_compatibility_explains_mismatch_without_assuming_support() {
    let state = crate::tests::test_state(RuntimeConfig::development());
    let record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("record");
    let mut view = ConversationDetailView::from_record(
        &record,
        ModelSources {
            vault: &state.vault,
            preferences: &state.preferences,
            models: &state.models_dev,
            environments: &state.environments,
            environment_snapshots: &state.environment_snapshots,
            presets: &[],
        },
        &[],
        None,
        &record.title,
        "",
        None,
    );
    view.attachments.attachments.push(
        crate::slices::conversations::attachments::page::StagedAttachmentView {
            id: "0".repeat(32),
            src: "/conversations/new/attachments/0".to_owned(),
            remove_action: "/conversations/new/attachments/0/remove".to_owned(),
            label: "1 by 1 image".to_owned(),
            filename: "image.png".to_owned(),
            is_image: true,
        },
    );
    view.model_picker.model = "example-model".to_owned();
    view.model_picker.selected_image_input = Some(false);
    assert!(
        view.image_compatibility_note()
            .contains("does not accept images")
    );
    view.model_picker.selected_image_input = None;
    assert!(
        view.image_compatibility_note()
            .contains("cannot confirm image input")
    );
    view.model_picker.selected_image_input = Some(true);
    assert_eq!(view.image_compatibility_note(), "");
    view.attachments.attachments[0].is_image = false;
    view.model_picker.selected_image_input = Some(false);
    assert_eq!(view.image_compatibility_note(), "");
}

#[test]
fn dense_markup_uses_escaped_text_with_bounded_nodes() {
    let text = format!("{}<script>alert(1)</script>", "* item\n".repeat(8192));
    let html = reply_html(&text);
    assert!(html.matches('<').count() <= 64);
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
}

#[test]
fn retained_output_stays_out_until_the_transcript_patch_fills_it() {
    use crate::execution::command::{
        CommandChunk, CommandResult, CommandStream, CommandTermination,
    };
    use crate::providers::{AssistantActivity, ToolOutput};

    let state = crate::tests::test_state(RuntimeConfig::development());
    let mut record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("record");
    let command = CommandResult::new(
        vec![
            CommandChunk {
                stream: CommandStream::Stdout,
                text: "VISIBLE-PREVIEW".to_owned(),
            },
            CommandChunk {
                stream: CommandStream::Stdout,
                text: " HIDDEN-RETAINED".to_owned(),
            },
        ],
        CommandTermination::Exited(0),
    );
    let retained = state
        .outputs
        .store(
            &crate::execution::OutputKey {
                scope: crate::execution::OutputScope::conversation(record.id),
                job: crate::sessions::JobId::generate().expect("job"),
                tool_call: "call-1".to_owned(),
                model_hidden: false,
            },
            &command,
        )
        .expect("store output");
    let (preview, _) = command.bounded("VISIBLE-PREVIEW".len());
    let mut message = assistant_message("", MessageStatus::Complete);
    message.activity = vec![AssistantActivity::ToolCall {
        id: "call-1".to_owned(),
        name: "run".to_owned(),
        arguments: serde_json::json!({ "command": "grep" }),
        result: Some(ToolOutput {
            resource: None,
            label: "run".to_owned(),
            output: String::new(),
            command: Some(preview.retain(retained.clone())),
        }),
    }];
    record.messages = vec![message];
    let view = ConversationDetailView::from_record(
        &record,
        ModelSources {
            vault: &state.vault,
            preferences: &state.preferences,
            models: &state.models_dev,
            environments: &state.environments,
            environment_snapshots: &state.environment_snapshots,
            presets: &[],
        },
        &[],
        None,
        &record.title,
        "",
        None,
    );
    let rendered = view.render().expect("page");
    assert!(rendered.contains("VISIBLE-PREVIEW"));
    assert!(!rendered.contains("HIDDEN-RETAINED"));
    assert!(rendered.contains("View full retained output"));
    assert!(rendered.contains(&format!("id=\"output-{}\"", retained.reference)));
    assert!(rendered.contains(&format!(
        "action=\"/conversations/{}/output\"",
        record.id.as_hex()
    )));
    assert!(rendered.contains("name=\"reference\""));
    assert!(rendered.contains(&format!("value=\"{}\"", retained.reference)));
    assert!(rendered.contains("method=\"get\""));
    assert!(rendered.contains("data-graft-history=\"none\""));
}

#[test]
fn retained_output_omits_the_control_when_the_preview_shows_everything() {
    use crate::execution::command::{
        CommandChunk, CommandResult, CommandStream, CommandTermination,
    };
    use crate::providers::{AssistantActivity, ToolOutput};

    let state = crate::tests::test_state(RuntimeConfig::development());
    let mut record = state
        .conversations
        .create("Discussion".to_owned())
        .expect("record");
    let command = CommandResult::new(
        vec![CommandChunk {
            stream: CommandStream::Stdout,
            text: "ONLY-OUTPUT".to_owned(),
        }],
        CommandTermination::Exited(0),
    );
    let retained = state
        .outputs
        .store(
            &crate::execution::OutputKey {
                scope: crate::execution::OutputScope::conversation(record.id),
                job: crate::sessions::JobId::generate().expect("job"),
                tool_call: "call-1".to_owned(),
                model_hidden: false,
            },
            &command,
        )
        .expect("store output");
    let (preview, _) = command.bounded("ONLY-OUTPUT".len());
    let mut message = assistant_message("", MessageStatus::Complete);
    message.activity = vec![AssistantActivity::ToolCall {
        id: "call-1".to_owned(),
        name: "run".to_owned(),
        arguments: serde_json::json!({ "command": "grep" }),
        result: Some(ToolOutput {
            resource: None,
            label: "run".to_owned(),
            output: String::new(),
            command: Some(preview.retain(retained.clone())),
        }),
    }];
    record.messages = vec![message];
    let view = ConversationDetailView::from_record(
        &record,
        ModelSources {
            vault: &state.vault,
            preferences: &state.preferences,
            models: &state.models_dev,
            environments: &state.environments,
            environment_snapshots: &state.environment_snapshots,
            presets: &[],
        },
        &[],
        None,
        &record.title,
        "",
        None,
    );
    let rendered = view.render().expect("page");
    assert!(rendered.contains("ONLY-OUTPUT"));
    assert!(!rendered.contains("View full retained output"));
    assert!(!rendered.contains(&format!("id=\"output-{}\"", retained.reference)));
}

fn assistant_message(text: &str, status: MessageStatus) -> ConversationMessage {
    let id = crate::conversations::MessageId::generate().expect("message id");
    ConversationMessage {
        parent: None,
        id,
        response: Some(id),
        final_phase: status != MessageStatus::Pending,
        role: MessageRole::Assistant,
        text: text.to_owned(),
        input: None,
        command: None,
        attachments: Vec::new(),
        activity: Vec::new(),
        continuation: Vec::new(),
        status,
        error: None,
        request: None,
        completion: None,
        requests: Vec::new(),
    }
}

#[test]
fn copy_source_stays_inert_and_excludes_response_neighbours() {
    use crate::providers::{AssistantActivity, ToolOutput};
    let conversation = crate::conversations::ConversationId::generate().expect("conversation");
    let text = "# Title\n\n```html\n</template><script>alert(1)</script>\n```\nLiteral <b>HTML</b> & \"quotes\" and 'apostrophes'";
    let mut message = assistant_message(text, MessageStatus::Complete);
    message.activity = vec![
        AssistantActivity::Thinking("SECRET-THOUGHT".to_owned()),
        AssistantActivity::Response(text.to_owned()),
        AssistantActivity::Tool(ToolOutput {
            resource: None,
            label: "SECRET-TOOL-LABEL".to_owned(),
            output: "SECRET-TOOL-OUTPUT".to_owned(),
            command: None,
        }),
        AssistantActivity::Response("Second response".to_owned()),
    ];
    let view = message_view(&conversation, &message);
    let copy = view.copy.as_ref().expect("settled response copy");
    let expected = format!("{text}\n\nSecond response");
    assert_eq!(copy.source, expected);
    let html = MessageBody { message: &view }
        .render()
        .expect("message body");
    let source = html
        .split_once(&format!("data-copy-for=\"{}\"", view.id))
        .expect("message-bound source")
        .1
        .split_once('>')
        .expect("source start")
        .1
        .split_once("</template")
        .expect("inert source boundary")
        .0;
    assert_eq!(copy.escaped_bytes, source.len());
    assert!(source.contains("&#60;/template&#62;&#60;script&#62;alert(1)&#60;/script&#62;"));
    assert!(!source.contains('<'));
}

#[test]
fn copy_source_excludes_status_text_while_the_message_shows_it() {
    let conversation = crate::conversations::ConversationId::generate().expect("conversation");
    let message = assistant_message("Settled reply", MessageStatus::Interrupted);
    let view = message_view(&conversation, &message);
    let copy = view.copy.as_ref().expect("settled response copy");
    assert!(!copy.source.contains("Interrupted"));
    let html = MessageBody { message: &view }
        .render()
        .expect("message body");
    assert!(html.contains("Interrupted"));
    assert!(html.contains("Settled reply"));
}

#[test]
fn copy_control_requires_a_settled_non_empty_assistant_response() {
    let conversation = crate::conversations::ConversationId::generate().expect("conversation");
    let mut user = assistant_message("Question", MessageStatus::Complete);
    user.role = MessageRole::User;
    assert!(message_view(&conversation, &user).copy.is_none());
    assert!(
        message_view(
            &conversation,
            &assistant_message("   ", MessageStatus::Complete)
        )
        .copy
        .is_none()
    );
    assert!(
        message_view(
            &conversation,
            &assistant_message("Partial", MessageStatus::Pending)
        )
        .copy
        .is_none()
    );
    assert!(
        message_view(
            &conversation,
            &assistant_message("Settled", MessageStatus::Complete)
        )
        .copy
        .is_some()
    );
}

#[test]
fn copy_source_bytes_share_the_transcript_budget() {
    let state = crate::tests::test_state(RuntimeConfig::development());
    let mut record = state
        .conversations
        .create("Budget".to_owned())
        .expect("record");
    let text = "Response with a fence\n\n```rust\nfn main() {}\n```\n".repeat(64);
    let latest = assistant_message(&text, MessageStatus::Complete);
    let older = assistant_message("Older reply", MessageStatus::Complete);
    record.messages = vec![older, latest];
    let view = message_view(&record.id, &record.messages[1]);
    let copy_len = view.copy.as_ref().expect("copy").escaped_bytes;
    let older = message_view(&record.id, &record.messages[0]);
    let older_cost =
        older.html.len() + older.copy.as_ref().map_or(0, |copy| copy.escaped_bytes) + 2048;
    let without_copy = view.html.len() + 2048;
    // The newest entry always renders. Its copy bytes still decide whether the
    // older entry shares the same budget.
    assert_eq!(visible_messages(&record, without_copy).len(), 1);
    assert_eq!(
        visible_messages(&record, without_copy + copy_len - 1).len(),
        1
    );
    assert_eq!(
        visible_messages(&record, without_copy + copy_len + older_cost).len(),
        2
    );
}

#[test]
fn plan_gate_binds_the_exact_plan_hash() {
    let (state, _, _, run) = crate::slices::human_gates::tests::conversation_at_gate();
    let view = super::pending_code_gate(&run, &state.workflow_artefacts).unwrap();
    assert_eq!(
        view.candidate,
        run.gates[0].candidate.artefact_hash.as_str()
    );
}

#[test]
fn command_output_escapes_untrusted_stream_text() {
    use crate::execution::command::{CommandChunk, CommandStream, CommandTermination};
    let tool = crate::providers::ToolOutput {
        resource: None,
        label: "run `printf '<img src=x onerror=alert(6)>'`".to_owned(),
        output: "oute\nrrer\nThe command exited with code 3.".to_owned(),
        command: Some(crate::execution::CommandResult::new(
            vec![
                CommandChunk {
                    stream: CommandStream::Stdout,
                    text: "<script>alert(1)</script>".to_owned(),
                },
                CommandChunk {
                    stream: CommandStream::Stderr,
                    text: "<img src=x onerror=alert(1)>".to_owned(),
                },
            ],
            CommandTermination::Exited(3),
        )),
    };
    let conversation = crate::conversations::ConversationId::generate().expect("conversation");
    let html = activity_html(
        &conversation,
        "message-1",
        "",
        &[
            crate::providers::AssistantActivity::ToolCall {
                id: "<script>alert(2)</script>".to_owned(),
                name: "run".to_owned(),
                arguments: serde_json::json!({"command": "<img src=x onerror=alert(3)>"}),
                result: Some(tool),
            },
            crate::providers::AssistantActivity::ToolCall {
                id: "<script>alert(4)</script>".to_owned(),
                name: "run".to_owned(),
                arguments: serde_json::json!({"command": "<img src=x onerror=alert(5)>"}),
                result: None,
            },
        ],
        &[crate::providers::ToolProgress {
            id: "private-progress-id".to_owned(),
            stream: CommandStream::Stderr,
            text: "<script>alert(7)</script>".to_owned(),
        }],
        false,
        false,
    );
    assert!(!html.contains("<script>"));
    assert!(!html.contains("<img"));
    assert!(html.contains("alert(1)"));
    assert!(html.contains("img src=x onerror=alert(1)"));
    assert!(html.contains("img src=x onerror=alert(6)"));
    assert!(html.contains("alert(7)"));
    assert!(!html.contains("private-progress-id"));
    for marker in ["alert(2)", "alert(3)", "alert(4)", "alert(5)"] {
        assert!(!html.contains(marker));
    }
}

#[test]
fn write_output_uses_only_successful_recorded_contents_and_escapes_html() {
    let contents =
        "\t<html>\r\n<script>alert('G’day')</script>\nWrote the file.\n@@ literal text\n";
    let arguments = serde_json::json!({
        "path": "/scratch/example.html",
        "contents": contents,
        "private": "PRIVATE-ARGUMENT",
    });
    let mut tool = crate::providers::ToolOutput {
        label: "write `/scratch/example.html`".to_owned(),
        output: "Wrote the file.".to_owned(),
        ..Default::default()
    };
    let block = tool_block("write", Some(&arguments), Some(&tool), false);
    assert_eq!(block.output, contents);
    let html = MessageContent {
        id: "message-1",
        output_action: "/conversations/example/output",
        blocks: vec![block],
    }
    .render()
    .expect("message content");
    assert!(!html.contains("<script>"));
    assert!(html.contains("&#60;script&#62;"));
    assert!(!html.contains("PRIVATE-ARGUMENT"));

    for arguments in [serde_json::json!({}), serde_json::json!({"contents": 42})] {
        assert_eq!(
            tool_block("write", Some(&arguments), Some(&tool), false).output,
            "File content is unavailable."
        );
    }
    let empty = serde_json::json!({"contents": ""});
    assert!(
        tool_block("write", Some(&empty), Some(&tool), false)
            .output
            .is_empty()
    );
    assert!(
        tool_block("write", Some(&arguments), None, true)
            .output
            .is_empty()
    );

    tool.label = "write".to_owned();
    assert_eq!(
        tool_block("write", Some(&arguments), Some(&tool), false).output,
        "Wrote the file."
    );
    tool.output = "That path is read-only.".to_owned();
    assert_eq!(
        tool_block("write", Some(&arguments), Some(&tool), false).output,
        tool.output
    );
}

#[test]
fn edit_output_preserves_untrusted_diff_content_without_metadata() {
    let output = concat!(
        "Updated the file.\n\n",
        "--- /scratch/example.html\n+++ /scratch/example.html\n",
        "@@ -1,3 +1,3 @@\n",
        " <html>\r\n",
        "-<script>old()</script>\n+<script>new()</script>\n",
        "@@ -20,3 +20,3 @@\n",
        "--- removed text\n+++ added text\n @@ literal context\n",
        "[output truncated]",
    );
    let mut tool = crate::providers::ToolOutput {
        label: "edit `/scratch/example.html`".to_owned(),
        output: output.to_owned(),
        ..Default::default()
    };
    let block = tool_block("edit", None, Some(&tool), false);
    assert_eq!(
        block.output,
        concat!(
            " <html>\r\n",
            "-<script>old()</script>\n+<script>new()</script>\n\n",
            "--- removed text\n+++ added text\n @@ literal context\n",
            "[output truncated]",
        )
    );
    let html = MessageContent {
        id: "message-1",
        output_action: "/conversations/example/output",
        blocks: vec![block],
    }
    .render()
    .expect("message content");
    assert!(!html.contains("<script>"));
    assert!(html.contains("&#60;script&#62;new()&#60;/script&#62;"));
    assert_eq!(tool_block("read", None, Some(&tool), false).output, output);
    tool.label = "edit".to_owned();
    assert_eq!(tool_block("edit", None, Some(&tool), false).output, output);
}

#[test]
fn tool_labels_preserve_command_text_inside_the_recorded_wrapper() {
    for command in [
        "  printf '  spaced  '  ",
        "echo `pwd`",
        "printf '%s\\n' \"a`b`c\"\nprintf done",
        "<script>alert(1)</script>",
    ] {
        let tool = crate::providers::ToolOutput {
            label: format!("run `{command}`"),
            ..Default::default()
        };
        assert_eq!(tool_block("run", None, Some(&tool), false).label, command);
    }
    for label in ["run", "run `unfinished", "other `command`"] {
        let tool = crate::providers::ToolOutput {
            label: label.to_owned(),
            ..Default::default()
        };
        assert_eq!(tool_block("run", None, Some(&tool), false).label, label);
    }
}

#[test]
fn failed_file_tools_preserve_requested_paths_without_exposing_other_arguments() {
    use crate::providers::{AssistantActivity, ToolOutput};
    let conversation = crate::conversations::ConversationId::generate().expect("conversation");
    for name in ["read", "list", "write", "edit"] {
        for (path, escaped) in [
            ("/outside/file", "/outside/file"),
            ("read `literal filename`", "read `literal filename`"),
            (
                "<img src=x onerror=alert(8)>",
                "&#60;img src=x onerror=alert(8)&#62;",
            ),
        ] {
            let arguments = serde_json::json!({"path": path, "contents": "PRIVATE-CONTENTS"});
            let tool = ToolOutput {
                label: name.to_owned(),
                output: "Denied: <script>alert(9)</script>".to_owned(),
                ..Default::default()
            };
            let block = tool_block(name, Some(&arguments), Some(&tool), false);
            assert_eq!(block.label, path);
            assert!(block.failed);
            let html = activity_html(
                &conversation,
                "message-1",
                "",
                &[AssistantActivity::ToolCall {
                    id: "PRIVATE-CALL-ID".to_owned(),
                    name: name.to_owned(),
                    arguments,
                    result: Some(tool),
                }],
                &[],
                false,
                false,
            );
            assert!(html.contains(escaped));
            assert!(html.contains("Denied:"));
            assert!(html.contains("alert(9)"));
            assert!(!html.contains("<script>"));
            assert!(!html.contains("<img"));
            assert!(!html.contains("PRIVATE-CONTENTS"));
            assert!(!html.contains("PRIVATE-CALL-ID"));
        }
    }
}

#[test]
fn file_contents_and_incomplete_records_do_not_imply_failure() {
    let arguments = serde_json::json!({"path": "/requested/file"});
    let mut tool = crate::providers::ToolOutput {
        label: "read `/resolved/file`".to_owned(),
        output: "That path does not exist.".to_owned(),
        ..Default::default()
    };
    let block = tool_block("read", Some(&arguments), Some(&tool), false);
    assert!(!block.failed);
    assert_eq!(block.label, "/resolved/file");
    tool.label = "read".to_owned();
    tool.output.clear();
    assert!(!tool_block("read", Some(&arguments), Some(&tool), true).failed);
    let pending = tool_block("read", Some(&arguments), None, true);
    assert!(!pending.failed);
    assert_eq!(pending.label, "read");
    tool.output = "Choose a file to read.".to_owned();
    for arguments in [serde_json::json!({}), serde_json::json!({"path": 42})] {
        let block = tool_block("read", Some(&arguments), Some(&tool), false);
        assert!(block.failed);
        assert_eq!(block.label, "read");
    }
}

#[test]
fn usage_panel_escapes_model_input_and_keeps_unknown_cost_distinct_from_zero() {
    use crate::conversations::{PriceProvenance, RequestId, RequestUsage};
    use crate::providers::{AuthMethod, ModelUsage, ProviderKind};
    let conversation = crate::conversations::ConversationId::generate().expect("conversation");
    let mut known = ModelUsage::new(ProviderKind::Xai, "<script>alert(1)</script>");
    known.input_tokens = Some(1_000_000);
    let known_request = RequestUsage {
        id: RequestId::generate().expect("request"),
        usage: known,
        auth: AuthMethod::ApiKey,
        prices: Some(PriceProvenance {
            source: "models.dev".to_owned(),
            input: Some(1_000_000),
            output: None,
            cache_read: None,
            cache_write: None,
        }),
        sources: Vec::new(),
        advertised: Vec::new(),
    };
    let unknown = RequestUsage {
        id: RequestId::generate().expect("request"),
        usage: ModelUsage::new(ProviderKind::OpenaiCodex, "gpt-5"),
        auth: AuthMethod::ApiKey,
        prices: None,
        sources: Vec::new(),
        advertised: Vec::new(),
    };
    let plan = RequestUsage {
        id: RequestId::generate().expect("request"),
        usage: {
            let mut usage = ModelUsage::new(ProviderKind::Xai, "grok-4");
            usage.input_tokens = Some(20);
            usage
        },
        auth: AuthMethod::Plan,
        prices: None,
        sources: Vec::new(),
        advertised: Vec::new(),
    };
    let html = message_details(&conversation, &[known_request, unknown, plan]);
    assert!(html.contains("Known subtotal ≈$1.00 (incomplete)"));
    assert!(html.contains("Cost unknown"));
    assert!(html.contains("Cost unknown for plan authentication"));
    assert!(!html.contains("$0.00"));
    assert!(html.contains("incomplete"));
    assert!(html.contains("alert(1)"));
    assert!(!html.contains("<script>"));
}

#[test]
fn phases_render_as_one_logical_response_with_aggregated_copy() {
    use crate::conversations::{MessageId, MessageRole, MessageStatus};
    use crate::providers::AssistantActivity;
    let state = crate::tests::test_state(RuntimeConfig::development());
    let mut record = state.conversations.create("Phases".to_owned()).unwrap();
    let anchor = MessageId::generate().unwrap();
    let phase = |id: MessageId, text: &str, final_phase: bool| ConversationMessage {
        parent: None,
        id,
        response: Some(anchor),
        final_phase,
        role: MessageRole::Assistant,
        text: text.to_owned(),
        input: None,
        command: None,
        attachments: Vec::new(),
        activity: vec![AssistantActivity::Response(text.to_owned())],
        continuation: Vec::new(),
        status: MessageStatus::Complete,
        error: None,
        request: None,
        completion: None,
        requests: Vec::new(),
    };
    record.messages = vec![
        ConversationMessage {
            parent: None,
            id: MessageId::generate().unwrap(),
            response: None,
            final_phase: false,
            role: MessageRole::User,
            text: "Question".to_owned(),
            input: None,
            command: None,
            attachments: Vec::new(),
            activity: Vec::new(),
            continuation: Vec::new(),
            status: MessageStatus::Complete,
            error: None,
            request: None,
            completion: None,
            requests: Vec::new(),
        },
        phase(anchor, "First response", false),
        phase(MessageId::generate().unwrap(), "Second response", true),
    ];
    let views = visible_messages(&record, usize::MAX);
    assert_eq!(views.len(), 2, "user entry plus one logical response");
    let response = &views[1];
    assert_eq!(response.id, reply_id(&record.id, anchor));
    let copy = response
        .copy
        .as_ref()
        .expect("settled logical response copy");
    assert_eq!(copy.source, "First response\n\nSecond response");
    let partial = response_view(&record.id, anchor, &[&record.messages[2]], None);
    assert!(
        partial.copy.is_none(),
        "a history window must not copy an incomplete response"
    );
}

#[test]
fn a_live_snapshot_before_the_phase_commit_does_not_repeat_committed_text() {
    let state = crate::tests::test_state(RuntimeConfig::development());
    let record = state.conversations.create("Phases".to_owned()).unwrap();
    let mut phase = assistant_message("Committed response sentinel", MessageStatus::Complete);
    phase.final_phase = false;
    let request = crate::conversations::RequestUsage {
        id: crate::conversations::RequestId::generate().unwrap(),
        usage: crate::providers::ModelUsage::new(
            crate::providers::ProviderKind::Xai,
            "grok-4.6".to_owned(),
        ),
        auth: crate::providers::AuthMethod::ApiKey,
        prices: None,
        sources: Vec::new(),
        advertised: Vec::new(),
    };
    phase.requests.push(request.clone());
    let mut stale = crate::providers::AssistantReply::default();
    stale.push_response(&phase.text);
    stale.usage.push(request);
    let view = response_view(&record.id, phase.id, &[&phase], Some((&stale, true)));
    assert_eq!(view.html.matches("Committed response sentinel").count(), 1);
    assert!(view.copy.is_none());
    assert!(view.streaming);
}
