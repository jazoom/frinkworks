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
            id: crate::conversations::MessageId::generate().expect("message id"),
            continuation: Vec::new(),
            role: MessageRole::User,
            text: "\"".repeat(32 * 1024),
            activity: Vec::new(),
            status: MessageStatus::Complete,
            error: None,
            request: None,
            completion: None,
        })
        .collect();
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
    );
    assert!(view.omitted_messages > 0);
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
    assert!(
        view.render()
            .expect("page")
            .contains("remain in local history and model context")
    );
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
        false,
        false,
    );
    assert!(!html.contains("<script>"));
    assert!(!html.contains("<img"));
    assert!(html.contains("alert(1)"));
    assert!(html.contains("img src=x onerror=alert(1)"));
}
