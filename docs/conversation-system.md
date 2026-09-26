# Conversation system

## Model

A conversation holds agent context. The user chooses its boundaries independently of work completion.

Ordinary messages require no workflow. Plans and checklists remain ordinary content without a managed document lifecycle.

Optional workflows sequence model calls, deterministic operations and human decisions. Each run pins its inputs and effective settings.

Conversation settings stay local. A separate explicit action saves future defaults.

The authority boundaries remain distinct:

- Requested settings.
- Directory authority.
- Runtime consent.
- Command approval.
- Plan review.

Preferences supply no consent. Additional workflow authority requires run-only approval.

Directory modes are **Read** and **Write**. Read is the default and permits shell commands through read-only Microsandbox mounts.

Write mounts original directories read-write. Changes take effect immediately. Plan acceptance never approves or applies file changes.

## Storage

Conversations persist in a private SQLite database inside the data-directory `conversations` folder. The database is the sole backend. Startup loads transcripts only for interrupted requests and ownership recovery.

Metadata, messages and cumulative summary requests use separate tables:

- One conversation row holds the title, revision, network access, model settings, approvals, continuation checkpoint, compaction record and queue.
- Message rows have conversation-scoped identifiers, immutable parents and immutable append order. Each parent exists earlier in the same conversation.
- Summary-request rows are keyed by conversation and append order.

A mutation commits only affected rows in one transaction. Unchanged message and request rows keep their bytes, including across appends. Output checkpoints load only metadata and the pending response. Summary-request updates and automatic titles do not load transcripts.

`ConversationStore::metadata` reads catalogue fields without message content. Catalogue views and destination selectors use that projection. Reset queries propagate database errors instead of treating unreadable records as an empty catalogue.

Message validation and queue bounds remain. Retained history has no aggregate message-count, conversation-size or catalogue-size ceiling.

The database file and its SQLite sidecars stay inside the private directory boundary. SQLite uses WAL and full commit synchronisation before a separate ownership journal clears.

Database locks have a five-second wait bound. Disk-full and database-busy errors stop the commit without automatic replay. An uncertain commit blocks later mutations until restart.

An alpha record that still uses the JSON file format fails closed. The application does not migrate it and does not replace it.

## Read-only conversation tree

The Tree action opens the conversation companion. The canonical address is `/conversations/{conversation_id}/tree`. Each page contains at most 64 entries.

Child pages expose retained alternatives. Text search scans at most 1,024 entries per page. Next entries continues a partial search.

Transcript links bind inspection to an explicit leaf. Earlier and Later stay on that path. Latest returns to the active path.

Inspection changes neither the active leaf nor execution ownership. It starts no model request. Settlement updates historical status without replacement of the tree or inspected transcript.

Full conversation records contain only the active ancestor path. Provider requests, titles and independent forks consume that projection.

### Branch selection

Continue here selects an earlier complete assistant response as the active leaf. The request carries the conversation revision, the expected active leaf and the destination entry. The store updates only conversation metadata in one transaction. Existing descendants stay retained, so the tree shows both alternatives.

The next Send appends a new child of the selected response. A provider request projects only the selected ancestor path. The abandoned branch stays available for inspection. It reaches no ordinary context, title, summary or handoff prompt.

Branch selection rejects an active job, an unresolved question, a queued item, a continuation checkpoint and a prepared decision. It also rejects a stale revision or a stale expected leaf. A rejected form changes no branch.

A branch changes model context only. It changes no file, no directory approval and no execution ownership. Conversation settings stay unchanged.

The store retains compaction checkpoints across branch changes. It selects the checkpoint with the latest covered ancestor on the selected path. Summary coverage binds to that immutable ancestor, not to an abandoned child. The retained suffix follows that ancestor on the selected path.

### Context retention

Permanent retention is separate from model context. Every original entry stays in the database after a summary. A summary replaces covered entries only in the model projection. Local transcript views and the tree always read the original entries.

