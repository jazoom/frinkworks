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

A canonical page retains shared navigation and the selected theme. The instruction field precedes generation. A labelled textarea holds the generated prompt.

At a safe decision, the page offers exact prepared changes or context only. Neither choice applies, discards or reverses files.

Preparation opens an unsent draft. Exact-change drafts display pinned settings and an explicit run-only approval control.

Send transfers the existing run and its original baseline. Outstanding decisions and remaining progression stay pending.

## Boundaries

Generation runs no tools. Navigation and preparation create no conversation record. Source revisions and run fingerprints reject stale results.

Copied settings supply no consent. The destination requires fresh approval for the pinned execution.

Restart preserves safe gates. The owner companion provides a separate restoration control. Restoration makes no model call or gate decision.

## Validation

Desktop and mobile checks cover Springfield and Sector 7-G. The mobile draft remains within the viewport with its pinned settings open.

The browser uses a supplied prompt through the real preparation endpoint. Successful hosted-model generation remains unverified.
