# UI overhaul

## Direction

`DESIGN.md` records the visual system. This tracker records the approved scope and implementation evidence.

The user approved the workspace proportions and lime accent. The other themes remain available.

The user rejected the starter buttons and the sidebar status footer. Neither appears in the implementation.

Existing capabilities and authority boundaries remain intact. Missing behaviour requires a user decision before implementation.

## Workload

Only one bounded pass is active. Each pass ends with a user review before the next pass starts.

| Stage | Scope                                                         | Status               |
| ----- | ------------------------------------------------------------- | -------------------- |
| 1     | Shared sidebar, conversation header, empty state and composer | Approved             |
| 2a    | Configured draft and directory setup                          | Completed            |
| 2b    | Execution setup, host consent and execution review            | Completed            |
| 2c    | Instructions setup and recorded sources                       | Completed            |
| 2d    | Presets setup                                                 | Completed            |
| 3a    | Active conversation                                           | Completed            |
| 3b    | Current work                                                  | Approved and staged  |
| 4a    | Candidate review                                              | Approved             |
| 4b    | Recovery                                                      | Awaiting user review |
| 5     | Workflows and resource catalogues                             | Queued               |
| 6     | Attention, history, providers and settings                    | Queued               |

Later stages require their own bounded scope and design decisions. Theme expansion remains outside Stage 1.

## Stage 1 scope

- Use the approved 300-pixel sidebar and readable navigation scale.
- Add the search field boundary and resource heading.
- Match the compact header and quiet empty state.
- Use a wider composer with one desktop control row.
- Retain image attachments and model selection.
- Retain file references, prompt commands and composer help.
- Keep directory access and execution location visible below the composer.
- Retain responsive navigation and keyboard access.

Stage 1 does not redesign Setup, active transcripts, review decisions or catalogue content. Shared styles can affect their navigation and composer.

## Acceptance

- The desktop composition uses the approved sidebar and composer proportions without fabricated conversations or status.
- The mobile layout has no horizontal overflow or inaccessible controls.
- The composer retains drafts through its popovers and attachment commands.
- The page retains real navigation links and Hypergraft protocol identifiers.
- No UI action grants directory access or starts model work without the existing command.
- Browser evidence covers desktop, mobile and a dark theme.
- `mise run clean` completes without errors or warnings.

## Evidence

The initial browser inspection used the existing server at `http://localhost:4000/conversations/new`.

No code changed before the user approved the direction and the omission of starter buttons and the sidebar status footer.

The implementation review used an isolated server at `http://127.0.0.1:4400`. Temporary data contained a placeholder provider key. No hosted-model request ran.

Browser exercises covered:

- Model selection and effort selection.
- Draft retention through model controls and Setup.
- Image selection, automatic upload and removal.
- File-reference insertion through composer help.
- Mobile navigation, Escape and focus restoration.
- Empty-message state and responsive control layout.

Screenshots cover these viewports:

- Desktop: 1586 × 956.
- Compact desktop: 1024 × 768.
- Mobile: 390 × 844.
- Narrow mobile: 320 × 568.
- Sector 7-G: 1440 × 900.

The screenshots live in `.impeccable/review/ui-overhaul/`. The narrow-mobile correction keeps the complete heading above the composer.

The browser reported no console or page errors during the exercises. Accessibility scans reported zero violations for mobile and the inspected dark-theme popovers.

One development reload received a 404 for `main.js` during an asset rebuild. Subsequent navigation loaded the assets successfully.

The mechanical design scan used its limited fallback parser. Its findings concerned design-record differences and existing styles. It did not establish computed contrast.

The visual review ran in-thread because this harness has no subagent tool. `DESIGN.md` now records the changed palette and shared controls.

The frontend suite passed all 113 tests. The Rust suite passed all 1338 tests. `mise run clean` completed without errors or warnings.

Hosted-model execution remains outside this stage's browser evidence. The user reviewed Stage 1 before the Stage 2a request.

## Stage 1 refinement

The user requested cleaner hover states and icons after the first review. This pass preserves the layout and existing behaviour.

Directory actions retain transparent backgrounds without hover underlines. The user rejected the underline treatment. The separator sits outside the button and its focus outline.

Header and composer controls use a neutral hover tint. Outlined controls also change their border colour. The paperclip and help icons use cleaner geometry.

The browser review covered desktop, mobile and Sector 7-G. Keyboard focus remains visible. The narrow-mobile heading remains fully visible without horizontal overflow.

