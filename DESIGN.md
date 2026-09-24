---
name: Power Plant
description: A local conversation workspace with an optional work companion.
colors:
    canvas: "#f5f5ed"
    paper: "#fefcf6"
    paperGreen: "#eeeee5"
    cover: "#28321f"
    coverText: "#f5f6ef"
    coverMuted: "#c4c8bc"
    ink: "#171a16"
    quietInk: "#65695f"
    rule: "#d9dad2"
    action: "#b3f04e"
    actionInk: "#18210c"
    error: "#a63b32"
typography:
    title:
        fontFamily: "IBM Plex Sans, ui-sans-serif, system-ui, sans-serif"
        fontSize: "18px"
        fontWeight: 600
    conversationTitle:
        fontSize: "28px"
        fontWeight: 600
    conversationWelcome:
        fontSize: "34px"
        fontWeight: 600
    navigation:
        fontSize: "16px"
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
    control: "3px"
    workspaceControl: "7px"
    record: "3px"
    composer: "9px"
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

Springfield uses pale paper and lime actions. Thin rules separate the transcript, companion and controls.

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

Conversation text has a maximum measure of 70 characters. Result headings identify the next decision.

Conversation titles use 28-pixel text. The empty-state heading uses 34-pixel text with a 20-pixel description. Navigation uses 16-pixel text.

Mobile conversation titles use 24-pixel text. The empty-state heading uses 26-pixel text with a 16-pixel description.

Catalogue forms retain their existing type scale. The workspace's smaller metadata is not a new standard for all form text.

First-use chooser headings use 30px, and connection headings use 28px. Narrow screens use 28px and 26px respectively. Introductory text uses 15px.

## Layout

The desktop index occupies 300 pixels. Current work occupies 400 pixels. Setup uses 400 to 488 pixels according to viewport width.

The conversation fills the remaining width.

Setup and Current work open with a 320 ms slide and close in 240 ms. Reduced motion removes the slide.

The transcript and companion content scroll independently. The composer stays outside the transcript scroll area.

The index leaves the page below 1021 pixels. A branded mobile bar supplies Menu on conversation pages. Menu provides the same navigation destinations.

The composer reaches a maximum width of 1110 pixels. Its controls share one desktop row and wrap on narrower screens.

Short mobile screens omit the empty-state description and reduce its heading. The composer remains available without a page scroll.

Configured drafts show a directory summary above the composer. Short mobile screens retain its heading and the access strip, with full details in Setup.

Below 701 pixels, the companion fills the conversation area. The hidden transcript, composer and conversation toolbar become inert.

Expanded review occupies the conversation width without a new conversation or URL. A separate native link opens the canonical candidate review page.

## Elevation & Depth

Surface tones and thin rules establish boundaries. A thin border defines the composer without a shadow. Setup has no modal backdrop or centred dialog shadow.

Protected consent and destructive actions retain their explicit forms and confirmations. A companion transition grants no authority.

Host consent uses a native modal above Setup with a dimmed backdrop. Its content scrolls within the viewport. Cancel grants no authority.

## Shapes

Controls retain DaisyUI primitives. Workspace controls use seven-pixel corners. Recent records use five-pixel corners. User messages retain three-pixel corners. The composer uses nine-pixel corners.

The existing Power Plant mark remains unchanged. Workspace icons use the approved reference's stroke geometry.

## Components

### Navigation

New conversation remains prominent. The index contains up to twelve server-derived recent conversations with real titles and status.

The search field has a visible boundary. Recent rows pair a conversation icon with a title and status dot. Resource links have a Resources heading.

The sidebar search filters those recent titles live as plain text. The catalogue link beside it stays the native fallback. The filter survives live replacement of recent records.

Needs your attention carries the positive server decision count. The live projection refreshes the count with the recent list. Recent records show state dots. Untouched saved records read Draft. Responsive idle records read Ready. Review, completion and cancellation transitions stay live.

Conversation pages carry their own header with the mobile menu trigger. The separate location bar stays for catalogue pages that need navigation and execution status.

Needs your attention lists real unresolved decisions with owning context links. An optional conversation identifier selects the return destination only: valid context shows Back to conversation, while any other value omits the return link. Refresh and decision pages preserve valid context, and every decision stays visible. History connects conversations to runs and evidence.

The sidebar resource group links directly to Workflows, Presets, Environments, Agents and Skills. Providers and Settings stay separate below the group.

The sidebar has no local status footer. Catalogue headers omit generic return links to conversations.

Workflows and Presets each combine use and management on their canonical page. An optional conversation identifier selects the destination only.

Valid context retains Back to conversation. Without valid context, the selected resource offers a conversation chooser on its own page.

Skills combines the global skill catalogue and a plain Markdown editor on one page. The editor contains the complete `SKILL.md` file, including standard YAML frontmatter. The page shows the actual global directory and explains direct file placement. Copied files appear on refresh and become available to the next request without a restart.

