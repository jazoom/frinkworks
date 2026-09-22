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
- Code assessment.
- File-application approval.

Preferences supply no consent. Additional workflow authority requires run-only approval.

Review before apply isolates proposals. Direct write changes the named directory immediately. File application never implicitly creates a Git commit.

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
- Pending input and candidate-bound inputs.
- Replacement-summary capacity and the ordinary output reserve.

The summary generator separately budgets its own request. Pending input and unresolved tool exchanges stay intact. If no complete exchange fits the budget, the application reports the constraint and keeps the previous context.

The application measures the replacement request before it commits a checkpoint. Manual compaction measures the saved instructions and transcript without runtime tools. Automatic compaction also measures runtime instructions, tools and pending input.

A committed checkpoint stores coverage against durable source positions. Repeated summaries include the previous summary and newly covered content once. Configured requests keep candidate-bound inputs outside summary coverage.

### Long active turns

One logical assistant response can hold several model phases. A settled tool batch starts a fresh pending entry. The logical job and action budget stay unchanged. Every completed phase is durable and carries the logical-response anchor.

The transcript groups phases under one logical-response anchor. Oversized groups link to individual phases through bounded entry views. Copy requires a settled response that fits completely within the transcript window and copy budget. Only the settled final phase is a user-selectable branch boundary. Intermediate completed phases permit compaction inside a long active turn without loss of durable history.

Compaction rejects coverage that contains opaque continuation blocks for the selected model. It reports the unsupported boundary instead. An automatic compaction attempt that makes no source progress and no usable context reduction stops further automatic attempts. Genuine source growth or a committed context reduction resets that guard.

## Live execution

Ordinary messages share one model lifecycle. Tool-free messages use the chat job. Host tools without named directories and read-only sandbox tools use the ordinary conversation runtime. They do not create a workflow run.

Ordinary file-change work also uses that runtime. Reviewed directories, direct-write directories and directory-backed host tools still own a `WorkflowRun` for baselines, candidates, already-written snapshots, gates and apply journals. That record is not a configured model sequence.

The ordinary adapter and configured executor share the file-attempt driver. That driver owns baseline capture, candidate materialisation, final snapshots and cleanup outside the model loop.

Ordinary model requests omit workflow roles and required-output tools. Candidate revisions retain verified decision feedback and exclude the original conversation context. The application never treats a conversation reply as approval. Uncertain apply outcomes and incomplete cleanup block further work.

Host tools require explicit host consent and the selected command policy.

Configured workflows keep their explicit step sequences. Directory grants pin directory identities for handoff and recovery.

### File-change review evidence

Controlled provider tests execute host commands against temporary directories. They retain the original baseline and final snapshot after a write. Later host edits do not change those snapshots. Cancellation after a write preserves that write and its final snapshot.

Authority tests reject stale directory identities and foreign conversation ownership. The Rust suite covers candidate approval, revision and configured workflow transitions. These tests do not establish hosted-provider success or a live sandbox review-before-apply flow.

The browser review opened the new conversation and its directory setup panel without console or page errors. It did not execute a file-change request.

Commit steps detect changed repositories from the approved candidate and pinned review-before-apply grants. They do not search parent directories or select a global destination.

Each repository has a separate commit result and journal. Recovery retains successful commits and restores only incomplete file application before a reference update.

Commits require a clean Git index and worktree. They include task changes, not unchanged ignored files from the captured directory.

A changed non-Git directory prevents the commit step before the first write. File application remains available without commits.

Linked Git worktrees remain unsupported. Managed Git commits refuse colocated jj repositories.

Other version-control commands follow ordinary tool permissions, not the managed Git transaction.

An ordinary execution cannot dispatch a registered commit step. A configured workflow can include an explicit commit operation.

Direct-write results use immutable before-and-after manifests. The conversation companion and execution pages provide bounded previews and downloads.

The interface labels these results Already-written changes. An absent final snapshot remains unknown. The application never reconstructs it from current host files.

Host commands can affect locations outside the named-directory snapshots. Other processes can also change files within those snapshots.

## Handoff

Handoff generation uses the selected provider without tools. Context bounds, output bounds and credential redaction constrain the request.