Compaction keeps complete recent exchanges within an application-selected budget of 20,000 tokens. Selection walks backwards along the active path and uses the model-aware tokenizer or an explicit approximation. A fallback count is an estimate, not a measured value. The selection never splits a tool call from its result and never asks the summary model to count tokens.

The automatic retention budget accounts for fixed request content:

- Effective instructions and tool declarations.
- Pending input and declared workflow inputs.
- Replacement-summary capacity and the ordinary output reserve.

The summary generator separately budgets its own request. Pending input and unresolved tool exchanges stay intact. If no complete exchange fits the budget, the application reports the constraint and keeps the previous context.

The application measures the replacement request before it commits a checkpoint. Manual compaction measures the saved instructions and transcript without runtime tools. Automatic compaction also measures runtime instructions, tools and pending input.

A committed checkpoint stores coverage against durable source positions. Repeated summaries include the previous summary and newly covered content once. Configured requests keep declared workflow inputs outside summary coverage.

### Long active turns

One logical assistant response can hold several model phases. A settled tool batch starts a fresh pending entry. The logical job and action budget stay unchanged. Every completed phase is durable and carries the logical-response anchor.

The transcript groups phases under one logical-response anchor. Oversized groups link to individual phases through bounded entry views. Copy requires a settled response that fits completely within the transcript window and copy budget. Only the settled final phase is a user-selectable branch boundary. Intermediate completed phases permit compaction inside a long active turn without loss of durable history.

Compaction rejects coverage that contains opaque continuation blocks for the selected model. It reports the unsupported boundary instead. An automatic compaction attempt that makes no source progress and no usable context reduction stops further automatic attempts. Genuine source growth or a committed context reduction resets that guard.

## Live execution

Ordinary messages share one model lifecycle. Tool-free messages use the chat job. Ordinary tool execution uses the conversation runtime without a workflow run.

Both directory modes use live mounts. The application creates no directory copy, baseline manifest or file candidate before or after a command.

Authorised access includes ignored files. Tool output bounds and bounded file previews do not restrict the files that a shell command can access.

A sandbox receives private writable scratch space at `/workspace`, including when no directory mounts exist. Duplicate or overlapping guest mounts fail before execution.

Host tools require explicit host consent. Host execution cannot enforce Read and rejects Read grants. Work locations do not confine host commands.

**Ask each time** is the default command policy for sandbox and host execution. **Automatic (YOLO)** removes individual decisions without a directory permission change.

The shared approval gate covers direct commands, model Run calls and repository-status workflow commands. Approval tokens bind the exact request and permit one decision.

Settings and presets retain the command policy. Future defaults copy requested settings only. A new conversation receives no copied directory approval or host consent.

Configured workflows retain explicit step sequences and pinned launch settings. System-only workflows also retain settings, even without model phases.

Phase authority intersects the declared tools and directories with authorised grants. A Read grant never becomes writable through a phase override or YOLO.

Plan gates bind the exact plan resolved from declared historical inputs. Revision decisions retain feedback and enforce the configured attempt limit across restart.

Microsandbox environment-image snapshots remain separate from directory access. Each sandbox attempt pins a prepared environment image.

### Command evidence

Evidence records the command request and its arguments. Timestamped lifecycle events distinguish approval, dispatch and terminal results.

Transcripts and evidence retain success or failure with bounded, redacted output. Retained output references support later pages without filesystem capture.

No evidence record grants replay authority. A restart never replays an interrupted command.

Cancellation leaves earlier Write and host effects intact. Incomplete cleanup retains reservations until recovery establishes that the runtime is absent.

Frinkworks provides no file-application transaction, rollback or automatic commit. Explicit version-control commands follow ordinary command approval and directory permissions.

## Handoff

Handoff generation uses the selected provider without tools. Context bounds, output bounds and credential redaction constrain the request.

Generation reserves the browser session. A changed source or run invalidates its result.