Help and model popovers retained an unsent draft. The mobile accessibility scan reported zero violations. The browser reported no console or page errors.

Screenshots live in `.impeccable/review/ui-overhaul/refinement/`. The user approved the next bounded pass after this refinement.

## Stage 2a scope

This pass covers the configured draft summary and the Directories section of Setup. It inherits the approved Stage 1 shell.

The user explicitly requested a working command start-directory selector after clarification. The selector moves the chosen directory to the first position.

Directory order remains the command-location source. A changed order invalidates consent under the existing rules. Active work blocks the change.

Access modes and approvals remain separate. Saved access changes retain the existing execution review. Other Setup sections remain deferred.

## Stage 2a decisions

The configured draft uses “Work on your files”, not a readiness claim. Its paths and access states come from the current configuration.

Guest paths retain the real `/access/…` mount locations.

Current access stays inside each directory record. This keeps the applied mode beside its requested mode when several directories exist.

Non-sensitive Review before apply needs no additional directory consent. Its candidate decision still gates host writes. Direct write and sensitive access retain explicit approval.

The selector changes directory order, not the persisted format. It preserves directory identities and access modes. Invalid or unavailable destinations fail without substitution.

Saved changes retain revision checks and active-work guards. A changed order invalidates runtime consent and stored directory approvals under the existing rules.

Setup inherits the larger controls and uses up to 488 pixels on desktop. Other sections inherit this shell without a content redesign.

Short mobile screens retain the configured heading and the access strip. Full details remain in Setup.

## Stage 2a evidence

The browser review used the existing server for initial inspection only. The implementation review used `http://127.0.0.1:4400` with temporary data and a placeholder provider key.

Browser exercises covered:

- Draft and saved command-location selection.
- Access radios and explicit Direct write approval.
- Fresh approval after a changed directory order.
- Directory removal and unsent-message retention.
- Saved requested access without changes to applied access.
- Host work locations and unavailable directories.
- Mobile companion exclusion, Escape and focus restoration.
- Transparent button hover states without text underlines.

Saved execution review returned the expected 409 because the temporary environment was not ready. It changed no access mode.

The browser reported no console or page errors. Final accessibility scans reported zero violations for desktop, mobile and Sector 7-G.

The native chooser was not exercised through the browser. Rust tests cover picker selection, cancellation and revision-bound commands.

The browser exercise exposed a draft-loss bug after directory commands. The fix retains the unsent message. Frontend tests pin this command boundary.

Rust tests cover selector validation and authority. They cover stale revisions, active work and consent invalidation without model requests.

Screenshots live in `.impeccable/review/ui-overhaul/stage2a/`. They cover desktop, compact desktop, mobile, narrow mobile and Sector 7-G.

The mechanical detector used its limited fallback parser. Its advisory findings concern design-record differences and existing styles, not computed contrast.

The visual review and design documentation ran in-thread because this harness has no subagent tool. The bounded review disposition is `ship`.

The frontend suite passed all 120 tests. The Rust suite passed all 1340 tests. Development and production asset builds passed.

`mise run clean` completed without errors or warnings. Existing working-tree changes remain uncommitted. No hosted-model request ran.

The user authorised the bounded Execution pass after the Stage 2a presentation. Instructions and Presets remain deferred.

## Stage 2b decisions

This pass covers Execution setup, host consent and execution-change review.

The user approved two functional decisions after clarification:

- Saved network changes join execution review instead of immediate application.
- Host consent shows a fixed policy summary, with Change policy as a return to Setup.

The current configuration remains separate from requested values. The review table marks each changed row. Each directory retains its own access mode.

A draft collects execution choices without another save confirmation. Saved changes remain revision-bound. Stop-and-switch and discard-and-switch carry the requested network policy through their existing safeguards.

Host consent remains separate from execution review. Its modal names the actual process identity and start directory. It warns about unrestricted access and immediate file changes.

Cancel grants no consent and changes no settings. The interface does not promise a return to sandbox. Requested saved changes block consent for the old configuration.

Environment status uses the real preparation and snapshot records. No fabricated Ready badge appears. Sandbox network settings explicitly do not restrict host tools.

## Stage 2b evidence

The existing server at `http://localhost:4000` remained available. Authority-sensitive browser exercises used a separate temporary data directory at `http://127.0.0.1:4400`.

The fixture used a placeholder provider connection and temporary directories. No hosted-model request ran.

