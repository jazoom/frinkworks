---
version: 1
slug: "route-conversations-new"
primary_target: "route:/conversations/new"
related_targets: ["app/src/slices/conversations/templates/detail.html","app/src/slices/conversations/templates/composer_toolbar.html","app/src/slices/conversations/templates/draft_summary.html","app/src/slices/conversations/templates/directory_card.html","app/src/slices/conversations/templates/command_directory.html","app/src/slices/conversations/templates/effective_settings.html","app/src/slices/conversations/templates/instructions_settings.html","app/src/slices/conversations/templates/presets_settings.html","app/src/slices/conversations/page/presets.rs","app/src/shared_templates/layout/chat.html"]
---

# New conversation

## Scope

Visitor mode: Operate.

The user starts an unsaved conversation, selects a model and optionally requests directory access. Send remains the primary action.

## Visual reference

`i/001-new-conversation.png` defines the desktop composition. `i/071-mobile-conversation.png` supplies mobile navigation guidance.

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

References `i/002-configured-draft.png` and `i/004-directory-setup.png` define this bounded extension. Stage 1 remains approved.

The configured draft shows actual paths and access modes. Pending approval remains explicit. The heading makes no claim that an environment is ready.

Directory records contain access radios with descriptions. Current access remains separate from requested changes. Saved changes retain the existing execution review.

The user requested a working command start-directory selector. Selection moves an existing directory first without changes to its identity or access mode.

A changed order requires fresh consent where applicable. Active work blocks selection. Directory commands retain the unsent message.

Short mobile screens retain the heading and composer access strip. Setup contains the complete configuration.

## Execution setup

References 007, 010 and 011 define this extension. Execution retains the approved companion layout and larger controls.

Saved network changes join the revision-bound execution review. The comparison shows current and requested values without changes to effective authority.

Host consent uses a native modal with a fixed policy summary. Change policy returns to Setup. Cancel grants no consent and changes no settings.

Environment status comes from preparation and snapshot records. Host warnings name the process identity and actual start directory.

Execution commands retain the unsent message. Active work and exact-candidate settlement retain their existing safeguards.

## Instructions setup

Reference 008 defines this extension. The editor sits above single-column tool rows with descriptions and a mixed-state Select all control.

Draft text and tool choices survive section changes and command responses. Saved edits retain honest save feedback and the existing consent boundaries.

Validation counts UTF-8 bytes. A blocked message submission reveals the editor and retains the unsent message.

The user chose recorded instruction sources, not a preview. The list describes the latest reply request on the active branch without a current-access badge.

## Presets setup

References 042 and 079 define this extension. The preview compares current and replacement values before an explicit command.

Each setting labels two value columns within the approved companion width. The save panel shows a name field and an independent snapshot.

The user chose stored settings for saved snapshots. Unreviewed execution changes and unsaved edits stay excluded. Drafts supply their current choices.

Preview and save retain uncommitted edits. Successful replacement supersedes them but keeps the unsent message. Directory approval and runtime consent remain separate.

Short mobile screens retain the complete header above Setup. Preset actions remain available through the panel scroll area.

## Review boundary

The user authorised Presets after Instructions. Stage 2d awaits user review. Preset catalogue redesign and active-conversation redesign remain deferred.

The workload and browser evidence live in `docs/ui-overhaul.md`.
