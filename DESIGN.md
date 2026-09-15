---
name: Power Plant
description: A local conversation workspace with an optional work companion.
colors:
    canvas: "#f6f7ef"
    paper: "#fbfcf6"
    paperGreen: "#f0f2e7"
    cover: "#293625"
    coverText: "#f0f3e3"
    coverMuted: "#bfc9b3"
    ink: "#26352c"
    quietInk: "#586853"
    rule: "#d6dccc"
    action: "#edcf49"
    actionInk: "#283321"
    error: "#a63b32"
typography:
    title:
        fontFamily: "IBM Plex Sans, ui-sans-serif, system-ui, sans-serif"
        fontSize: "18px"
        fontWeight: 600
    result:
        fontSize: "24px"
        fontWeight: 600
    section:
        fontSize: "16px"
        fontWeight: 600
    body:
        fontFamily: "IBM Plex Sans, ui-sans-serif, system-ui, sans-serif"
        fontSize: "14px"
        lineHeight: 1.55
    code:
        fontFamily: "IBM Plex Mono, ui-monospace, monospace"
        fontSize: "12px"
    expandedCode:
        fontSize: "13px"
    metadata:
        fontSize: "12px"
    small:
        fontSize: "11px"
rounded:
    control: "2px"
    record: "3px"
    composer: "4px"
spacing:
    sm: "8px"
    md: "16px"
    panel: "20px"
    lg: "24px"
---

# Design system: Power Plant

## Overview

**Creative north star: "Conversation + work companion"**

The conversation is the work destination. An olive index sits beside a pale transcript and an optional work companion.

The conversation and workflow setup share the companion structure. Catalogue authoring remains a separate flow.

## Colors

Springfield uses pale green paper and sunshine actions. Thin rules separate the transcript, companion and controls.

The default setting follows the system preference. Light mode uses Springfield. Dark mode uses Sector 7-G.

An explicit theme choice overrides the system preference. The Settings selector can restore the system preference.

The five themes retain distinct palettes:

- Springfield uses olive and pale green.
- Evergreen Terrace uses deep teal and marigold.
- Leftorium uses light neutral surfaces.
- Stonecutters uses deep blue and cool blue actions.
- Sector 7-G uses deep violet and lime actions.

`app/assets/workspace.css` supplies the workspace palette and maps it to the existing material tokens. `app/assets/input.css` retains shared controls and catalogue styles.

Diff additions and removals retain their explicit markers. Colour supplements those markers and never replaces them.

Leftorium uses darker status text on selected recent records. Provider introductions use the same cover text tokens as the index.

Error panels use a pale error surface and readable recovery links. Action colours change together during theme changes.

## Typography

IBM Plex Sans carries the interface. IBM Plex Mono identifies paths and code.

Conversation text has a maximum measure of 70 characters. Page titles remain compact. Result headings identify the next decision.

Catalogue forms retain their existing type scale. The workspace's smaller metadata is not a new standard for all form text.

First-use chooser headings use 30px, and connection headings use 28px. Narrow screens use 28px and 26px respectively. Introductory text uses 15px.

## Layout

The desktop index occupies 230 pixels. The optional work companion occupies 400 pixels. The conversation fills the remaining width.

The transcript and companion content scroll independently. The composer stays outside the transcript scroll area.

The index leaves the page below 1021 pixels. Menu provides the same navigation destinations.

Below 701 pixels, the companion fills the conversation area. The hidden transcript, composer and conversation toolbar become inert.

Expanded review occupies the conversation width without a new conversation or URL. A separate native link opens the canonical candidate review page.

## Elevation & Depth

Surface tones and thin rules establish boundaries. The composer has a low shadow. Setup has no modal backdrop or centred dialog shadow.

Protected consent and destructive actions retain their existing explicit forms and confirmations. A companion transition grants no authority.

## Shapes

Controls retain DaisyUI's slight corners. Recent records and user messages have three-pixel corners. The composer has four-pixel corners.

The existing Power Plant mark remains unchanged. Workspace icons use the approved reference's stroke geometry.

## Components

### Navigation

New conversation remains prominent. The index contains up to twelve server-derived recent conversations with real titles and status.

The sidebar search filters those recent titles live as plain text. The catalogue link beside it stays the native fallback. The filter survives live replacement of recent records.

Needs your attention carries the positive server decision count. The live projection refreshes the count with the recent list. Recent records show state dots. Untouched saved records read Draft. Responsive idle records read Ready. Review, completion and cancellation transitions stay live.

