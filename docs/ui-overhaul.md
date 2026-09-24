# UI overhaul

## Direction

The mocks in `i/` define the visual reference. Stage 1 uses `i/001-new-conversation.png` without its annotation strip.

The user approved its proportions and lime accent. The other themes remain available.

The user rejected the starter buttons and the sidebar status footer. Neither appears in the implementation.

Existing capabilities and authority boundaries remain intact. Missing behaviour requires a user decision before implementation.

## Workload

Only one bounded pass is active. Each pass ends with a user review before the next pass starts.

| Stage | Scope                                                         | References            | Status               |
| ----- | ------------------------------------------------------------- | --------------------- | -------------------- |
| 1     | Shared sidebar, conversation header, empty state and composer | 001, 071              | Approved             |
| 2a    | Configured draft and directory setup                          | 002, 004              | Completed            |
| 2b    | Execution setup, host consent and execution review            | 007, 010, 011         | Completed            |
| 2c    | Instructions setup and recorded sources                       | 008                   | Completed            |
| 2d    | Presets setup                                                 | 042, 079              | Awaiting user review |
| 3     | Active conversation and Current work                          | 003, 012–023, 071–072 | Queued               |
| 4     | Change review and recovery                                    | 006, 024–033, 073     | Queued               |
| 5     | Workflows and resource catalogues                             | 034–052               | Queued               |
| 6     | Attention, history, providers and settings                    | 053–070               | Queued               |

Later stages require their own bounded scope and reference selection. Theme expansion remains outside Stage 1.

## Stage 1 scope

- Match the reference sidebar width and readable navigation scale.
- Add the search field boundary and resource heading.
- Match the compact header and quiet empty state.
- Use a wider composer with one desktop control row.
- Retain image attachments and model selection.
- Retain file references, prompt commands and composer help.
- Keep directory access and execution location visible below the composer.
- Retain responsive navigation and keyboard access.

Stage 1 does not redesign Setup, active transcripts, review decisions or catalogue content. Shared styles can affect their navigation and composer.

## Acceptance

- The desktop composition follows 001 without fabricated conversations or status.
- The mobile layout has no horizontal overflow or inaccessible controls.
- The composer retains drafts through its popovers and attachment commands.
- The page retains real navigation links and Hypergraft protocol identifiers.
- No UI action grants directory access or starts model work without the existing command.
- Browser evidence covers desktop, mobile and a dark theme.
- `mise run clean` completes without errors or warnings.

## Evidence

The initial browser inspection used the existing server at `http://localhost:4000/conversations/new`.

No code changed before the direction and the two mock omissions received user approval.

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

Guest paths retain the real `/access/…` mount locations. The mock's `/workspace/…` paths do not replace them.

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

This pass covers Execution setup, host consent and execution-change review. References 007, 010 and 011 supply the composition.

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

This pass covers Instructions setup against reference 008. Presets and active-conversation redesign remain deferred. Model and effort controls stay in the composer.

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

This pass covers Presets setup against references 042 and 079. The preset catalogue and active-conversation redesign remain outside this pass.

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

`mise run clean` completed without errors or warnings. Existing staged and unstaged changes remain uncommitted. Stage 2d awaits user review.

The named browser session closed and the temporary server stopped. The original server remained available at `http://localhost:4000`.