Project skills live in `.agents/skills` directly inside each authorised directory. Discovery does not search nested project directories. Power Plant advertises the skill name and description. The model reads the body with the read tool.

Stale workflow or preset identities report an error without substitution. Resource navigation starts no work and grants no access. The breadcrumb group reads Resources.

### Transcript and composer

Header and composer controls use a neutral hover tint. Outlined controls also darken their border on hover. Primary actions retain the theme accent.

Directory actions retain transparent backgrounds on hover. Button text has no hover underline. The separator stays outside the button and its keyboard focus outline.

The paperclip uses a continuous diagonal stroke. The help icon uses a centred question mark and a separate dot. Both retain labelled controls.

User messages use a tinted, ruled surface. Assistant messages identify Power Plant with its mark.

The model control opens a searchable popover above the composer. It lists connected providers with an optional provider filter. Favourites appear first, with a separate star control on each row. The active Favourites filter uses a soft tint and a check mark. Local application data stores favourites across browser sessions.

Thinking effort stays visible beside the model as an outlined control with its label, value and caret. Both controls share the same height. The effort popover uses padded options and a tick for the current value. Models without adjustable effort show a disabled Not available control. Saved conversations apply model and effort changes immediately without changes to other settings. Unsent messages and unsaved setup fields survive those commands.

The composer places its editor above a compact toolbar. The editor limit is 32768 characters. The persisted message bound stays in the conversation store.

The paperclip opens image selection. Selection uploads automatically. The slash control opens skills and prompts. Composer help retains file references and prefix explanations.

Send uses an accessible Send message label. An empty composer disables Send unless it contains an attachment or prepared-change handoff.

Effective directory access appears below the composer. Add a directory stays beside it. The execution label stays visible even without directory access. Host mode names unrestricted access.

Job-bound cancellation reads Stop beside the composer submit control. It posts without a confirmation step. Current work keeps the same Stop control when the composer is inert.

Pending decisions retain their existing queue and consent rules.

Jump to latest appears when the reader leaves the transcript end. New output does not move the reader away from earlier messages.

### Conversation header

New conversations show the title without an explanatory subtitle. Saved records identify their directory context beside the conversation title.

The new-conversation empty state asks what the user wants to work on. It contains no starter buttons or decorative mark.

New drafts omit the transcript toolbar. Saved conversations retain its controls.

The header offers Handoff, Setup and a labelled menu for Conversation actions. The menu uses a vertical ellipsis.

Conversation actions contains an independent draft copy, rename and explicit deletion.

Current work appears when work is non-idle and its companion is closed. Initial page loads and reloads keep the companion closed. A message does not open the companion. A Needs your review strip opens the companion without approval.

Narrow screens wrap the actions below a long title. All actions retain readable labels.

### Current work

Current work contains execution progress and required decisions. Candidate review shows real changed files and bounded diff previews. Diffs and Markdown code blocks receive keyboard focus. The idle companion reads Ready when you are with View activity and evidence and Continue the conversation. Local review expansion stays within the conversation URL and one native link opens the canonical gate page.

Per-file addition and removal counts derive from the complete stored diff. Binary or oversized changes omit counts rather than infer them from truncated previews. The companion lists the total changed-file count and notes when only the first paths render. Recorded test outcomes are not part of the candidate evidence, so neither review surface shows a test result line. These omissions are deliberate: counts and test lines appear only when recorded data supports them.

The candidate footer has a bounded scroll area for long destinations and feedback forms. Current work retains the eligible job-bound Stop control without a confirmation step.

The approval footer names the destination and the actual application consequence. It distinguishes ordinary file application from a local Git commit. It distinguishes configured continuation to the next step from both file outcomes.

Host approval reads Run this command. It shows the exact command, work location and effective approval policy beside the decision. Session-bound approval and rejection evidence remains in the run record.

Request changes retains candidate-bound feedback. Discard posts directly without a confirmation dialog. Discard keeps evidence and history, so it is not destructive in the data sense. Discard does not reverse direct writes or host command effects.

Current work shows per-directory file-application outcomes from the authoritative run transaction record. Known partial application stays distinct from uncertain recovery and successful completion. Resolve the conflict links to the exact run attempt and its directory evidence. The link opens evidence and starts no write, retry or simulated resolution. This manual recovery destination is the production difference from the mock simulated conflict button.

Keep applied files and end task appears only when every transaction holds a known settled outcome and managed cleanup succeeded. Settlement binds the displayed run, attempt and outcome state. It keeps applied files and evidence without another application attempt, and ends ownership through cancellation rather than a Completed result. Terminal runs show Continue the conversation instead. Uncertain roots or cleanup retain execution blockers and disable settlement, retry and continuation.

### Setup

Setup occupies the companion position. Its section controls retain unsaved values when the user changes sections.

The visible sections are:

- Directories, or Work locations in host mode.
- Execution.
- Instructions.
- Presets.

The Presets section offers a preview for each preset and a separate Save this setup as a preset action.

Replacement hides the list and shows current and preset values. Each setting labels two value columns within the existing companion width.

