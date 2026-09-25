# Frinkworks

## Product

Frinkworks is a local coding agent. Axum, Askama, Hypergraft and Rig form its application stack.

The user supplies a hosted model connection. Frinkworks has no user accounts or product account.

A local vault stores provider credentials until the user forgets that provider. Conversations and configuration persist locally.

## Conversation model

A conversation defines an agent-context boundary. The user chooses its boundaries independently of work completion.

Ordinary work needs no workflow selection. Plans and checklists can appear as ordinary text. They have no managed document or task lifecycle.

The transcript and composer form the main work surface. An optional companion presents workflow setup, progress or required decisions.

A new-conversation page is an unsaved draft. Navigation and invalid submissions create no conversation record.

The first valid message creates the conversation. Its canonical address replaces the draft address.

New with same settings copies requested settings into an independent draft. It copies no transcript, approvals, reservations or runtime consent.

Conversation settings stay local. Use saved settings as future defaults explicitly stores an independent settings snapshot.

New drafts copy those requested defaults without authority. Ordinary setup controls apply when they change. Execution review, consent and preset replacement stay behind their own confirmations.

Saved network changes require execution review with the other execution settings. Host consent authorises the exact configuration after that review.

Preset replacement shows current and replacement values before explicit confirmation. It replaces every requested setting rather than merges selected fields.

A named preset stores an independent snapshot. Saved conversations supply stored settings only. Unreviewed execution changes and unsaved edits stay excluded.

Drafts supply their current settings without a conversation record. Presets contain no directory approval or runtime consent.

Preview and save retain uncommitted setup edits. Successful replacement supersedes those edits but retains the unsent message. Rejected replacement retains the edits.

Conversation instructions are optional and accept up to 32 KiB of UTF-8 text. Drafts retain instructions and tool choices without a conversation record.

Saved conversations apply instructions after the editor loses focus. Tool choices apply on change. Tool changes can invalidate consent, but instruction edits do not grant authority.

Setup shows instruction files from the latest recorded reply request on the active branch. The files describe past context, not current access or the next request.

The source list excludes summarisation requests and advertised skills. The request context retains the complete recorded sources.

The transcript retains complete history. Model context is a separate projection. Automatic compaction replaces earlier exchanges with a summary. It runs when the input reaches the configured percentage of a known model context window. The default is enabled at 95 percent. Settings can disable automation or set a whole percentage from 1 through 100.

Automatic compaction applies only when the model catalogue publishes a context window. Unknown capacity keeps the request on the operational bound and never triggers a percentage. Below the threshold, a request can still lack output headroom. Frinkworks then reports the shortage and offers manual compaction. Manual compaction is independent of the automatic policy.

## Workflows

A workflow is an optional explicit sequence. Steps can call a model, execute a registered deterministic operation or request a human decision.

A run records one workflow execution. It pins the definition, exact inputs and effective settings.

Model steps inherit conversation settings unless the definition supplies overrides. Saved presets do not change the pinned settings of an existing run.

Additional access requires explicit run-only approval. Workflow preferences never grant directory authority or runtime consent.

Workflow outputs can include ordinary text and structured assessment reports. These outputs belong to the execution, not a conversation document catalogue.

A plan decision can return to a declared revision step. The next attempt retains the exact plan and its decision feedback.

Required assessment and approval steps repeat after a revision. Their attempt bounds remain explicit.

The application has no task-loop execution mode, task-list input, saved-plan input or archive routes for those formats.

## Authority and file changes

Requested settings and runtime authority remain separate.

The authority boundaries are:

- Directory access approval.
- Runtime consent for the selected execution location.
- Approval for an individual shell command.
- Plan review.

Conversation directories have exactly two modes: **Read** and **Write**. Read is the default.

Read permits shell commands in Microsandbox with read-only mounts. Write mounts the original directories read-write. Changes take effect immediately.

Authorised tools can access ignored files. Frinkworks applies no implicit file exclusions and performs no full-tree hashing, copying or capture.

The application provides no directory snapshots, file candidates, file-application review, rollback or automatic commits. Plan acceptance approves a plan, not file changes.

Microsandbox environment-image snapshots remain. They contain the prepared environment, not copies of authorised directories. Private scratch space remains writable in Read mode.

Host execution requires explicit consent. Host tools run as the Frinkworks process user without path confinement. Host execution cannot enforce Read and rejects Read grants.

A direct `!command` or `!!command` runs in the selected location without a provider connection. Sandbox commands require a ready environment.

**Ask each time** is the default command policy in both locations. **Automatic (YOLO)** omits individual command decisions but never increases directory permissions.

