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

The approved reference is `docs/ui-overhaul/reference/`. The conversation, plans and workflow setup share its companion structure. Catalogue authoring remains a separate flow.

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

Needs your attention carries the positive server decision count. The live projection refreshes the count with the recent list. Recent records show state dots. Untouched saved records read Draft. Responsive idle records read Ready. Review, completion and cancellation transitions stay live. The footer keeps its local state dot.

Conversation pages carry their own header with the mobile menu trigger. The separate location bar stays for catalogue pages that need navigation and execution status.

Needs your attention lists real unresolved decisions with owning context links. An optional conversation identifier selects the return destination only: valid context shows Back to conversation, while any other value shows Back to conversations. Refresh and decision pages preserve valid context, and every decision stays visible. History connects conversations to runs, task loops and evidence.

Workflows and presets opens the shared resource page. Ruled sections distinguish reusable processes from complete settings snapshots, with Use workflow on processes and Preview setup on settings. An optional conversation identifier selects the destination only. Valid context shows Back to conversation and direct handoffs to that record, while any other value opens a destination chooser on the same page. Stale workflow or preset identities report an error without substitution. The breadcrumb group reads Library.

The resource page retains destinations for providers, environments, projects and saved agents. No catalogue capability disappears.

### Transcript and composer

User messages use a tinted, ruled surface. Assistant messages identify Power Plant with its mark.

The model control sits inside the composer with a dropdown chevron. The composer uses two rows with an 8000 character editor limit. The persisted message bound stays in the conversation store. The send control reads Send message. Effective directory access appears below it with a folder or shield icon and a visible Sandbox label. Project access stays beside the composer even with no attached project. An empty project catalogue links to project registration. Host mode names unrestricted access. Job-bound cancellation reads Stop task beside the composer and in Current work. The composer stays locked while a candidate or host command awaits a decision.

Jump to latest appears when the reader leaves the transcript end. New output does not move the reader away from earlier messages.

### Conversation header

The header shows the directory name followed by Conversation, or Nothing is saved until your first message on a draft. Saved records without directory context read No directory / Conversation. Actions run Plans, Setup, then Conversation actions. Conversation actions holds the independent draft copy with its settings note, rename and deletion behind an explicit confirmation. Plans carries its count only when plans or task lists exist. Current work appears only while work is non-idle and its companion is closed. A Needs your review strip opens Current work without approval when a decision waits. Narrow screens wrap the actions below a long title because production keeps readable text labels where the mock uses compact icon buttons.

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

Setup occupies the companion position. One native section selector reveals a group without removal of the other controls. The former hidden tabs are retired. The six sections, Model, Instructions and tools, Execution, Directories, Presets and Conversation details, are the approved production difference from the reference tab buttons. Genuinely new conversation, agent and preset forms select List, Read, Write and Run. An explicit empty tool choice stays empty after validation and on existing or copied records.

Setup submits through Review setup changes with Cancel setup changes. Requested values stay draft until the save or approval command succeeds. The execution-switch preview and its Stop task and switch, Discard changes and switch and Change execution settings settlement paths are unchanged.

Setup and Plans follow the actual header height. Long mobile titles remain visible above the open companion.

Section changes retain unsaved fields. Validation reveals affected controls before focus moves. Effective summaries change only after a successful settings command.

Closure restores focus even after a command replaces the original trigger. Escape closes the navigation menu before the companion.

### Plans

Plans opens a companion with native navigation fallback. A selected plan retains the conversation workspace and a canonical revision URL.

Explicit actions show their original contents in ruled transcript sections. Action details identify the author and immutable revision, rather than an inferred attachment.

Bounded delivery keeps the transcript and plan views within the Hypergraft response and node limits. Oversized transcript actions defer their complete contents to the pinned revision link. Oversized plan revisions use bounded continuation sections on the same canonical route with the complete pinned export unchanged. Sections retain complete Markdown blocks and share reference definitions. A block above the section limit uses escaped text across sections instead of partial Markdown. This bounded delivery is the production difference from the small mock examples.

The plan companion separates its content scroll area from its action footer. Prepare implementation opens a workflow preview and starts no work.

Revision controls accept a model request or supplied text. Earlier action titles and contents remain unchanged after either action.

Task breakdowns stay under their source plan. Older breakdowns identify their source revision and show an outdated notice for new runs.

Standalone task imports remain distinct from plans. Export and independent review retain their canonical routes. A saved standalone list records an Added tasks transcript action. The task detail keeps one h1 title and demotes the saved Markdown heading for display without changing the stored revision or pinned export.

On mobile, both Plans and a selected plan exclude the hidden conversation controls. Closure returns focus to a conversation control.

Ordinary replies never become plans through text recognition. A plan action grants no execution authority.

### Workflow setup

Workflow setup occupies the 400-pixel companion. Choose, inputs and review retain the brief and exact input selection through Back controls. The chooser lists Implement a saved plan first, then the six reference processes in reference order, then user-authored workflows. Implement a saved plan is the production difference from the six-process reference mock.

The content scrolls separately from the footer. The footer retains Back and the eligible next action.

Review distinguishes run defaults from effective phase settings. Exact host policy and unrestricted access remain beside run-only consent.

On mobile, the open workflow companion excludes the hidden conversation controls. Closure restores focus to Run a workflow.

Forms above 256 KiB retain a standalone representation at the same canonical URL. The companion supplies no execution authority.

### History and resources

History and resources use a 24-pixel title and 14-pixel body text. Thin rules and pale secondary controls match the workspace material.

Decision entries link to conversations and exact gate pages. The decision list contains no approval form.

The conversation catalogue pairs its directory filter with a title search. The query trims and matches titles without case sensitivity. Both filters stay in the canonical address and native form navigation. Access grants stay unchanged.

Run history filters by stored canonical directory identity. The filter applies before the fifty-record bound with newest matches first. Unavailable directories keep their labels. Run history stays run-centred as an approved difference from the reference conversation rows.

Run details retain the owning conversation and parent loop links. Evidence pages retain their canonical run links.

### Other surfaces

Catalogue pages retain ruled records and inline editors. Workflow pages retain process previews, phase selectors and explicit run consent.

First-time provider connection starts with a focused provider chooser in a compact branded form.

The provider choice precedes the connection form. Method descriptions align right on wider screens and sit below provider names on narrow screens.

Provider links use `/connect?provider=…` with native navigation fallback. Change provider returns to the chooser. API-only forms omit the redundant introductory sentence.

Pending plan sign-in and validation errors remain within the form. Successful plan polling navigates to conversations. Later provider management retains the shared shell.

## Validation boundary

[The final UI report](docs/ui-overhaul/MILESTONE-4.md) records capability destinations and state coverage. The approved reference remains unchanged.

Candidate review passed desktop and mobile accessibility audits in all five themes. Selected gradient controls retain incomplete contrast checks.

Browser evidence does not establish successful hosted execution or reviewed file application. Partial application and uncertain recovery retain automated coverage.

## Do's and Don'ts

- Use DaisyUI primitives for controls.
- Use Tailwind utilities in Askama templates for layout.
- Keep feature presentation within its slice.
- Keep native links for ordinary navigation.
- Keep exact candidate and revision fields on consequential commands.
- Keep effective access separate from unsaved settings.
- Do not infer plans from reply text.
- Do not use title case for headings or controls.
- Keep focus indicators visible.
- Respect reduced-motion preferences.