Conversation pages carry their own header with the mobile menu trigger. The separate location bar stays for catalogue pages that need navigation and execution status.

Needs your attention lists real unresolved decisions with owning context links. An optional conversation identifier selects the return destination only: valid context shows Back to conversation, while any other value omits the return link. Refresh and decision pages preserve valid context, and every decision stays visible. History connects conversations to runs and evidence.

The sidebar resource group links directly to Workflows, Presets, Environments and Providers. More resources contains Projects and Agents. Settings stays separate below the group.

The sidebar has no local status footer. Catalogue headers omit generic return links to conversations.

Workflows and Presets each combine use and management on their canonical page. An optional conversation identifier selects the destination only.

Valid context retains Back to conversation. Without valid context, the selected resource offers a conversation chooser on its own page.

Stale workflow or preset identities report an error without substitution. Resource navigation starts no work and grants no access. The breadcrumb group reads Resources.

### Transcript and composer

User messages use a tinted, ruled surface. Assistant messages identify Power Plant with its mark.

The model control sits inside the composer with a dropdown chevron. The composer uses two rows with an 8000 character editor limit. The persisted message bound stays in the conversation store. The send control reads Send message. Effective directory access appears below it with a folder or shield icon and a visible Sandbox label. Project access stays beside the composer even with no attached project. An empty project catalogue links to project registration. Host mode names unrestricted access. Job-bound cancellation reads Stop task beside the composer and in Current work. The composer stays locked while a candidate or host command awaits a decision.

Jump to latest appears when the reader leaves the transcript end. New output does not move the reader away from earlier messages.

### Conversation header

New conversations show the title without an explanatory subtitle. Saved records identify their directory context beside the conversation title.

The header offers Handoff, Setup and Conversation actions. Conversation actions contains an independent draft copy, rename and explicit deletion.

Current work appears when work is non-idle and its companion is closed. A Needs your review strip opens the companion without approval.

Narrow screens wrap the actions below a long title. All actions retain readable labels.

### Current work

Current work contains execution progress and required decisions. Candidate review shows real changed files and bounded diff previews. Diffs and Markdown code blocks receive keyboard focus. The idle companion reads Ready when you are with View activity and evidence and Continue the conversation. Local review expansion stays within the conversation URL and one native link opens the canonical gate page.

Per-file addition and removal counts derive from the complete stored diff. Binary or oversized changes omit counts rather than infer them from truncated previews. The companion lists the total changed-file count and notes when only the first paths render. Recorded test outcomes are not part of the candidate evidence, so neither review surface shows a test result line. These omissions are deliberate: counts and test lines appear only when recorded data supports them.

The candidate footer has a bounded scroll area for long destinations and feedback forms. Current work retains the eligible job-bound cancellation control.

The approval footer names the destination and the actual application consequence. It distinguishes ordinary file application from a local Git commit. It distinguishes configured continuation to the next step from both file outcomes.

Host approval reads Run this command. It shows the exact command, work location and effective approval policy beside the decision. Session-bound approval and rejection evidence remains in the run record.

Request changes retains candidate-bound feedback. Discard posts directly without a confirmation dialog. Discard keeps evidence and history, so it is not destructive in the data sense. Discard does not reverse direct writes or host command effects.

Current work shows per-directory file-application outcomes from the authoritative run transaction record. Known partial application stays distinct from uncertain recovery and successful completion. Resolve the conflict links to the exact run attempt and its directory evidence. The link opens evidence and starts no write, retry or simulated resolution. This manual recovery destination is the production difference from the mock simulated conflict button.

Keep applied files and end task appears only when every transaction holds a known settled outcome and managed cleanup succeeded. Settlement binds the displayed run, attempt and outcome state. It keeps applied files and evidence without another application attempt, and ends ownership through cancellation rather than a Completed result. Terminal runs show Continue the conversation instead. Uncertain roots or cleanup retain execution blockers and disable settlement, retry and continuation.

### Setup

Setup occupies the companion position. Its section controls retain unsaved values when the user changes sections.

The visible sections are:

- Model.
- Files and execution.
- Instructions.
- Presets.

New forms select the available tools by default. An explicit empty tool choice stays empty after validation and on copied records.

Saved conversations submit through Save settings. New conversations offer Keep draft settings. Cancel setup changes retains its existing behaviour.

A successful settings save shows a status message outside the scroll area. The message distinguishes local settings from future defaults.

