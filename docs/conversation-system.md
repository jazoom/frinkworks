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

## Live execution

Ordinary messages with tools use directory-backed internal agent execution. The internal execution record does not require workflow selection.

Tool-free messages use the normal chat job. Host tools require explicit host consent and the selected command policy.

Configured workflows retain their explicit sequences. Directory grants pin directory identities for handoff and recovery.

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

## Ownership and recovery

The existing run record remains the sole ownership authority. Transfer never copies a run or recaptures its baseline.

The conversation lock protects source and destination revisions through the ownership commit. A run fingerprint rejects stale or concurrent transfers.

A bounded ownership history preserves review provenance. Decision revisions include the ownership generation, so source-page forms cannot approve transferred changes.

A pending handoff journal resides in the run record. The application flushes that journal before it stores the source and destination projections.

A failed projection retains the journal and blocks execution. Startup completes the projections idempotently. A failed pre-commit write leaves ownership with the source.

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

- 1,020 Rust unit tests.
- One additional Rust test.
- 81 browser-unit tests.

`mise run clean` passes without warnings. Production and development asset builds pass.

The real browser checks use `http://localhost:4000` with synthetic persisted records and a placeholder provider credential.

The checks cover these paths:

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

Successful hosted-model generation and hosted agent execution remain unverified. The placeholder credential exercised the generation error path only.

The browser reports no page or console errors. The tested mobile surfaces have no horizontal overflow at 390 pixels.

Browser accessibility audits report no violations on the tested surfaces. The candidate diff audit leaves short line-number contrast checks incomplete.

The design detector uses its fallback because HTML parser dependencies are unavailable. That fallback does not assess computed contrast.