A tint and Changed identify different values. Long values wrap. Instruction text retains a bounded scroll region with keyboard focus.

Cancel returns to the list without a command. Apply replacement remains explicit. Its explanation separates settings from directory approval and runtime consent.

The save panel contains a name field and a settings summary. A separate disclosure contains the instructions. Back to presets retains the name.

Saved snapshots show stored settings only, even when execution changes await review. Draft snapshots show current choices. Both retain the unsent message.

The name accepts up to 80 UTF-8 bytes without control characters. An empty name uses the existing suggestion. Validation retains the entered name.

Preset controls retain the larger workspace scale and theme accent. Narrow screens stack the decision buttons. Every action remains accessible through the panel scroll area.

Directories opens with a Directory access heading and Add a directory. Empty conversations explain the absence of directory access.

Each directory has a bordered record with labelled paths and radio controls. The record separates requested access from current access and retains explicit approval actions.

The command start-directory selector moves the chosen directory first. It retains each access mode and requires fresh consent where applicable.

Unavailable directories remain visible but cannot become a new command location. Active work disables the selector.

Saved access changes retain Review changes and the existing execution review. A radio selection alone grants no authority.

Configured drafts show the actual directory paths and access states. The summary names the execution location and network setting without a readiness claim.

Directory commands retain the unsent message. Add, remove and Change setup retain transparent hover backgrounds without text underlines.

Add a directory below the composer opens the native directory picker directly. The response opens Directories with the applicable access controls and approval steps.

Directories shows the applied execution context and directory access controls. It contains no repository selector. Setup toggles the companion open and closed.

Explicit workflow commit steps detect each changed repository. Approval names the commit consequence. The run details show each repository's result separately.

New forms select the available tools by default. An explicit empty tool choice stays empty after validation and on copied records.

Instructions uses a labelled editor above single-column tool rows. Each row pairs a checkbox with a short description. Select all shows a mixed state for partial choices.

The editor retains the workspace control scale and corners. Selected checkboxes use the theme accent. Labels provide larger pointer targets and retain keyboard focus outlines.

The editor distinguishes unsaved drafts from saved conversation settings. Save feedback reflects the command result. Rejected saves retain later edits without an automatic retry.

Validation counts UTF-8 bytes and rejects unsupported control characters. The error appears beside the editor. A blocked message submission reveals Instructions before focus moves.

Tool explanations distinguish authorised sandbox directories from unrestricted host access. The empty selection explains that a model reply needs no sandbox.

Instruction sources shows recorded file paths and a native request-context link. It names the active branch and makes no current-access claim.

The list shows at most eight paths. The context page retains the complete list. Empty and unavailable records have distinct explanations.

Host and sandbox modes expose the same tool selections. Host consent covers file tools as well as Run. Host file changes take effect immediately. Command approval applies only to Run.

Ordinary setup controls apply when they change. There is no Save, Done, Keep draft or Cancel action. Close hides Setup. The application requires JavaScript.

Save as future defaults is a separate explicit action in the companion footer. Its explanation distinguishes stored conversation settings from access approval.

Execution uses labelled radio choices for the location and command policy. Sandbox network choices expose the domain field only for Restricted domains.

The environment selector shows recorded preparation and snapshot status. It makes no readiness claim without the corresponding records.

Saved network changes join the execution review. Requested execution values stay uncommitted until the review command succeeds. Drafts retain choices without a separate save confirmation.

Review execution change replaces the Setup sections within the companion. A Current/Requested table marks changed rows with a tint and the word Changed.

Each directory retains its own access row. Sandbox settings remain explicitly labelled in host mode. Keep current settings resets requested execution values without a command.

Active work retains Stop task and switch. Prepared changes retain their exact-candidate decision and Discard changes and switch. Neither action reverses existing host effects.

Host consent displays the process identity and privileges beside the actual start directory. It explains unrestricted access and immediate file changes.

The consent modal shows a fixed command policy. Change policy returns to Setup. Requested saved changes require review before host consent becomes available.

Cancel and Escape close consent without a settings change. Approval binds the displayed configuration to the session. Execution commands retain the unsent message.

Execution controls use the larger workspace scale. Narrow screens retain scroll access to every decision. Button text has no hover underline.

Setup and workflow companions follow the actual header height. The header retains its full content height on short screens. Wrapped mobile actions remain above the companion.

Section changes retain uncommitted execution fields. Validation reveals affected controls before focus moves. Effective summaries follow applied ordinary settings. Model and effort controls remain in the composer, outside Setup.

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

Provider links use `/connect?provider=…`. Change provider returns to the chooser. API-only forms omit the redundant introductory sentence.

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
- Do not add noscript fallbacks.
- Keep exact candidate and revision fields on consequential commands.
- Keep effective access separate from unsaved settings.
- Keep plans and checklists as ordinary content.
- Do not use title case for headings or controls.
- Keep focus indicators visible.
- Respect reduced-motion preferences.
