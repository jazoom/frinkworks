---
version: 1
slug: "route-conversations-new"
primary_target: "route:/conversations/new"
related_targets: ["app/src/slices/conversations/templates/detail.html","app/src/slices/conversations/templates/current_work.html","app/src/slices/conversations/templates/candidate.html","app/src/slices/human_gates/templates/detail.html","app/src/slices/conversations/templates/workflow_progress.html","app/src/slices/conversations/templates/composer_toolbar.html","app/src/slices/conversations/templates/directory_manager.html","app/src/slices/conversations/templates/directory_choice.html","app/src/slices/conversations/templates/directory_consent.html","app/src/slices/conversations/templates/command_directory.html","app/src/slices/conversations/templates/effective_settings.html","app/src/slices/conversations/templates/instructions_settings.html","app/src/slices/conversations/templates/presets_settings.html","app/src/slices/conversations/page/presets.rs","app/src/shared_templates/layout/chat.html"]
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

Model selection keeps the picker open. The selected row offers Set default when it differs from the saved default. A Default badge identifies the saved default. There is no defaults footer. Accepts images names both the image-input filter and the row badges.

The paperclip uses automatic image upload. The slash picker groups built-in Commands, Skills and Prompts. Composer help retains file references and command explanations.

Setup includes a Conversation section with an inline title field and an independent draft copy. Delete confirmation expands in place without replacement of the other actions.

Eligible conversations also expose Compact context. This section omits future defaults.

The built-in `/handoff` command opens a separate companion view without a page change. Generation and preparation retain the existing consent boundaries.

Mobile controls wrap. Short mobile viewports omit the empty-state description so that the composer remains accessible.

## Constraints

This stage changes presentation, not authority. Existing directory consent and execution rules remain intact.

The implementation retains Hypergraft target identifiers and native navigation links. No model work starts during the browser review.

## Configured draft and directories

This bounded extension covers configured drafts and directory setup. Stage 1 remains approved.

The draft manager shows actual paths and access modes. Pending approval remains explicit. The heading makes no claim that an environment is ready.

Directory records contain access radios with descriptions. Selection applies ordinary directory permissions immediately without an additional approval. Presets and defaults carry those permissions. Sensitive directories and host access need separate consent. Active work blocks access changes.

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

Preview and save retain uncommitted edits. Successful replacement supersedes them but keeps the unsent message. Directory permissions carry across. Sensitive-directory consent and host consent remain separate.

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

Command approval stays at the transcript end, after the agent's output. Approve and Reject act on the exact request. No approval sidebar or attention strip appears.

The request shows the command, purpose and actual execution location. A decision replaces the form with a collapsed acknowledgement until the command record supplies its result. Historical views link to the latest request.

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

## Recent directories

This extension supplies one directory manager. New drafts show it inline. Saved conversations open it from the access summary in the existing companion shell. It replaces the Directories section in Setup. The empty composer summary offers Add a directory. Without recent or selected directories, both Add controls open the native picker directly.

Without recent or selected directories, an inline Work with your files prompt replaces the empty list. The prompt describes access to local files. An outlined Add a directory button sits beneath the explanation. Selection reveals the manager and Add another directory.

The manager retains the existing visual system and native picker. It contains up to ten recent shortcuts and current grants absent from history. Each row has an add/remove checkbox and Read and Write radios. Forget appears only on unselected recent rows.

A selection restores the last chosen mode. A directory without history defaults to Write. Unavailable or overlapping paths cannot receive new access.

Local preferences retain history and exact-path sensitive-directory approval across restarts. Later conversations reuse that approval for Read and Write. Host consent remains separate.

Forget removes the shared shortcut without changes to grants in other conversations or sensitive-directory approval. Access changes update existing shortcuts without recreation of forgotten paths. Directory commands retain unsent text and uncommitted Setup fields.

Sandbox paths remain visible without disclosures. Each row contains a button-style cwd radio, separate from Read and Write with a divider. A tint and tick identify the selected cwd. Unselected and unavailable rows retain disabled cwd controls.

Each cwd label uses a native title tooltip, including labels for disabled controls. The browser controls its delay and dismissal. Screen readers retain the description. The manager contains no separate help icon or expandable cwd explanation.

Host mode names the manager Work locations. The execution summary below the composer names the selected sandbox beside Sandbox. Only that summary names unrestricted host access.

Hypergraft retains keyed controls during directory patches. The command guard blocks concurrent changes without a temporary disable state across the list. Uncertain results disable changes.

Existing rows retain their order after selection and Read or Write changes. Stable identifiers preserve each control's identity. Only the checkbox and directory text activate selection. Mobile uses the transcript scroll area. Desktop bounds the list height.

The browser pass uses isolated local paths without model requests. It covers mode changes, multiple directories, remembered consent and Forget.