Browser exercises covered:

- Draft host selection and both command policies.
- Draft consent, cancellation and unsent-message retention.
- Saved location, policy and network review with successful application.
- Keep current settings without a command.
- Separate host consent after application.
- Change policy, Escape and focus restoration.
- Blocked consent while saved changes remain requested.
- Invalid domains and unavailable-environment errors without a settings change.
- Mobile companion exclusion and scroll access to decision controls.

The selected sandbox environment was not ready. Its preview returned 409 without a substitute environment. Successful sandbox execution remains outside this browser evidence.

Rust tests cover revision conflicts and consent invalidation. They cover network validation before cancellation or discard, plus the requested policy after successful settlement.

Active-task and exact-candidate settlement paths have Rust coverage, not browser evidence. Browser exercises started no task or host command.

Screenshots live in `.impeccable/review/ui-overhaul/stage2b/`. They cover desktop, compact desktop, mobile, narrow mobile and Sector 7-G.

Final accessibility scans reported zero violations on the inspected surfaces. Some Setup scans retained incomplete automated checks. This result does not establish accessibility for every state.

The browser reported no console or page errors. Early probes for optional development reload routes returned 404 on the temporary server.

The final server build included those development routes. The reload endpoint then returned 200. The temporary server stopped after the final interaction exercises.

The original server remained available at `http://localhost:4000`.

The mechanical detector used its limited fallback parser. Its 32 advisory findings concern existing styles and design-record differences, not computed contrast.

The visual review and documentation ran in-thread because this harness has no subagent tool. The bounded review disposition is `ship` for the captured states.

The frontend suite passed all 123 tests. The Rust suites passed 1,341 tests. Development and production asset builds passed.

`mise run clean` completed without errors or warnings. All existing working-tree changes remain uncommitted.

The user authorised Instructions after the Execution presentation. Stage 2b is complete.

## Stage 2c decisions

This pass covers Instructions setup. Presets and active-conversation redesign remain deferred. Model and effort controls stay in the composer.

The editor retains the approved workspace scale. Tool rows replace the earlier cards. Select all retains partial and explicit empty choices.

Draft feedback names the unsaved state. Saved feedback distinguishes pending commands from successful saves. Rejected saves retain later edits without an automatic retry.

Validation enforces the existing 32 KiB bound in UTF-8 bytes. It rejects unsupported control characters. Invalid submissions retain instructions and the unsent message.

The user chose recorded instruction sources after clarification. Setup lists instruction files from the latest recorded reply request on the active branch.

The list excludes summarisation requests and advertised skills. It shows at most eight paths and links to the complete request context.

Recorded sources grant no access and make no promise about the next request. No current-access badge appears. Missing records and unavailable records remain distinct.

Tool explanations distinguish sandbox access from unrestricted host access. Instruction edits retain consent. Tool changes retain the existing consent invalidation.

## Stage 2c evidence

The existing server at `http://localhost:4000` supplied the initial inspection. Browser commands used isolated temporary data at `http://127.0.0.1:4400`.

The fixture contained a placeholder provider connection and temporary directories. A synthetic reply supplied recorded source evidence. No model request or host command ran.

Browser exercises covered:

- Saved instruction edits and unsent-message retention.
- Select all, partial choices and explicit empty choices.
- Draft retention across sections and tool commands.
- UTF-8 validation, error focus and correction on mobile.
- Recorded source paths and the exact request-context link.
- Separate host consent followed by an instruction edit that retained consent.
- A tool change that required fresh host consent.
- Mobile companion exclusion, Escape and focus restoration.

Frontend tests cover later edits during pending commands and rejected saves. They also cover uncertain results without a false save confirmation.

Rust tests cover invalid instructions without state changes. They cover source identity across branches and transcript windows. Empty latest sources do not substitute older sources.

Screenshots live in `.impeccable/review/ui-overhaul/stage2c/`. They cover desktop, compact desktop, mobile, narrow mobile and Sector 7-G.

Final accessibility scans reported zero violations on the inspected desktop, mobile and dark-theme surfaces. Mobile and dark-theme scans retained incomplete automated checks.

The browser reported no console or page errors. Early automation clicks reached moving or clipped controls before explicit animation waits and panel scrolls.

The final exercises used settled panels. The source fixture initially had an invalid metadata value. A corrected record supplied the final source evidence.

