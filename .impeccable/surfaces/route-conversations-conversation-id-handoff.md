---
version: 1
slug: "route-conversations-conversation-id-handoff"
primary_target: "route:/conversations/{conversation_id}/handoff"
related_targets: ["app/src/slices/conversations/handoff/templates/index.html", "app/src/slices/conversations/handoff/mod.rs"]
---

# Handoff

## Scope

Mode: Operate.

This surface prepares fresh agent context. The user inspects and edits a prompt before Send.

## Structure

The built-in `/handoff` command opens the existing companion shell. The slash button and a typed slash expose the same command.

The canonical route supports document navigation and targeted updates. The companion retains the transcript and unsent message. The instruction field precedes explicit generation.

A labelled textarea holds the generated prompt. At a safe decision, the companion offers workflow ownership transfer or context only. Neither choice changes files.

Preparation opens an unsent draft. Exact-change drafts display pinned settings and an explicit run-only approval control.

Send transfers the existing run and its original baseline. Outstanding decisions and remaining progression stay pending.

## Boundaries

Generation runs no tools. Navigation and preparation create no conversation record. Source revisions and run fingerprints reject stale results.

Copied settings supply no consent. The destination requires fresh approval for the pinned execution.

Restart preserves safe gates. The owner companion provides a separate restoration control. Restoration makes no model call or gate decision.

## Validation

Desktop and mobile checks cover Springfield and Sector 7-G. The mobile draft remains within the viewport with its pinned settings open.

An isolated browser pass uses a scripted provider for generation and the real preparation endpoint. It covers draft retention and mobile exclusion of conversation controls.

Successful hosted-model generation remains unverified.