The user can edit the prompt before preparation opens an unsent draft. Generation and preparation create no conversation record.

At a safe decision, the handoff page offers two choices:

- Transfer workflow ownership.
- Carry context only and leave workflow ownership with the source.

Neither choice changes files or reverses earlier effects.

Send transfers workflow ownership. It preserves these run facts:

- The exact plan and decision references.
- Artefact provenance.
- The pinned workflow and settings.
- Outstanding gates.
- Remaining progression.

The destination requires explicit run-only approval. Source consent never moves to the destination. Destination preferences remain independent of the pinned execution.

Safe transfer requires completed cleanup and a pending workflow decision. Active operations remain with their current owner.

## Fork

A fork copies context up to one settled assistant response. It opens an editable, unsent draft. It is a context alternative, not filesystem rollback.

A fork accepts only a boundary between complete exchanges. It rejects a boundary that ends a user turn, that leaves a tool call without its result or that includes an uncertain command outcome.

The destination records its source conversation, source revision and boundary. Copied messages retain their source identities for entry provenance. It holds no mutable alias to source history. It copies no queue, pending question, execution checkpoint, directory approval or runtime consent.

The draft creates no conversation and starts no model call. The destination conversation exists only after the user sends the draft through the ordinary first-message path. An exclusive token claim prevents duplicate Send requests from creating multiple destinations.

The fork copies requested settings only. It drops directory approvals and runtime consent. The user must give fresh consent for the destination. A fork retains immutable evidence references and receives no filesystem authority. Nested forks retain the same restrictions.

Retained command output references move to the destination scope. The application reads the source record through its own scope and stores a new record under the destination conversation. An unavailable copy keeps the bounded preview without a full-output link.

Workflow ownership remains with the original conversation. A fork never transfers ownership. A later handoff remains the only explicit ownership transfer.

## Image attachments

A conversation message can reference immutable images. Supported formats are PNG, JPEG and static WebP. SVG, animated PNG, animated WebP, GIF and every other format are rejected.

These upload bounds apply to one staged image and one message:

- Input bytes: at most 8 MiB for one file, before decode.
- Decoded size: at most 40 megapixels and 12,000 pixels on either edge.
- Count: at most 8 images for one message.
- Aggregate normalised bytes: at most 24 MiB for one message.
- Multipart request: at most 65 MiB, including form overhead.

The application decodes each upload and re-encodes it without source metadata. It never trusts a filename or a client MIME header. A rejected upload adds no reference and keeps the composer draft.

A staged image belongs to one browser session and one composer scope. Each new draft uses its nonce. A saved conversation uses its identifier. The upload route creates no conversation.

Send or queue submission claims staged references in the same transaction as the message or queue item. A foreign, consumed or stale reference aborts the whole append. A failed commit can leave an unreferenced object, never a committed reference to absent bytes.

Image bytes live in a content-addressed object directory. The database holds only reference metadata. Several messages or forks can share one object. The application removes an object only after no reference row remains.

Unclaimed uploads expire after one hour. Startup removes all unclaimed uploads because sessions do not survive a restart. Fork drafts retain separate references until release or restart.

Queue return restores staged references without deletion. Prompt revision retains the source images. Saved messages display retained images through scoped routes.

Image blocks reach the provider through the shared request projection. The request validates the capability of the selected model before dispatch. An unknown or unsupported image capability is rejected. Opaque continuation data and duplicates stay out of the projection.

The model picker marks each model with known image-input support and offers an **Images** filter. An incompatible or unknown selection shows a note before submission.

A retained reference survives compaction and a fork. Compaction keeps original image references. A summary request includes the images of each covered chunk, so an image-only turn never reads as empty text. Context estimation uses decoded dimensions, not the encoded byte length.

Image routes are session-scoped. A saved image requires a reference that belongs to its conversation. The response uses the explicit raster media type and `no-store`.

## Skill commands

