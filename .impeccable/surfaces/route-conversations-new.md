---
version: 1
slug: "route-conversations-new"
primary_target: "route:/conversations/new"
related_targets: ["app/src/slices/conversations/templates/detail.html","app/src/slices/conversations/templates/current_work.html","app/src/slices/conversations/templates/candidate.html","app/src/slices/human_gates/templates/detail.html","app/src/slices/conversations/templates/workflow_progress.html","app/src/slices/conversations/templates/composer_toolbar.html","app/src/slices/conversations/templates/draft_summary.html","app/src/slices/conversations/templates/directory_card.html","app/src/slices/conversations/templates/command_directory.html","app/src/slices/conversations/templates/effective_settings.html","app/src/slices/conversations/templates/instructions_settings.html","app/src/slices/conversations/templates/presets_settings.html","app/src/slices/conversations/page/presets.rs","app/src/shared_templates/layout/chat.html"]
---

# New conversation

## Scope

Visitor mode: Operate.

The user starts an unsaved conversation, selects a model and optionally requests directory access. Send remains the primary action.

## Visual direction

`DESIGN.md` records the workspace layout and responsive behaviour.

The user approved the lime accent and larger controls. Existing themes remain available. Existing IBM Plex fonts and the product mark remain.

The user rejected the three starter buttons and the sidebar status footer. No fabricated recent conversations appear.

## Structure

The sidebar carries grouped navigation. The header carries the title and Setup. A quiet question occupies the empty transcript.

The composer sits near the viewport bottom. Its toolbar groups attachments and model controls. Effective access stays below it.

The paperclip uses automatic image upload. Composer help retains file references and command explanations.

Mobile controls wrap. Short mobile viewports omit the empty-state description so that the composer remains accessible.

## Constraints

This stage changes presentation, not authority. Existing directory consent and execution rules remain intact.

The implementation retains Hypergraft target identifiers and native navigation links. No model work starts during the browser review.

## Configured draft and directories

This bounded extension covers configured drafts and directory setup. Stage 1 remains approved.

The configured draft shows actual paths and access modes. Pending approval remains explicit. The heading makes no claim that an environment is ready.

Directory records contain access radios with descriptions. Current access remains separate from requested changes. Saved changes retain the existing execution review.

The user requested a working command start-directory selector. Selection moves an existing directory first without changes to its identity or access mode.

A changed order requires fresh consent where applicable. Active work blocks selection. Directory commands retain the unsent message.

Short mobile screens retain the heading and composer access strip. Setup contains the complete configuration.

## Execution setup

Execution retains the approved companion layout and larger controls.

Saved network changes join the revision-bound execution review. The comparison shows current and requested values without changes to effective authority.

Host consent uses a native modal with a fixed policy summary. Change policy returns to Setup. Cancel grants no consent and changes no settings.

Environment status comes from preparation and snapshot records. Host warnings name the process identity and actual start directory.

Execution commands retain the unsent message. Active work and exact-candidate settlement retain their existing safeguards.

## Instructions setup

The editor sits above single-column tool rows with descriptions and a mixed-state Select all control.

Draft text and tool choices survive section changes and command responses. Saved edits retain honest save feedback and the existing consent boundaries.

Validation counts UTF-8 bytes. A blocked message submission reveals the editor and retains the unsent message.

The user chose recorded instruction sources, not a preview. The list describes the latest reply request on the active branch without a current-access badge.

## Presets setup

The preview compares current and replacement values before an explicit command.

Each setting labels two value columns within the approved companion width. The save panel shows a name field and an independent snapshot.

The user chose stored settings for saved snapshots. Unreviewed execution changes and unsaved edits stay excluded. Drafts supply their current choices.

Preview and save retain uncommitted edits. Successful replacement supersedes them but keeps the unsent message. Directory approval and runtime consent remain separate.

Short mobile screens retain the complete header above Setup. Preset actions remain available through the panel scroll area.

## Active conversation

Stage 3a covers the active transcript and composer. The transcript retains the approved visual language and larger controls.

Avatars sit beside author labels. Message actions follow the content. Mobile message bodies use the full available width.

The user chose existing reply statuses instead of model-generated descriptions. The mobile strip repeats the server status and opens Current work without a command.

Queue and Stop remain separate. The composer keeps the model and effort controls. Short mobile screens expose those controls through horizontal scroll.

The latest server patch controls the Send or Queue label. Live updates and Stop retain the unsent message.

## Current work

Stage 3b puts execution status and required decisions before the Context and usage disclosure.

Ordinary replies and retries use server-authored status. Workflow progress shows the pinned name, recorded state and current phase without inferred completion.

The companion retains eligible Stop controls outside its scroll area. Questions and execution pauses expose the existing controls through attention strips.

The desktop companion retains its 400-pixel width. Mobile fills the conversation area and excludes the hidden composer.

Compact desktop composer controls scroll horizontally without overlap with the companion. Escape and responsive focus restoration remain available.

## Candidate review

Stage 4a leads with changed files and the selected diff. Wide review containers use adjacent columns. Narrow containers stack them.

File selection reveals and focuses the preview. It sends no command and does not narrow approval to one file.

The full review selects the first file on each manifest page. Metadata and immutable downloads remain available through disclosures.

Approval labels follow the pinned next command. File application, local Git commits and configured continuation remain distinct.

Companion decisions return conversation patches. Draft retention and mobile focus survive the removal of the decision controls.

## Recovery

Stage 4b separates recorded directory and repository outcomes from managed cleanup. The records describe past attempts, not current files.

Unresolved application remains visible after a terminal run state. A stopped application makes no claim that any directory reports Applied.

View attempt evidence opens the exact attempt without a command. Current work and Activity share the evidence presentation with the canonical attempt page.

Settlement retains its exact run, attempt and transaction state. It keeps files and evidence through cancellation, not a Completed result.

Unsettled file or repository work omits Continue the conversation. The interface adds no retry, rollback or recovery authority.

Attention stays inside the toolbar. A rejected mobile command closes the companion and exposes its authoritative error without draft loss.

The error has its own grid row. Recovery controls retain the larger workspace scale and keyboard focus.

## Review boundary

The user approved and committed Stages 3b–4b in `a6b33db` before Stage 5a.

Stage 5a covers the workflow catalogue. Ordinary unsent text survives catalogue navigation in tab memory, bound to its conversation.

The workflow editor and preset catalogue remain separate passes. Stage 5a grants no permission to stage or commit.

The workload and browser evidence live in `docs/ui-overhaul.md`.