Generation reserves the browser session. A changed source or run invalidates its result.

The user can edit the prompt before preparation opens an unsent draft. Generation and preparation create no conversation record.

At a safe decision, the handoff page offers two choices:

- Continue the exact prepared changes.
- Carry context only and leave prepared changes in the source.

Neither choice applies or discards prepared changes. Neither choice reverses direct writes or earlier workflow effects.

Send transfers ownership for exact-change handoff. It preserves these run facts:

- The original baseline and candidate references.
- Artefact provenance.
- The pinned workflow and settings.
- Outstanding gates.
- Remaining progression.

The destination requires explicit run-only approval. Source consent never moves to the destination. Destination preferences remain independent of the pinned execution.

Safe transfer requires completed cleanup and a captured source at a pending human decision. Uncertain or partially applied operations remain with their current owner.

## Fork

A fork copies context up to one settled assistant response. It opens an editable, unsent draft. It is a context alternative, not filesystem rollback.

A fork accepts only a boundary between complete exchanges. It rejects a boundary that ends a user turn, that leaves a tool call without its result or that includes an uncertain command outcome.

The destination records its source conversation, source revision and boundary. Copied messages retain their source identities for entry provenance. It holds no mutable alias to source history. It copies no queue, pending question, execution checkpoint, directory approval or runtime consent.

The draft creates no conversation and starts no model call. The destination conversation exists only after the user sends the draft through the ordinary first-message path. An exclusive token claim prevents duplicate Send requests from creating multiple destinations.

The fork copies requested settings only. It drops directory approvals and runtime consent. The user must give fresh consent for the destination. A fork of a candidate review retains immutable evidence references and receives no filesystem authority. Nested forks retain the same restrictions.

Retained command output references move to the destination scope. The application reads the source record through its own scope and stores a new record under the destination conversation. An unavailable copy keeps the bounded preview without a full-output link.

Prepared changes remain with the original run. A fork never transfers ownership. A later handoff remains the only explicit ownership transfer.

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

A retained reference survives compaction and a fork. Compaction keeps original image references. A summary request includes the images of each covered chunk, so an image-only turn never reads as empty text. Context estimation uses decoded dimensions, not the encoded byte length.

Image routes are session-scoped. A saved image requires a reference that belongs to its conversation. The response uses the explicit raster media type and `no-store`.

## Skill commands

A composer message can start with `/skill:name` or `/skill:scope/name`. The resolver includes the skill directory and body, then appends the trailing user instructions. Relative references use the model-visible skill directory.

Global skill names can collide with project skill names. An unqualified duplicate name is ambiguous. Suggestions show the scopes and insert a qualified command for duplicate names.

The resolver reads global skills from the global skill folder. It reads project skills from `.agents/skills` below an authorised work location. A candidate-backed location exposes no skill body. The preview reports the limitation instead of reading current host files.

A leading backslash escapes the prefix. For example, `\/skill:name` sends the literal text `/skill:name`. Expansion output never runs command classification again.

A preview binds the source scope, path, relative-path base and body hash. Send rejects a changed preview. An unpreviewed command resolves from one validated source snapshot at submission. The submission freezes the typed text, the expanded text and the provenance. A queued command keeps its frozen text until the user selects a new resource.

The application rejects a skill body above 1 MiB or an expanded message above 32 KiB. Previews and submissions exclude skills that contain a known provider API key. Unknown slash commands leave the draft intact.

## Prompt templates

A composer message can start with `/template-name`. The resolver reads global templates directly inside `<data_root>/prompts`. An absent folder produces no templates. Session submission and unsaved drafts use the same catalogue builder, so a preview and a send agree on the source.

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

A preview binds the template scope, path and body hash. Send rejects a changed preview. An unpreviewed command resolves from one validated source snapshot at submission. The submission freezes the typed text, the expanded text and the provenance. The application rejects a template body above 64 KiB or an expanded message above 32 KiB. Previews and submissions exclude a template that contains a known provider API key. There is no prompt-management page: the user edits the Markdown files directly.

## Ownership and recovery

The existing run record remains the sole ownership authority. Transfer never copies a run or recaptures its baseline.

The conversation lock protects source and destination revisions through the ownership commit. A run fingerprint rejects stale or concurrent transfers.

