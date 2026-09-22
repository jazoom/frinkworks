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
    let rendered = view.render().expect("page");
    let rendered = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(rendered.contains("remain in local history and model context"));
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
fn dense_markup_uses_escaped_text_with_bounded_nodes() {
    let text = format!("{}<script>alert(1)</script>", "* item\n".repeat(8192));
    let html = reply_html(&text);
    assert!(html.matches('<').count() <= 64);
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
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
fn candidate_review_escapes_untrusted_file_contents() {
    let state = crate::tests::test_state(RuntimeConfig::development());
    let record = state.conversations.create("Discussion".to_owned()).unwrap();
    let title = record.title.clone();
    let gate = super::PendingCodeGateView {
        run_id: "run".to_owned(),
        gate_id: "gate".to_owned(),
        revision: "1".to_owned(),
        candidate: "abc".to_owned(),
        diff_base: "def".to_owned(),
        diff_href: "/runs/run/gates/gate".to_owned(),
        review_href: "/conversations/candidate-review?run=run".to_owned(),
        ordinary: true,
        commit_on_approval: true,
        application_destination: "/tmp/test".to_owned(),
        can_request_revision: false,
        quick_task: false,
        exclusions: Vec::new(),
        total_changes: 1,
        changes_truncated: false,
        changes: vec![super::CandidateChangeView {
            path: "<script>alert(1)</script>".to_owned(),
            name: "<script>alert(1)</script>".to_owned(),
            directory: "project".to_owned(),
            status: "Added",
            preview: "+<img src=x onerror=alert(1)>\n".to_owned(),
            additions: 1,
            removals: 0,
            has_counts: true,
        }],
    };
    let view = ConversationDetailView::from_record_with_gate(
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
        &title,
        "",
        Some(gate),
        None,
        Vec::new(),
        crate::slices::conversations::attachments::AttachmentsView::empty(String::new()),
        None,
        None,
    );
    let html = view.render().expect("page");
    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(!html.contains("<img src=x"));
    assert!(html.contains("&#60;script&#62;"));
    assert!(html.contains("&#60;img"));
    assert!(html.contains("Apply and commit"));
    assert!(!html.contains("no Git commit"));
}

#[derive(askama::Template)]
#[template(path = "conversations/templates/workflow_progress.html")]
struct ProgressHarness {
    run: WorkflowProgressView,
}

fn partial_progress_view() -> WorkflowProgressView {
    WorkflowProgressView {
        run_href: "/runs/aaa".to_owned(),
        name: "Quick task".to_owned(),
        state: "Active",
        current_step: "Apply changes".to_owned(),
        result: "Worker activity stays in the run record.",
        task_progress: String::new(),

        conversation_id: "ccc".to_owned(),
        apply_run_id: "aaa".to_owned(),
        apply_attempt_id: "bbb".to_owned(),
        apply_state: "recovered",
        apply_outcomes: vec![ApplyOutcomeView {
            directory: "fieldnotes".to_owned(),
            path: "/tmp/fieldnotes".to_owned(),
            outcome: "Applied",
        }],
        apply_resolve_href: "/runs/aaa/attempts/bbb/changes".to_owned(),
        apply_partial: true,
        apply_uncertain: false,
        apply_complete: false,
        settlement_eligible: true,
        run_terminal: false,
    }
}

// Recovery controls bind the displayed run, attempt and outcome state.
#[test]
fn partial_progress_links_the_exact_attempt_and_settlement_identity() {
    use askama::Template;
    let html = ProgressHarness {
        run: partial_progress_view(),
    }
    .render()
    .expect("progress");
    assert!(html.contains("/runs/aaa/attempts/bbb/changes"));
    assert!(html.contains("/conversations/ccc/runs/aaa/settle-partial"));
    assert!(html.contains("name=\"attempt\""));
    assert!(html.contains("value=\"bbb\""));
    assert!(html.contains("value=\"recovered\""));
}

#[test]
fn command_output_escapes_untrusted_stream_text() {
    use crate::execution::command::{CommandChunk, CommandStream, CommandTermination};
    let tool = crate::providers::ToolOutput {
        resource: None,
        label: "run `exit 3`".to_owned(),
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
        &[crate::providers::AssistantActivity::Tool(tool)],
        &[],
        &[],
        false,
        false,
    );
    assert!(!html.contains("<script>"));
    assert!(!html.contains("<img"));
    assert!(html.contains("alert(1)"));
    assert!(html.contains("img src=x onerror=alert(1)"));
}

#[test]
fn usage_panel_labels_unknown_cost_instead_of_a_zero_total() {
    use crate::conversations::{PriceProvenance, RequestId, RequestUsage};
    use crate::providers::{AuthMethod, ModelUsage, ProviderKind};
    let conversation = crate::conversations::ConversationId::generate().expect("conversation");
    let mut known = ModelUsage::new(ProviderKind::Xai, "grok-4");
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
    let html = activity_html(
        &conversation,
        "message-1",
        "Done.",
        &[],
        &[],
        &[known_request, unknown, plan],
        false,
        false,
    );
    assert!(html.contains("Estimated cost $1.00") || html.contains("Known subtotal $1.00"));
    assert!(html.contains("Cost unknown"));
    assert!(html.contains("Cost unknown for plan authentication"));
    assert!(!html.contains("$0.00"));
    assert!(html.contains("incomplete"));
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