The mechanical detector used its limited fallback parser. Its 32 advisory findings concern existing styles and design-record differences. None targeted the new Instructions styles.

The visual review and documentation ran in-thread because this harness has no subagent tool. The bounded review disposition is `ship` for the captured states.

The frontend suite passed all 129 tests. The Rust suites passed 1,344 tests. Development and production asset builds passed.

`mise run clean` completed without errors or warnings. Existing staged and unstaged changes remain uncommitted. The user authorised Presets after Stage 2c.

The named browser session closed and the temporary server stopped. The original server remained available at `http://localhost:4000`.

## Stage 2d decisions

This pass covers Presets setup. The preset catalogue and active-conversation redesign remain outside this pass.

The user chose saved settings only for snapshots from saved conversations. Unreviewed execution changes and unsaved edits stay excluded. Drafts supply current choices.

The replacement preview compares every setting before explicit confirmation. Each setting labels two value columns within the approved companion width.

Changed rows use a tint and an explicit label. Complete instruction text remains available through bounded scroll regions. Requested directories retain real paths and access modes.

The save panel shows a name field and the exact settings snapshot. Instructions use a separate disclosure. The existing empty-name suggestion remains available.

Name validation enforces the 80-byte UTF-8 bound and rejects control characters. Invalid names remain editable. A hidden name field does not block unrelated draft commands.

Preview and save preserve uncommitted setup edits. Rejected replacement also preserves them. Successful replacement supersedes later ordinary edits, rather than restores them over the confirmed preset.

Every preset command retains the unsent message. Saved snapshots remain independent. Presets copy no directory approval or runtime consent.

The user questioned the deletion volume during implementation. The audit separated extracted templates and presentation code from retained authority checks. No earlier working-tree changes were reverted.

## Stage 2d evidence

The existing server supplied the initial inspection. Command exercises used temporary data at `http://127.0.0.1:4400` with a placeholder provider connection.

The fixture contained one synthetic saved conversation. Draft commands created no additional conversation. No model request or host command ran.

Browser exercises covered:

- Named snapshots from drafts and saved conversations.
- Saved snapshots that exclude unreviewed execution changes.
- Full replacement previews and cancellation without a command.
- Successful replacement with unsent-message retention.
- A stale revision that returned 409 without replacement.
- A fresh preview after the revision conflict.
- UTF-8 validation and retained names across section changes.
- Real directory paths after a draft patch.
- Separate host consent after a host preset replacement.
- Fresh host consent after replacement of an already approved configuration.
- Mobile scroll access, Escape and focus restoration.

Frontend tests pin successful and rejected replacement boundaries. They cover saved-only summaries and safe text insertion. Rust tests pin snapshot validation and revision-bound saves.

The first inspection exposed omitted draft directory metadata after patches. A hidden data container replaced the nested template. The final preview retained the real paths.

The visual review found clipped header actions at 320 × 568. The header now retains its full content height above Setup.

The final narrow-mobile exercise retained the composer and Send after closure. No horizontal overflow appeared in the inspected views.

Screenshots live in `.impeccable/review/ui-overhaul/stage2d/`. They cover desktop, compact desktop, mobile, narrow mobile and Sector 7-G.

Accessibility scans reported zero violations for the inspected save and preview panels. Mobile scans retained one incomplete skip-link check. Desktop and dark preview scans had no incomplete checks.

The browser reported no console or page errors in the completed exercises. The network log contains the expected revision conflict and development reload traffic.

The temporary server initially rejected an incomplete fixture root. The corrected fixture retained a private data directory and its ownership marker.

The mechanical detector used its limited fallback parser. Its 32 advisory findings concern existing styles and design-record differences. None targeted the new Presets styles.

The visual review and documentation ran in-thread because this harness has no subagent tool. The final review scored the header correction resolved, with disposition `ship`.

The frontend suite passed all 140 tests. The Rust suites passed 1,346 tests. Development and production asset builds passed.

`mise run clean` completed without errors or warnings. Existing staged and unstaged changes remained uncommitted at presentation. The user subsequently authorised Stage 3a.

The named browser session closed and the temporary server stopped. The original server remained available at `http://localhost:4000`.

## Stage 3a decisions

This pass covers the active transcript and composer. Current work remains Stage 3b. Review and recovery remain Stage 4.

The user chose existing reply statuses instead of model-generated descriptions. The mobile activity strip mirrors the server status without another model request.

The strip opens Current work without a command. If another companion route is active, its native link returns to the canonical conversation route.