A bounded ownership history preserves review provenance. Decision revisions include the ownership generation, so source-page forms cannot approve transferred changes.

A pending handoff journal resides in the run record. The application flushes that journal first, then commits the source and destination projections in one transaction, then clears the journal. A failed projection keeps the journal and blocks execution. Startup completes both projections idempotently.

A failed pre-commit write leaves ownership with the source.

Safe gates survive restart and session expiry. Restoration requires fresh runtime consent in the owner conversation.

Restoration starts no model call and makes no gate decision. The original assessment, revision or application choice remains pending.

Restoration retains the pinned directory identities. Run-only authority grants no access to future conversation messages or workflows.

An unresolved gate prevents another message, another workflow or deletion of its owner conversation.

Application transactions retain their separate journals and preimages. Completed application records reload without Git write authority or dependence on later candidate references.

## Removed alpha formats

The application contains no historical retention requirement. Persisted format versions remain 1.

The removed facilities include:

- Managed conversation documents and archives.
- Plan controls and task imports.
- Task-list parsing and task-loop persistence.
- Task-loop routes and progression.
- Saved-document workflow inputs.
- Automatic browser updates to model preferences.

Current file-operation recovery remains separate from historical compatibility. The application adds no compatibility migration.

## Validation

The full suites pass:

- 1,194 Rust library tests.
- One additional Rust binary test.
- 81 browser-unit tests.

`mise run clean` passes without warnings. Production and development asset builds pass.

The real browser checks use `http://localhost:4000` with synthetic persisted records and a placeholder provider credential.

The checks cover these paths:

- A fork after a completed tool exchange and its unsent draft.
- Both handoff choices and an unsent draft.
- Rejection of missing run-only approval.
- Transfer without file writes.
- Restart and fresh consent in the destination.
- Rejection of a source-page approval after transfer.
- Explicit application to a real temporary file without a commit.
- Restart after completed application.
- Direct-write previews and before-and-after downloads.
- Desktop and mobile presentation in Springfield and Sector 7-G.

A supplied prompt entered the real preparation endpoint for transfer tests. The browser did not simulate a successful provider response.

The fork browser check used a synthetic persisted conversation with one completed tool exchange. It followed the fork control to the confirmation page and opened the unsent draft. The draft created no second conversation and started no model call. The source revision and messages stayed unchanged. Console and page diagnostics were empty. The fork beside pending prepared changes is covered by the Rust router test, not the browser check.

The review browser check used a separate local fixture. It opened the fork draft and exercised a rejected Send without a stored provider. No destination conversation appeared. Console and page diagnostics were empty. This check made no hosted-provider request.

The branch route test selected an earlier response and kept both alternatives in the tree. Its controlled provider request contained only the selected path. A store test reopened two branches across restart with the abandoned branch excluded from model context. A real browser check used a synthetic conversation with two retained branches. It selected an earlier response through Continue here, appended a new child with a failed placeholder-provider request and reloaded the tree after a server restart. The abandoned branch stayed in the tree. Console and page diagnostics were empty. These checks use synthetic records, not hosted model requests.

The branch review browser exercise switched between synthetic branches across a server restart. It retained an unsent draft during branch selection. Both alternative tips stayed available after selection of their shared ancestor. Console and page diagnostics were empty. This exercise made no provider request.

A controlled long-turn test committed a host command before compaction. The next request carried the summary and retained phase once. The command wrote one file marker, which stayed unchanged across interruption and reload. A view test rejects duplicate text from a live snapshot that precedes a phase commit.

A controlled browser fixture exercised live phase transitions and settlement. Response anchors stayed stable, phase text appeared once, and the unsent draft stayed intact. The settled copy contained both phases without tool or reasoning content. Console and page diagnostics were empty.

Successful hosted-model generation and hosted agent execution remain unverified. The placeholder credential exercised the generation error path only.

The browser reports no page or console errors. The tested mobile surfaces have no horizontal overflow at 390 pixels.

Browser accessibility audits report no violations on the tested surfaces. The candidate diff audit leaves short line-number contrast checks incomplete.

The design detector uses its fallback because HTML parser dependencies are unavailable. That fallback does not assess computed contrast.
