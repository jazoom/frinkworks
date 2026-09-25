---
name: Frinkworks
description: A local conversation workspace with an optional work companion.
colors:
    canvas: "#f5f5ed"
    paper: "#fefcf6"
    paperGreen: "#eeeee5"
    cover: "#28321f"
    coverText: "#f5f6ef"
    coverMuted: "#c4c8bc"
    ink: "#171a16"
    brandInk: "#565c4e"
    brandReverse: "#f5f2eb"
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
    transcript:
        fontSize: "16px"
        lineHeight: 1.7
    toolRecord:
        fontSize: "15px"
    replyStatus:
        fontSize: "13px"
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

# Design system: Frinkworks

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

IBM Plex Sans carries the interface. IBM Plex Mono identifies paths and code. The wordmark uses vector outlines from `logo.png`, with the source letter shapes and spacing. The application loads no additional font for the wordmark.

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

The composer and message records reach a maximum width of 1110 pixels. Message prose retains its 70-character measure.

Composer controls share one desktop row and wrap on narrower screens. Mobile message bodies use the full transcript width below their author labels.

The transcript toolbar scrolls horizontally when its controls exceed the available width. The mobile activity strip occupies a separate row beneath those controls.

At heights up to 650 pixels, mobile composer controls scroll horizontally. Queue and Stop retain a separate row below them.

Short mobile screens use a 20-pixel conversation title and the complete header. The empty queue explanation disappears, but queued records and errors remain.

Mobile composer content uses a bounded scroll area. Queued records have their own bounded scroll area, so attachments and queues cannot displace every control.

Short mobile screens omit the empty-state description and reduce its heading. The composer remains available without a page scroll.

Configured drafts show a directory summary above the composer. Short mobile screens retain its heading and the access strip, with full details in Setup.

Below 701 pixels, the companion fills the conversation area. The hidden transcript, composer and conversation toolbar become inert.

Plan review remains in the conversation companion. A separate native link opens the canonical plan decision page.

## Elevation & Depth

Surface tones and thin rules establish boundaries. A thin border defines the composer without a shadow. Setup has no modal backdrop or centred dialog shadow.

Protected consent and destructive actions retain their explicit forms and confirmations. A companion transition grants no authority.

Host consent uses a native modal above Setup with a dimmed backdrop. Its content scrolls within the viewport. Cancel grants no authority.

## Shapes

Controls retain DaisyUI primitives. Workspace controls use seven-pixel corners. Recent records use five-pixel corners. User messages retain three-pixel corners. The composer uses nine-pixel corners.

The Frinkworks symbol uses a lime accent on a transparent canvas. `app/public/images/logo.svg` uses olive-grey ink for light backgrounds. `app/public/images/logo-dark.svg` uses off-white for dark backgrounds. The sidebar and mobile header select the symbol for their cover colour. Other symbols follow the page theme. The favicon follows the system colour preference.

Both symbol variants use identical geometry. Every straight section, including the lime bar, has a perpendicular width of 48 units. The vertical gap is 24 units on the 288-unit canvas.

`app/public/images/wordmark.svg` supplies the outlined wordmark. It matches the symbol’s main stroke colour and retains the accessible name Frinkworks. Its width is 170 pixels in the sidebar and 162 pixels on the connection page. The mobile header uses 148 pixels, or 136 pixels below a viewport width of 361 pixels. The symbol dimensions remain unchanged.

Workspace icons use the shared stroke geometry in `app/public/images/workspace-icons.svg`.

## Components

### Transport feedback

A delayed accent bar identifies page requests, including streamed GET responses. It describes transport, not execution progress.

A failed navigation keeps the current page and offers Retry navigation or Dismiss. An uncertain command result takes precedence and requires a reload.

Connection feedback reports disconnected or stopped WebSocket updates. The conversation list shows receipt time only after its projection updates both the list and attention count.

That timestamp does not describe reply output or execution completeness. Reply output uses separate streamed requests.

Back and Forward restore the catalogue content position after a fresh response. The transcript retains its own auto-scroll behaviour. Intent prefetch stays disabled.

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