Historical windows suppress the strip, including windows that contain a pending entry. Settled replies also suppress it.

Messages use larger text and separate author labels. Avatars sit beside desktop content. Mobile bodies use the full transcript width.

Revise and Fork from here move below their messages. Copy remains below the response. Tool records retain their actual labels and existing disclosures.

The composer retains model and effort controls. Queue delivery uses a labelled selector with the existing values. Stop retains its job-bound command and no confirmation.

Short mobile screens expose composer controls through horizontal scroll. Queue and Stop retain their own row. Queues and composer content use bounded scroll areas.

The first inspection found Stop below the viewport at 320 × 568. The corrected layout retains the complete header and transcript scroll area.

A retained composer previously kept the Queue label after Stop. It now reads the latest server-authored label after each patch.

This pass removes no domain flow or authority safeguard. Template movement accounts for most deleted markup. The queue selector replaces the two radio labels.

The working tree was clean at `87691c7` when Stage 3a began. The earlier UI work remains in that commit. This pass creates no commit.

## Stage 3a evidence

Initial inspection used the existing server. Browser commands used an isolated server at `http://127.0.0.1:4400` with ephemeral stores and synthetic jobs.

The temporary fixture used the application router and security middleware. Scripted events reached the real observation route. No model request or host command ran.

Browser exercises covered:

- Live thought, tool, reply and retry statuses.
- Stop and normal completion, with the return to Send.
- Queue delivery through the existing keyboard shortcut.
- Removal of queued corrections without loss of a later draft.
- Image upload and removal without loss of the draft.
- Show thinking and response copy.
- Prompt revision, explicit confirmation and cancellation with draft restoration.
- Tree navigation and the historical transcript window.
- Scroll retention during new output and Jump to latest.
- Mobile Current work exclusion, Escape and focus restoration.
- Return from workflow setup to Current work through the status strip, without a workflow launch.
- The complete narrow-mobile header above Setup.
- New drafts and the model popover at 320 × 568.

Frontend tests pin the server label after patches and the shortcut's submitter identity. The status test rejects transcript content as an activity source.

The status test also covers safe text insertion and historical windows with pending entries. Existing Rust coverage retains the canonical observation route and job-bound cancellation.

Screenshots live in `.impeccable/review/ui-overhaul/stage3a/`. They cover desktop, compact desktop, mobile, narrow mobile and Sector 7-G.

Accessibility scans reported zero violations on the inspected desktop, mobile and dark-theme surfaces. Mobile and dark scans retained incomplete contrast checks for clipped scroll content.

The final browser session reported no console or page errors. One earlier asset rebuild produced a temporary `main.js` 404. Later requests loaded the assets successfully.

Early automation encountered a clipped Revise link and an explicit revision dialog. The final exercise used transcript scroll and explicit dialog acceptance.

The mechanical detector used its degraded parser. Its 33 advisory findings concern existing styles and design-record differences. The documented tool-record scale accounts for one new finding.

The review and documentation ran in-thread because this harness has no subagent tool. The verdict scored both listed corrections resolved, with disposition `ship`.

The frontend suite passed all 145 tests. The Rust suites passed 1,346 tests. Development and production asset builds passed.

`mise run clean` completed without errors or warnings. `git diff --check` passed. The named browser session and temporary server closed.

The temporary fixture source remains under `/tmp/powerplant-active-TpjTmC` for evidence. No fixture hook or test server remains in application code.

The original server remains available at `http://localhost:4000`. The user subsequently authorised Stage 3b after the tree correction. The Impeccable sidecar refresh remains outside the authorised scope.

## Stage 3a tree correction

The user reported overlapping text in Conversation tree. The shared `record-row` class imposed a two-column grid on entries that expected a vertical layout.

The tree template now uses local Tailwind layout utilities for entries and branch tips. Metadata and excerpts occupy separate rows. Long unbroken text wraps within the companion.

The correction changes no branch command or navigation destination. Browser exercises used read-only navigation on the reported conversation. No model request or host command ran.

Desktop and mobile exercises covered search, Children navigation and the return to all entries. Escape restored focus to Tree and removed the composer exclusion.

The 320-pixel viewport retained the full page width without horizontal overflow. A temporary browser-only text fixture also retained the entry width.

The eight tree tests passed. Accessibility scans reported zero violations, with one incomplete check on each inspected viewport.