Setup and presets expose both command policies. An explicit action saves requested settings as future defaults. Saved defaults contain no runtime consent.

Command approval covers the submitted command, not script internals. Direct commands, model Run calls and repository-status workflow commands use the same approval gate.

Evidence records requests, arguments, timestamps and results. Output is bounded and redacted. Evidence is not a filesystem audit or permission to replay a command.

Cancellation stops execution without reversal of file changes. Incomplete sandbox cleanup retains its reservation until recovery establishes that the runtime is absent.

The catalogue supplies no file authority. Directory order selects the initial command location. Explicit version-control commands follow ordinary command approval and directory permissions.

## Handoff

Handoff generates an editable prompt and an unsent draft for a fresh conversation. The next agent does not start before the user sends it.

At a safe workflow decision, handoff offers two choices:

- Transfer workflow ownership.
- Carry context only and leave ownership with the source conversation.

Neither choice changes files. One conversation owns a workflow at a time.

Transfer preserves the pinned workflow, provenance and remaining steps. Draft settings convey no consent. Stale or concurrent changes prevent an unsafe transfer.

Restart preserves safe gates. The destination requires fresh run-only consent before it restores a pending decision.

## Resources

The resource navigation links directly to Workflows, Presets, Environments, Agents and Skills. Providers and Settings stay separate below the group.

Catalogue navigation creates no conversation or execution. It grants no access.

The workflow catalogue shows saved phase names and kinds. Phase purposes describe the definition, not completed activity or effective file access.

Workflow selection opens setup for the validated conversation. Without that context, the catalogue offers a conversation chooser before the existing launch review.

An ordinary unsent message in a saved conversation survives catalogue navigation within the current document. Reload and another conversation do not retain it.

Agents supply reusable instructions and requested settings. A saved agent is not a subagent or an implicit workflow participant.

A skill is an ordinary `SKILL.md` file with YAML frontmatter. Global skills live at `<data_root>/skills/<skill-folder>/SKILL.md`. Users can copy skill folders directly or edit complete files on the Skills page. No Frinkworks identifiers or revision fields are required. The next request discovers copied files without a restart.

A new global directory starts with code review, debugging and test design skills. Restart does not replace user files or restore deleted defaults.

Project skills live in `.agents/skills` directly inside each authorised directory. Discovery does not search nested project directories. Frinkworks advertises a skill name and description in both host and sandbox modes. The model reads the body with the read tool. A skill grants no tool, directory or command authority.

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

The application requires JavaScript. Real links carry ordinary navigation. Hypergraft supplies command patches and live projections.

The transcript preserves its scroll position when the reader leaves the end. Jump to latest returns to new output.

Live status comes from the server's reply state. The mobile activity strip repeats that status and opens Current work without a command.

The interface generates no task description and makes no additional model request for status. Historical transcript windows show no active-reply strip.

Current work prioritises execution status and required decisions. Workflow progress uses the recorded workflow name, state and current phase without an inferred completion percentage.

Questions and execution pauses expose their existing controls through attention strips. Context estimates and recorded usage remain available in a separate disclosure.

The companion retains eligible Stop controls outside its content scroll area. Stop during a question retains observation until settlement without a replacement execution.

Reply status and retry details stay live after companion navigation. Navigation grants no authority and starts no model request.

Queue and Stop remain separate actions. Stop targets the displayed job.

After ordinary reply completion or cancellation, the composer returns to Send without loss of the unsent message.

On mobile, an open companion excludes hidden conversation controls from interaction. Closure restores focus to a conversation control.

Use saved settings as future defaults shows a status message outside the scroll area.

## Development constraints

The application remains in alpha. Persisted formats stay at version 1. Removed formats need no compatibility migration or archive loader.

No historical records or evidence require retention. Current execution recovery still protects files and authority.

Each conversation can run one unfinished operation. Conversations do not share that reservation.

Conversations can execute at the same time. A local data reset requires that no execution is active. Safe gates keep their conversation reservation. They do not block other conversations.

Live mounts expose concurrent host changes directly. Frinkworks does not provide filesystem transactions or restore an earlier file state.

The implementation status and evidence boundary are in `docs/conversation-system.md`. Hosted-model execution remains outside the current validation evidence.

## Brand

The product name and wordmark are Frinkworks. The symbol assets are `app/public/images/logo.svg` and `app/public/images/logo-dark.svg`. The outlined wordmark is `app/public/images/wordmark.svg`.

Interface copy uses Australian English and sentence case.

The repository contains no customer evidence, testimonials or launch claims.