Project skills live in `.agents/skills` directly inside each authorised directory. Discovery does not search nested project directories. Frinkworks advertises the skill name and description. The model reads the body with the read tool.

Stale workflow or preset identities report an error without substitution. Resource navigation starts no work and grants no access. The breadcrumb group reads Resources.

### Transcript and composer

Header and composer controls use a neutral hover tint. Outlined controls also darken their border on hover. Primary actions retain the theme accent.

Directory actions retain transparent backgrounds on hover. Button text has no hover underline. The separator stays outside the button and its keyboard focus outline.

The paperclip uses a continuous diagonal stroke. The help icon uses a centred question mark and a separate dot. Both retain labelled controls.

User messages use a tinted, ruled surface. Assistant messages identify Frinkworks with its mark.

Avatars sit beside desktop author labels and message bodies. Mobile avatars sit beside the labels, above full-width content.

Message text uses the transcript scale. Status stays beside the author. Revise, Fork from here and Copy appear below the applicable message.

Tool results use bordered disclosures with the actual tool label. Unfinished tools show their recorded name without an inferred path or result.

The mobile activity strip repeats the server's active reply status. It opens Current work without a command. Historical windows and settled replies omit the strip.

The model control opens a searchable popover above the composer. It lists connected providers with an optional provider filter. Favourites appear first, with a separate star control on each row. The active Favourites filter uses a soft tint and a check mark. Local application data stores favourites across browser sessions.

Thinking effort stays visible beside the model as an outlined control with its label, value and caret. Both controls share the same height. The effort popover uses padded options and a tick for the current value. Models without adjustable effort show a disabled Not available control. Saved conversations apply model and effort changes immediately without changes to other settings. Unsent messages and unsaved setup fields survive those commands.

The composer places its editor above a compact toolbar. The editor limit is 32768 characters. The persisted message bound stays in the conversation store.

The paperclip opens image selection. Selection uploads automatically. The slash control opens skills and prompts. Composer help retains file references and prefix explanations.

Send uses an accessible Send message label. An empty composer disables Send unless it contains an attachment or prepared-change handoff.

Effective directory access appears below the composer. Add a directory stays beside it. The execution label stays visible even without directory access. Host mode names unrestricted access.

Job-bound cancellation reads Stop beside the composer submit control. It posts without a confirmation step. Current work keeps the same Stop control when the composer is inert.

Active replies use the follow-up placeholder and Queue label. A labelled selector retains After this work and Correct current work when both choices apply.

The shortcut hint distinguishes Queue from Send. The submit label follows the latest server patch after settlement.

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

### Conversation tree

Tree entries and branch tips use full-width vertical records. Entry metadata sits above the excerpt, with navigation links below it.

Long unbroken excerpts wrap within the companion. Tree records do not use the shared two-column catalogue layout.

### Current work

Current work puts execution status and required decisions before Context and usage. That disclosure retains estimates, compaction details and recorded summary usage.

Ordinary replies repeat the server status. Retry details follow the same observation updates and disappear when the retry ends. No generated activity description appears.

Workflow progress leads with the pinned workflow name, recorded state and current phase. It shows no inferred percentage or completion count.

The companion header uses 16-pixel text and a 44-pixel Close control. Eligible Stop controls remain outside the content scroll area, including during questions.

Needs your answer and Execution paused open the existing decision controls. Native attention links and Back to current work open the canonical conversation route.

Compact desktop composers use horizontal scroll for controls when their container narrows. Queue and Stop remain below those controls without overlap with the companion.

A mobile resize moves focus from excluded conversation controls into the open companion. Escape closes the companion and restores the conversation trigger.

Plan review shows the exact plan and its decision controls. Acceptance starts only the declared continuation. It grants no file authority.

Revision feedback remains bound to the plan. Plan decisions from the companion return conversation patches and retain unsent text.

The companion retains its 400-pixel width. Markdown code blocks receive keyboard focus. Long plan content has a bounded scroll area.

The idle companion reads Ready when you are with View activity and evidence and Continue the conversation.