The layout detector reported no findings through its degraded parser. The visual review ran in-thread and returned `ship` for the corrected records.

Screenshots live in `.impeccable/review/ui-overhaul/stage3a/tree-fix/`. The original server remains available. The named browser session closed.

Development and production asset builds passed. `mise run clean` completed without errors or warnings. The earlier Stage 3a changes remain intact.

## Stage 3b decisions

This pass covers Current work status, attention controls and responsive layout. Candidate review, recovery and the preset catalogue remain outside the pass.

Execution status and decisions precede Context and usage. That disclosure retains the existing estimates, compaction details and summary usage.

Ordinary replies repeat server-authored status. Retry details share the reply metadata and disappear when the retry ends. No additional model request supplies activity descriptions.

Workflow progress shows the pinned workflow name, recorded state and current phase. It adds no inferred percentage or completion count.

Questions and execution pauses expose the existing controls through attention strips. Their commands retain the original identities and consent rules.

The companion retains eligible Stop controls outside its content scroll area. A question uses the same job-bound form instead of a separate form inside its content.

Stop during a question keeps observation active until settlement. The cancellation flag remains set. This change starts no replacement execution.

Native attention links and Back to current work open the canonical conversation route. Safe observation resumes after companion navigation without a duplicate loop from observation URLs.

The desktop companion retains its 400-pixel width. Its header uses larger text and a 44-pixel Close control. Mobile retains the same content order.

Compact desktop composer controls scroll horizontally within their container. Queue and Stop stay below those controls without overlap with the companion.

A mobile resize moves focus out of excluded conversation controls. Escape then closes the companion and restores the trigger.

No new module extraction occurred. Most deleted template lines moved into the Context and usage disclosure or the persistent Stop footer.

## Stage 3b evidence

Initial inspection used the existing server. Browser commands used ephemeral stores and synthetic jobs at `http://127.0.0.1:4400`.

The fixture used the actual router, security middleware and observation routes. No model request or host command ran. The user conversation stayed unchanged.

Browser exercises covered:

- Live tool, thought, reply and retry statuses.
- Retry settlement after navigation to Activity and back.
- Stop during a reply and a question, with retained unsent text.
- Normal completion and the return to Send.
- Question choice submission without directory or command authority.
- Host approval presentation and rejection without execution.
- Attention navigation from the tree to the canonical Current work panel.
- Workflow state and current-phase presentation.
- Context disclosure, historical windows and the return to latest output.
- Mobile exclusion, Escape and focus restoration after a viewport change.
- Compact desktop controls and the narrow-mobile Stop footer.

The first inspection found a compact composer overlap and a question cancellation without a settlement update. Both faults received corrections and regression coverage.

The navigation exercise also exposed a retained observer that did not restart after safe navigation. The correction restarts only safe observation, not an unsafe command.

Retry metadata now follows both progress and final frames. Frontend tests reject transcript content as a status source and insert retry details as text.

Rust tests pin question cancellation and observation metadata. Existing tests retain candidate identities, partial-application safeguards and continuation consent.

Pause controls received visual inspection. The attempted End pause browser click sent no request. Continue and End pause command behaviour relies on existing Rust coverage.

Screenshots live in `.impeccable/review/ui-overhaul/stage3b/`. They cover desktop, compact desktop, mobile, 320 × 568 and Sector 7-G.

Accessibility scans reported zero violations on the inspected surfaces. Each scan retained one incomplete contrast check. These results do not establish accessibility for every state.

The browser reported no console or page errors during the completed exercises. Early automation reached moving controls before explicit animation waits.

The mechanical detector used its degraded parser. Its 27 advisory findings concern existing styles and design-record differences, not computed contrast.

The visual review and documentation ran in-thread because this harness has no subagent tool. The captured layout is ready for user review.

The frontend suite passed all 147 tests. The Rust suites passed 1,348 tests. Development and production asset builds passed.

`mise run clean` completed without errors or warnings. `git diff --check` passed. The named browser session closed and the isolated server stopped.

The fixture source remains under `/tmp/powerplant-current-work-2JSLFz`. No fixture hook remains in application code. Hosted-model execution remains outside this evidence.

The original server remains available at `http://localhost:4000`. The staged patch remains byte-identical to its initial snapshot. All earlier work remains uncommitted.

The user subsequently approved and staged Stage 3b. Stage 4a follows that approval. The Impeccable sidecar refresh remains outside the authorised scope.

## Stage 4a decisions

