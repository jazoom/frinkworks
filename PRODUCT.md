# Power Plant

## Product

Power Plant is a local coding agent. Axum, Askama, Hypergraft and Rig form its application stack.

The user supplies a hosted model connection. Power Plant has no user accounts or product account.

A local vault stores provider credentials until the user forgets that provider. Conversations and configuration persist locally.

## Conversation model

A conversation defines an agent-context boundary. The user chooses its boundaries independently of work completion.

Ordinary work needs no workflow selection. Plans and checklists can appear as ordinary text. They have no managed document or task lifecycle.

The transcript and composer form the main work surface. An optional companion presents workflow setup, progress or required decisions.

A new-conversation page is an unsaved draft. Navigation and invalid submissions create no conversation record.

The first valid message creates the conversation. Its canonical address replaces the draft address.

New with same settings copies requested settings into an independent draft. It copies no transcript, approvals, reservations or runtime consent.

Conversation settings stay local. Use saved settings as future defaults explicitly stores an independent settings snapshot.

New drafts copy those requested defaults without authority. Unsaved edits do not change effective settings.

## Workflows

A workflow is an optional explicit sequence. Steps can call a model, execute a registered deterministic operation or request a human decision.

A run records one workflow execution. It pins the definition, exact inputs and effective settings.

Model steps inherit conversation settings unless the definition supplies overrides. Saved presets do not change the pinned settings of an existing run.

Additional access requires explicit run-only approval. Workflow preferences never grant directory authority or runtime consent.

Workflow outputs can include ordinary text and structured assessment reports. These outputs belong to the execution, not a conversation document catalogue.

A human decision can return to a declared revision step. The next attempt retains the exact candidate, original baseline and candidate-bound feedback.

Required assessment and approval steps repeat after a revision. Their attempt bounds remain explicit.

The application has no task-loop execution mode, task-list input, saved-plan input or archive routes for those formats.

## Authority and file changes

Requested settings and runtime authority remain separate.

The authority boundaries are:

- Directory access approval.
- Runtime consent for the selected execution location.
- Approval for an individual host command.
- Assessment of proposed code.
- Approval to apply exact prepared file changes.

Conversation directories support Read only, Review before apply and Direct write.

Review before apply keeps proposals isolated until approval. Direct write changes the named directory immediately.

Diffs are the primary code result. The interface must distinguish isolated proposals from files that the agent already changed.

File application does not implicitly create a Git commit. Explicit Git operations remain separate from file application.

Candidate approval binds the exact candidate and original baseline. A newer host state cannot silently replace that baseline.

Application transactions retain recovery journals and preimages. Uncertain file outcomes or incomplete cleanup block further work.

Cancellation and discarded proposals do not undo direct writes or other host effects.

Sandbox tools use authorised mounts. Host commands run as the Power Plant process user, without additional privileges or path confinement.

Ask each time is the default host command policy. Run without approval requires fresh consent for the destination settings.

Command approval covers the submitted command. Power Plant does not inspect script internals. The hosted model receives command output.

The project catalogue supplies context, not file authority. Directory order never selects an implicit Git destination.

## Handoff

Handoff generates an editable prompt and an unsent draft for a fresh conversation. The next agent does not start before the user sends it.

When isolated changes exist, the target system offers two choices:

- Continue those exact prepared changes.
- Carry context only and leave isolated changes in the source conversation.

Neither choice applies or discards changes. Neither choice undoes direct writes.

Transfer must preserve the original baseline, provenance and remaining workflow steps. One owner controls the prepared changes at a time.

Draft settings convey no consent. Stale or concurrent changes must prevent an unsafe transfer.

Handoff supports both choices at a safe decision. Transfer preserves the existing run, its original baseline and its remaining progression.

Restart preserves safe gates. The destination requires fresh run-only consent before it restores a pending decision.

## Resources

The resource navigation links directly to Workflows, Presets, Environments and Providers. More resources contains Projects and Agents.

Catalogue navigation creates no conversation or execution. It grants no access.

Projects have a name and one immutable Git worktree path. Unavailable projects retain their labels. Project registration does not grant access.

Agents supply reusable instructions and requested settings. A saved agent is not a subagent or an implicit workflow participant.

Environment recipes supply an OCI image and optional setup script. Each sandbox attempt pins a ready prepared snapshot.

An idle conversation owns no persistent sandbox. A new environment selection affects later attempts only.

Tool-free chat and host commands need no sandbox runtime. Sandbox execution requires its selected ready environment without substitution.

The supported providers are:

- xAI.
- OpenAI Codex.
- Synthetic.
- OpenRouter.
- DeepSeek.

The connect page also supports ChatGPT and SuperGrok plan sign-in. The vault can hold more than one provider connection.

The model picker uses connected providers and the models.dev catalogue. Models without adjustable effort show Not available.

## Interface

Sector 7-G and the existing theme tokens remain part of the product identity.

The available themes are:

- Springfield.
- Evergreen Terrace.
- Leftorium.
- Stonecutters.
- Sector 7-G.

The system preference selects Springfield or Sector 7-G unless the user selects a theme explicitly.

Real links provide native navigation fallback. Hypergraft supplies command patches and live projections.

The transcript preserves its scroll position when the reader leaves the end. Jump to latest returns to new output.

On mobile, an open companion excludes hidden conversation controls from interaction. Closure restores focus to a conversation control.

Settings feedback remains outside the scroll area. The message distinguishes conversation-local settings from future defaults.

## Development constraints

The application remains in alpha. Persisted formats stay at version 1. Removed formats need no compatibility migration or archive loader.

No historical records or evidence require retention. Current execution recovery still protects files and authority.

One browser session permits one active command. A conversation reservation protects its unfinished operation.

One workflow execution can hold the process-wide execution reservation. Safe gates release the session reservation for other conversations.

The implementation status and evidence boundary are in `docs/conversation-system.md`. Hosted-model execution remains outside the current validation evidence.

## Brand

The product name and wordmark are Power Plant. The mark is `app/public/images/logo.svg`.

Interface copy uses Australian English and sentence case.

The repository contains no customer evidence, testimonials or launch claims.