Command approval reads Run this command for both execution locations. It shows the exact command, work location and effective policy.

Evidence links show requests and bounded output. They start no command and make no current-file claim.

Current work retains the eligible job-bound Stop control. Cancellation leaves earlier file changes intact. Incomplete cleanup retains execution blockers.

The interface contains no file candidate, application decision, before/after preview, rollback or automatic commit control.

The attention strip stays within the toolbar and opens Current work without a command. Mobile rejection closes the companion to expose its authoritative error.

The error occupies a separate grid row instead of the toolbar area. Its focus remains visible, and the unsent draft remains intact.

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

Directory commands retain the unsent message. Add and remove retain transparent hover backgrounds without text underlines.

Draft summary items open their relevant Setup section. Directory names, paths and access labels share one button. Execution and network summaries open Execution.

Add a directory below the composer opens the native directory picker directly. The response opens Directories with the applicable access controls and approval steps.

Directories shows the applied execution context and directory access controls. It contains no repository selector. Setup toggles the companion open and closed.

Directory controls expose exactly Read and Write. Read is the default. Write changes original files immediately.

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

Execution uses labelled radio choices for the location and command policy. Ask each time and Automatic (YOLO) remain visible for both locations.

Preset forms expose the same command choices. Automatic (YOLO) never increases directory permissions. Sandbox network choices expose the domain field only for Restricted domains.

The environment selector shows recorded preparation and snapshot status. It makes no readiness claim without the corresponding records.

Saved network changes join the execution review. Requested execution values stay uncommitted until the review command succeeds. Drafts retain choices without a separate save confirmation.

Review execution change replaces the Setup sections within the companion. A Current/Requested table marks changed rows with a tint and the word Changed.

Each directory retains its own access row. Sandbox settings remain explicitly labelled in host mode. Keep current settings resets requested execution values without a command.

Active work retains Stop task and switch. A pending plan decision retains its plan identity during an environment change. Neither action reverses existing effects.

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

At a safe decision, the page offers workflow ownership transfer or context only. Neither choice changes files.

A transfer draft displays pinned settings and explicit run-only approval. Send transfers ownership without another model call or gate decision.

An expired runtime retains the plan in the owner companion. Decision controls stay disabled until fresh consent restores execution.

The handoff page uses the existing type scale and controls. It introduces no new palette or panel system.

### Workflow catalogue

Workflow names lead ruled records with visible ordered sequences. Phase names pair with their recorded kinds. Decision routes remain visible below the sequence.

The catalogue uses 20-pixel record headings and 16-pixel body text. Its controls retain a 44-pixel minimum height and seven-pixel corners.

Open workflow setup carries validated conversation context. Without context, Choose conversation opens the existing chooser. Edit remains separate from the primary setup action.

Phase purposes and details use a disclosure. These details describe the saved definition rather than effective access or completed work.

Desktop phases wrap within the record. Mobile phases form one column. Conversation context stacks above its return link on narrow screens.

Long names wrap without loss of actions. The chooser and selection errors receive focus after navigation, with their content scroll area at the top.

The catalogue adds no execution control. Launch review retains the effective settings and required consent.

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

Run history filters by stored canonical directory identity. The filter applies before the fifty-record bound with newest matches first. Unavailable directories keep their labels. Run history stays run-centred.

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

Rust tests cover ownership transfer, restart and plan decision integrity. Real Microsandbox tests cover live Read and Write mounts.

The Read/Write browser pass uses isolated local data without a connected provider.

Browser evidence does not establish successful hosted-model generation or hosted agent execution.

## Do's and Don'ts

- Use DaisyUI primitives for controls.
- Use Tailwind utilities in Askama templates for layout.
- Keep feature presentation within its slice.
- Keep native links for ordinary navigation.
- Do not add noscript fallbacks.
- Keep exact plan and revision fields on consequential commands.
- Keep effective access separate from unsaved settings.
- Keep plans and checklists as ordinary content.
- Do not use title case for headings or controls.
- Keep focus indicators visible.
- Respect reduced-motion preferences.