This pass covers candidate review in the actual application. Recovery remains Stage 4b. The preset catalogue remains outside the pass.

Changed files and the selected diff precede metadata and decisions. Wide review containers use adjacent columns. Narrow containers stack them.

The companion retains local expansion and its canonical full-review link. File selection reveals and focuses the preview without a command.

The full review selects the first file on each manifest page by default. File metadata and immutable downloads remain available through disclosures.

The interface distinguishes binary, oversized and unavailable previews. Complete evidence supplies counts. Preview limits never change the approval scope.

Approval labels follow the pinned next command, not the candidate format. File application, local Git commits and configured continuation remain distinct.

The decision controls name the destination. Commit approval retains its partial-commit warning. Discard explicitly leaves earlier direct writes and host effects intact.

Exact candidate hashes and gate revisions remain on every consequential form. Runtime consent and continuation ownership retain their existing checks.

The browser exercise exposed an existing draft-loss fault after successful companion decisions. Those commands now return conversation patches instead of full reloads.

The patch retains unsent text. If a decision removes the focused control, the companion receives focus after settlement. Escape then restores the conversation trigger.

Recovery controls and execution safeguards remain unchanged. No persisted format, dependency or fixture hook enters the application.

## Stage 4a evidence

The existing server remained available at `http://localhost:4000`. Authority-sensitive browser exercises used ephemeral stores at `http://127.0.0.1:4400`.

The isolated fixture supplied synthetic candidates to the actual router, security middleware and templates. It did not replace the application interface.

Browser exercises covered:

- Local file selection, expansion and canonical full-review navigation.
- Manifest pagination and text pagination with exact candidate fields intact.
- Added, removed, binary and oversized files, plus long paths and escaped hostile text.
- Immutable binary downloads through the existing route.
- Distinct file-application, commit and configured-continuation consequences.
- Rejected stale feedback and wrong-candidate approval, without file application.
- Expired consent, with absent full-page controls and disabled companion controls.
- Feedback disclosure, cancellation and focus restoration.
- Exact-candidate discard through the real command route.
- Draft retention, Send restoration and mobile Escape after discard.
- Desktop, compact desktop, mobile, narrow mobile and Sector 7-G.

The first visual pass exposed a duplicate Expand control and an oversized footer. One correction batch removed the duplicate and reduced the footer.

The addition count failed contrast on a selected row. Its corrected colour derives from the theme success and text tokens.

Early automation reached controls during transitions or outside their scroll area. Final command exercises waited for settled panels and used the bounded footer scroll area.

Successful approval and revision dispatch retain Rust coverage. The browser did not approve file application, create a Git commit or start a model request.

The synthetic host baseline remained unchanged after rejected decisions and discard. The user conversation remained untouched.

Regression tests cover:

- Exact decision fields across conversation and canonical representations.
- Approval consequences from the pinned next step.
- Candidate escaping and bounded diff navigation.
- Preview selection without authority or loss of native modified navigation.
- Conversation patches after approval, feedback and discard.
- Draft retention and mobile focus after decision controls disappear.

The frontend suite passed all 152 tests. The Rust suites passed 1,349 tests. Development and production asset builds passed.

Final accessibility scans reported zero violations on the inspected surfaces. Scans retained one or two incomplete automated checks, so the evidence is not exhaustive.

Final browser diagnostics contained no console or page errors. The network log contains the expected 409 responses for stale decisions.

One development request for `main.js` returned 404. Later requests loaded the asset successfully.

The mechanical detector used its degraded parser. Its 26 advisory findings concern existing styles and design-record differences, not the new review layout.

The visual review ran in-thread because this harness has no subagent tool. Screenshots live in `.impeccable/review/ui-overhaul/stage4a/`.

`mise run clean` completed without errors or warnings. Both staged and unstaged diff checks passed.

The named browser session closed and the isolated server stopped. Temporary fixture sources remain under `/tmp/powerplant-stage4a`.

The original server remains available at `http://localhost:4000`. The staged patch remains byte-identical to the initial snapshot. Stage 4a remains unstaged and uncommitted.

The user approved Stage 4a and authorised the bounded Recovery pass. Approval grants no permission to stage or commit.

## Stage 4b decisions

This pass covers recovery presentation in the actual application. Workflow and resource catalogues remain outside the pass.

Current work separates recorded file outcomes from managed cleanup. Failed and interrupted runs no longer hide unresolved file application inside Execution details.