A composer message can start with `/skill:name` or `/skill:scope/name`. The resolver includes the skill directory and body, then appends the trailing user instructions. Relative references use the model-visible skill directory.

Global skill names can collide with project skill names. An unqualified duplicate name is ambiguous. Suggestions show the scopes and insert a qualified command for duplicate names.

The resolver reads global skills from the global skill folder. It reads project skills from `.agents/skills` below an authorised work location.

A leading backslash escapes the prefix. For example, `\/skill:name` sends the literal text `/skill:name`. Expansion output never runs command classification again.

A preview binds the source scope, path, relative-path base and body hash. Send rejects a changed preview. An unpreviewed command resolves from one validated source snapshot at submission. The submission freezes the typed text, the expanded text and the provenance. A queued command keeps its frozen text until the user selects a new resource.

The application rejects a skill body above 1 MiB or an expanded message above 32 KiB. Previews and submissions exclude skills that contain a known provider API key. Unknown slash commands leave the draft intact.

## Prompt templates

A composer message can start with `/template-name` or `/scope/template-name`. The resolver reads global templates directly inside `<data_root>/prompts` and project templates from `.agents/prompts` directly below an authorised work location. An absent folder produces no templates. Global templates use the `global` scope. A project template uses its work-location alias.

If the alias is `global`, its templates use `project:global` to avoid a scope collision. `/project:global/template-name` selects that project template. Session submission and unsaved drafts use the same catalogue builder, so a preview and a send agree on the source.

An unqualified duplicate name is ambiguous. The suggestions show the scopes and insert a qualified command for duplicate names. A user selects one with `/scope/template-name` rather than a silent precedence rule. A template body is never read from a nested directory or an external application configuration.

Frontmatter is optional. It carries `description` and `argument-hint`. The template body is the Markdown after the closing delimiter. A file that starts a frontmatter block must close it. The template name is the file stem without the `.md` extension.

The template name and the trailing arguments use the format `/name argument-one "argument two"`. Quotes group one argument that contains spaces. There is no shell interpolation.

Substitution is non-recursive and non-executable. The supported forms are:

- `$1`, `$2` and later positions.
- `$@` and `$ARGUMENTS` for every argument joined with spaces.
- `${N:-default}`, `${@:-default}` and `${ARGUMENTS:-default}` for an absent or empty value.
- `${@:N}` for the arguments from position N.
- `${@:N:L}` for at most L arguments from position N.

Positions use one-based indices. A backslash before `$` writes a literal dollar sign. `$(` and backticks stay literal text. Expanded text never runs command classification again, so a leading `!`, `!!` or `/` in a body stays literal.

`skill:` and application command names are reserved. A malformed name, a reserved name and a case-insensitive duplicate never become selectable. A bad template produces a limitation in the suggestions, not a partial substitution.

A preview binds the template scope, path and body hash. Send rejects a changed preview. An unpreviewed command resolves from one validated source snapshot at submission. The submission freezes the typed text, the expanded text and the provenance. The application rejects a template body above 64 KiB or an expanded message above 32 KiB. Previews and submissions exclude a template that contains a known provider API key. The Prompts page provides a catalogue and an editor for global templates. Direct file placement remains supported, and the composer discovers a saved or copied global file without a restart.

## Direct commands

A composer message that starts with `!` runs a shell command directly. A message that starts with `!!` runs the same way but excludes the command text and its output from every automatic model context. Neither form starts a model response or a title request.

The first non-whitespace character decides the syntax. Classification runs on the original typed text before resource expansion. A backslash before `!` writes literal text, for example `\!note`. Expansion output never runs command classification again.

A direct command needs the selected location, the Run capability and valid consent. A host command needs host selection. A sandbox command needs a ready environment and directory approval. It needs no provider connection. The provider page offers **Continue without a provider** for a new session. With Ask each time, every direct command waits for the same approval as a model Run call. Typed syntax alone is not approval. The application rejects an empty command and an attached image. A rejected submission leaves the draft intact.