Use saved settings as future defaults is a separate explicit action. Its explanation distinguishes saved values from unsaved edits and access approval.

The execution-switch preview retains its existing settlement paths. Requested values stay draft until the save or approval command succeeds.

Setup and workflow companions follow the actual header height. Long mobile titles remain visible above the open companion.

Section changes retain unsaved fields. Validation reveals affected controls before focus moves. Effective summaries change only after a successful settings command.

Closure restores focus even after a command replaces the original trigger. Escape closes the navigation menu before the companion.

### Handoff

The conversation header offers Handoff as a text link. Its canonical page keeps the selected theme and shared navigation.

A focused page presents an optional instruction field before generation. The generated prompt appears in a labelled textarea with an explicit continuation action.

Prepare new conversation opens an unsent draft. It does not start the next agent.

At a safe decision, the page offers exact prepared changes or context only. Neither choice applies, discards or reverses files.

An exact-change draft displays pinned settings and explicit run-only approval. Send transfers ownership without another model call or gate decision.

An expired runtime displays Restore prepared changes in the owner companion. The candidate remains visible, but decision controls stay disabled until fresh consent restores execution.

The handoff page uses the existing type scale and controls. It introduces no new palette or panel system.

### Workflow setup

Workflow setup occupies the 400-pixel companion. Choose, inputs and review retain the brief through Back controls.

The chooser lists the five bundled sequences before user-authored workflows. There is no saved-plan selection or repeated task group.

The content scrolls separately from the footer. The footer retains Back and the eligible next action.

Review distinguishes run defaults from effective phase settings. Exact host policy and unrestricted access remain beside run-only consent.

On mobile, the open workflow companion excludes the hidden conversation controls. Closure restores focus to Run a workflow.

Forms above 256 KiB retain a standalone representation at the same canonical URL. The companion supplies no execution authority.

### History and resources

Resource catalogues and their forms use the shared catalogue layout. Their 18-pixel titles sit in a full-width header, with explanatory text beneath them. The content uses the same background and inset as Settings. Thin rules separate records, and the header stays above the content scroll area.

Decision entries link to conversations and exact gate pages. The decision list contains no approval form.

The conversation catalogue pairs its directory filter with a title search. The query trims and matches titles without case sensitivity. Both filters stay in the canonical address and native form navigation. Access grants stay unchanged.

Run history filters by stored canonical directory identity. The filter applies before the fifty-record bound with newest matches first. Unavailable directories keep their labels. Run history stays run-centred as an approved difference from the reference conversation rows.

Run details retain the owning conversation. Evidence pages retain their canonical run links.

### Other surfaces

Catalogue pages retain ruled records and inline editors. Workflow pages retain process previews, phase selectors and explicit run consent.

First-time provider connection starts with a focused provider chooser in a compact branded form.

The provider choice precedes the connection form. Method descriptions align right on wider screens and sit below provider names on narrow screens.

Provider links use `/connect?provider=…` with native navigation fallback. Change provider returns to the chooser. API-only forms omit the redundant introductory sentence.

Pending plan sign-in and validation errors remain within the form. Successful plan polling navigates to conversations.

In-app Providers uses the shared catalogue header and content background. The storage note sits beneath the page title. Connection controls sit directly on the content background, with connected providers beneath them.

The standalone page uses the same background and retains its compact bordered form. Both versions share the provider chooser and connection controls.

## Validation boundary

The implementation status and evidence boundary are in `docs/conversation-system.md`.

Current browser checks cover desktop and mobile navigation with synthetic conversation data. Springfield and Sector 7-G retain their theme tokens.

The mobile handoff page passes the browser accessibility audit. This result does not establish accessibility for every execution state or theme.

Browser checks cover ownership transfer, restart and explicit application to a real temporary file without a commit.

A supplied prompt enters the real preparation endpoint. Synthetic records support direct-write previews and downloads.

Browser evidence does not establish successful hosted-model generation or hosted agent execution.

## Do's and Don'ts

- Use DaisyUI primitives for controls.
- Use Tailwind utilities in Askama templates for layout.
- Keep feature presentation within its slice.
- Keep native links for ordinary navigation.
- Keep exact candidate and revision fields on consequential commands.
- Keep effective access separate from unsaved settings.
- Keep plans and checklists as ordinary content.
- Do not use title case for headings or controls.
- Keep focus indicators visible.
- Respect reduced-motion preferences.