A stopped application can contain no applied directories. The summary uses the recorded outcomes without an invented conflict cause or success claim.

Directory results retain the recorded alias and host path. Each outcome has a short explanation. The results describe the attempt, not current files.

Repository results retain their separate transaction states. Only a completed commit supplies a commit identifier. Success in one repository establishes no success in another.

Current work and Activity share the read-only evidence presentation. View attempt evidence opens the exact attempt and starts no write or retry.

The canonical attempt page now includes its recorded directory and repository results. It no longer relies on candidate outputs alone for application evidence.

Keep applied files and end task retains its existing command. It binds the displayed run, attempt and transaction state.

Settlement keeps files and evidence. It records Cancelled, not Completed. Uncertain outcomes and incomplete cleanup retain the existing blockers.

The interface adds no retry, rollback or recovery authority. The workflow application and settlement logic remain unchanged.

Activity no longer promises that plain conversation bypasses recovery. Unsettled file or repository work omits the local Continue the conversation control.

The recovery attention strip opens Current work without a command. The toolbar contains the strip, so the desktop grid cannot place it outside the viewport.

A rejected mobile command exposes its authoritative error outside the companion. Closure retains the draft and releases the hidden composer controls.

The error occupies its own grid row. It no longer covers the toolbar. Recovery controls retain the larger workspace scale.

## Stage 4b evidence

The existing server remained available at `http://localhost:4000`. The protected user conversation remained untouched.

Authority-sensitive browser exercises used ephemeral stores at `http://127.0.0.1:4400`. Synthetic records supplied partial, unchanged and uncertain file outcomes.

Other records supplied incomplete cleanup and mixed repository results. These records were preview data, not real file applications or Git commits.

The temporary fixture compiled the application router, security middleware and templates. No fixture hook entered application code.

The previous `/tmp/powerplant-stage4a/` fixture was absent. This pass created a separate fixture under `/tmp/powerplant-stage4b/`.

Browser exercises covered:

- Recorded directory outcomes and separate cleanup status.
- Mixed repository outcomes without an unverified commit identifier.
- Exact-attempt evidence navigation in a separate tab.
- Rejected stale settlement without another file application.
- Successful settlement through the existing cancellation command.
- Draft retention after rejection, settlement and Activity navigation.
- Mobile exclusion, error focus, Escape and Continue the conversation.
- Desktop, compact desktop, mobile, narrow mobile and Sector 7-G.

The browser found an off-screen attention strip and a covered mobile error. The correction exposed a toolbar overlap, which received a separate error row.

The canonical evidence scan found an incorrect heading level. The template now uses page-level headings outside the companion.

Early screenshots captured incomplete panel transitions. Later captures and interactions used settled panels. One browser navigation reached a blank tab and required a fresh navigation.

Rust regression tests cover stale attempts, transaction states and wrong owners. They reject uncertain outcomes, incomplete cleanup and redundant terminal settlement.

The tests also pin exact evidence links and escaped directory text. They distinguish completed commit identifiers from unverified identifiers across the conversation and canonical page.

Frontend regression tests cover retained drafts and mobile focus after settlement. They also cover the authoritative error after a rejected command.

The frontend suite passed all 155 tests. The Rust suites passed 1,351 tests. Development and production asset builds passed.

No model request or host execution ran during the browser exercises. The synthetic host baseline retained its original contents.

The visual review ran in-thread because this harness has no subagent tool. Screenshots and the review record live in `.impeccable/review/ui-overhaul/stage4b/`.

Final accessibility scans reported zero violations. They retained zero or one incomplete automated check. These results do not establish accessibility for every recovery state.

Final browser diagnostics contained no console or page errors. The stale settlement produced the expected 409 response.

The early temporary server omitted the development reload routes and returned 404. The final fixture included those routes and returned 200.

The browser tool also reported one connection failure after closure. Its diagnostic command reported no warning or failure. A fresh navigation succeeded.

The mechanical detector used its degraded parser and reported 26 advisory findings in existing workspace styles. It did not evaluate computed contrast.

`mise run clean` completed without errors or warnings. Both staged and unstaged diff checks passed.

The named browser session closed and the isolated server stopped. The original server remains available at `http://localhost:4000`.

Stage 4b awaits user review. The Git index remains byte-identical to its initial snapshot. No Stage 4a or Stage 4b change is staged or committed.

The Impeccable sidecar refresh remains outside the authorised scope.
