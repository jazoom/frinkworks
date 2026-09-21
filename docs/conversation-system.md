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

- 1,111 Rust library tests.
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

Successful hosted-model generation and hosted agent execution remain unverified. The placeholder credential exercised the generation error path only.

The browser reports no page or console errors. The tested mobile surfaces have no horizontal overflow at 390 pixels.

Browser accessibility audits report no violations on the tested surfaces. The candidate diff audit leaves short line-number contrast checks incomplete.

The design detector uses its fallback because HTML parser dependencies are unavailable. That fallback does not assess computed contrast.