The application persists the pending command entry before it starts a process. The entry records the context inclusion, the command text, the actual directory, the output reference and the termination. A restart marks an unsettled command as interrupted. Captured process output survives a restart before a review decision. The application never replays the command.

Command capture retains up to 256 KiB of output. Capture stops the process if output exceeds this limit. Direct commands and model Run calls return an 8 KiB preview with the outcome and retained reference. The model receives this preview and can read further pages through `read_output`. The transcript uses the same preview and expands the retained output in place. Preview truncation does not mark retained storage as truncated.

Model response text has a separate 64 KiB limit per reply phase. Model thinking text has its own 64 KiB cap. Tool results count towards neither limit. A full display budget truncates tool previews without failure of the model request. File reads retain their existing page limits.

An included `!` entry becomes delimited command evidence in the model projection. An excluded `!!` entry is absent from ordinary context, compaction, titles, workflow context and generated handoff prompts. The `read_output` tool refuses an excluded record even with the exact reference. Local output views remain available.

A command records process output and termination, not file state. Host execution changes files directly under the process user's authority.

A sandbox command uses the selected ready environment and live directory mounts. Read mounts reject writes. Write mounts expose changes to the host immediately.

Preparation failure starts no host command. Incomplete cleanup blocks further work. Cancellation never reverses a write.

Context exclusion remains separate from local approval and command evidence.

Context exclusion does not restrict filesystem access through independently authorised tools.

The application rejects command syntax in the steering and follow-up queues. It rejects command syntax that contains an image attachment. Expanded resource text remains ordinary model input.

## Ownership and recovery

The existing run record remains the sole ownership authority. Transfer never copies a run or a directory.

The conversation lock protects source and destination revisions through the ownership commit. A run fingerprint rejects stale or concurrent transfers.

A bounded ownership history preserves review provenance. Decision revisions include the ownership generation, so source-page forms cannot decide a transferred plan gate.

A pending handoff journal resides in the run record. The application flushes that journal first, then commits the source and destination projections in one transaction, then clears the journal. A failed projection keeps the journal and blocks execution. Startup completes both projections idempotently.

A failed pre-commit write leaves ownership with the source.

Safe gates survive restart and session expiry. Restoration requires fresh runtime consent in the owner conversation.

Restoration starts no model call and makes no gate decision. The original plan decision remains pending.

Restoration retains the pinned directory identities. Run-only authority grants no access to future conversation messages or workflows.

An unresolved gate prevents another message, another workflow or deletion of its owner conversation.

## Removed alpha formats

The application contains no historical retention requirement. Persisted format versions remain 1.

The removed facilities include:

- Managed conversation documents and archives.
- Managed conversation-plan controls and task imports.
- Task-list parsing and task-loop persistence.
- Task-loop routes and progression.
- Saved-document workflow inputs.
- Automatic browser updates to model preferences.
- Directory snapshots and file candidates.
- File-change review, application and rollback.
- Automatic commit workflows and before/after manifests.

Obsolete permissions and workflow formats fail closed. The application never interprets review-before-apply as Write and adds no compatibility migration.

## Validation

Security tests cover these invariants:

- Exact command approval, rejection, cancellation and token replay refusal.
- Host consent and rejection of Read grants for host execution.
- Directory identity and phase capability integrity.
- Plan provenance, revision bounds and restart validation.
- Handoff ownership, stale decisions and recovery reservations.
- Bounded output and credential redaction.
- Rejection of obsolete permissions and persisted workflow formats.

The ignored Microsandbox integration test uses a scripted provider and an isolated prepared environment. It makes no hosted-model request.

That test exercises model Run calls with Read and Write mounts, plus a system-only repository-status workflow. It also exercises ignored files and large files.

Browser validation uses isolated local data and no connected provider. It covers direct commands and execution settings, not successful hosted-model execution.

`docs/development.md` describes the validation commands and runtime prerequisites. The final task report records the executed suite results.
