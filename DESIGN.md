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
        fontSize: "24px"
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

Provider introductions use the same cover text tokens as the index.

Error panels use a pale error surface and readable recovery links. Action colours change together during theme changes.

## Typography

IBM Plex Sans carries the interface. IBM Plex Mono identifies paths and code. The wordmark uses Bree Serif Regular outlines with optical spacing. The application loads no additional font for the wordmark.

The wordmark retains the letter shapes from [Bree Serif](https://github.com/google/fonts/tree/main/ofl/breeserif), under the SIL Open Font License 1.1. Optical spacing adjusts the source kerning:

- `Fr`: −0.010 em.
- `ri`: +0.003 em.
- `kw`: +0.006 em.

Conversation text has a maximum measure of 70 characters. Result headings identify the next decision.

Page titles use 24-pixel text. The conversation empty-state heading uses 34-pixel text with a 20-pixel description. Navigation uses 16-pixel text.

The empty-state heading uses 26-pixel text with a 16-pixel description on narrow screens.

Catalogue forms retain their existing type scale. The workspace's smaller metadata is not a new standard for all form text.

First-use chooser headings use 30px, and connection headings use 28px. Narrow screens use 28px and 26px respectively. Introductory text uses 15px.

## Layout

The desktop index occupies 300 pixels. Current work occupies 400 pixels. Setup uses 400 to 488 pixels according to viewport width.

The conversation fills the remaining width.

Setup and Current work open with a 320 ms slide and close in 240 ms. Reduced motion removes the slide.

The transcript and companion content scroll independently. The composer stays outside the transcript scroll area.

The index leaves the page below 1021 pixels. A branded mobile bar supplies Menu on conversation pages. Menu provides the same navigation destinations.

The composer and message records reach a maximum width of 1110 pixels. They share the same outer gutters. Message prose retains its 70-character measure.

Composer controls share one desktop row and wrap on narrower screens. Mobile message bodies use the full transcript width below their author labels.

The transcript toolbar scrolls horizontally when its controls exceed the available width. The mobile activity strip occupies a separate row beneath those controls.

At heights up to 650 pixels, mobile composer controls scroll horizontally. Queue and Stop retain a separate row below them.

Short mobile screens use a 20-pixel conversation title and the complete header. The empty queue explanation disappears, but queued records and errors remain.

Mobile composer content uses a bounded scroll area. Queued records have their own bounded scroll area, so attachments and queues cannot displace every control.

Short mobile screens omit the empty-state description and reduce its heading. The composer remains available without a page scroll.

New drafts show the directory manager above the composer. Saved conversations open it from the access summary in the existing companion position. Desktop bounds the draft list height. Mobile drafts use the transcript scroll area.

Below 701 pixels, the companion fills the conversation area. The hidden transcript, composer and conversation toolbar become inert.

Plan review remains in the conversation companion. A separate native link opens the canonical plan decision page.

## Elevation & Depth

Surface tones and thin rules establish boundaries. A thin border defines the composer without a shadow. Setup has no modal backdrop or centred dialog shadow.

Protected consent and destructive actions retain their explicit forms and confirmations. A companion transition grants no authority.

Host consent uses a native modal above Setup with a dimmed backdrop. Its content scrolls within the viewport. Cancel grants no authority.

## Shapes

Controls retain DaisyUI primitives. Workspace controls and navigation links use seven-pixel corners. User messages retain three-pixel corners. The composer uses nine-pixel corners.

The Frinkworks symbol depicts Frink inside a pale circular badge. His skin is warm yellow (`#f2cc69`), with dark olive outlines (`#293218`) and a green bow tie (`#b3d153`). The circle, lenses and coat use off-white (`#fff8e7`).

`app/public/images/logo.svg` uses a stronger olive rim for light backgrounds, including conversation avatars. `app/public/images/logo-dark.svg` retains the fine rim for dark backgrounds. The sidebar and mobile header select the variant for their cover colour. Other placements follow the page theme.

Both variants retain the same character artwork and palette. Both use a square `0 0 1200 1200` viewBox and the same outer badge dimensions. The area outside the circle remains transparent.

The favicon crops the approved Frink artwork from the glasses through the mouth and tooth. A pale rounded square separates the olive outline from dark backgrounds. The crop excludes the tall forehead and body.

`app/public/images/favicon.svg` uses the `515 424 390 390` viewBox and retains the original character paths and transforms. The same asset serves light and dark backgrounds.

`app/public/images/wordmark.svg` supplies the Bree Serif wordmark and retains the accessible name Frinkworks. It uses olive-grey on light backgrounds and off-white on dark backgrounds.

The sidebar pairs its 171-pixel wordmark with a 44-pixel logo and a 15-pixel gap. The connection page retains a 162-pixel wordmark. The mobile header uses 148 pixels, or 138 pixels below a viewport width of 361 pixels. Other logo boxes retain their existing dimensions.

The wordmark retains the shared `0 0 970 154` viewBox. A uniform transform centres the approved outlines without a change to their proportions.

Interface icons use Lucide through the generated sprite at `app/public/images/workspace-icons.svg`. The shared `.workspace-icon` class sets a stroke width of 2. Icons inherit the surrounding text colour.

`scripts/build-icon-sprite.mjs` selects SVGs from the pinned `lucide-static` package. Vite runs the generator for development and production asset builds. The browser loads no icon library.

Decorative icons use `aria-hidden="true"`. Icon-only buttons carry an accessible name through `aria-label`. The Frinkworks logo and wordmark remain custom assets.

## Components

### Focus behaviour

Keyboard navigation retains visible focus. Programmatic focus restoration and non-interactive section destinations use `focusQuietly` in `app/assets/main.ts`.

Window and tab changes do not reset that state. The next navigation key restores the normal focus indication.

Text fields and validation errors retain their focus indication. A scroll-only reveal does not move focus.

### Transport feedback

A delayed accent bar identifies page requests, including streamed GET responses. It describes transport, not execution progress.

A failed navigation keeps the current page and offers Retry navigation or Dismiss. An uncertain command result takes precedence and requires a reload.

Connection feedback reports disconnected or stopped WebSocket updates. The attention count updates on store changes without routine status text.

Each connection receives a fresh snapshot. Idle connections require no periodic refresh.

Reply output uses separate streamed requests. Successful reply checkpoints do not refresh the conversation list.

Back and Forward restore the catalogue content position after a fresh response. The transcript retains its own auto-scroll behaviour. Intent prefetch stays disabled.

### Navigation

New conversation uses a quiet outline and subtle cover tint instead of a filled accent. Its full-width target, plus icon and visible label remain at the top of the index.

The 300-pixel index groups navigation under Your work and Resources. Providers and Settings occupy the bottom group without another divider. Short viewports scroll the navigation as one region.

The index uses 20-pixel navigation gutters and 12-pixel gaps between icons and labels. Navigation rows retain a 44-pixel minimum height. Group headings use muted 13-pixel text.

Resource symbols occupy 18 pixels inside aligned 20-pixel icon boxes. A larger gap and a thin rule separate Resources from Your work. The active destination uses a tint and semibold text.

The Conversations page owns history and title search. It lists the most recently updated conversations first, with directory filters and pagination. The sidebar contains no recent list or search field.

Needs your attention carries the positive server decision count. Its live projection updates only that badge. History records show state dots. Untouched saved records read Draft. Responsive idle records read Ready.

Conversation pages carry their own header with the mobile menu trigger. The separate location bar stays for catalogue pages that need navigation and execution status.

Needs your attention lists real unresolved decisions with owning context links. An optional conversation identifier selects the return destination only: valid context shows Back to conversation, while any other value omits the return link. Refresh and decision pages preserve valid context, and every decision stays visible. History connects conversations to runs and evidence.

The sidebar resource group links directly to its catalogues:

- Environments.
- Workflows.
- Presets.
- Agents.
- Skills.
- Prompts.

Providers and Settings stay separate below the group.

The sidebar has no local status footer. Catalogue headers omit generic return links to conversations.

Supervised development initially exposes only Rebuild and restart. Active work or an unavailable idle probe replaces that action with a warning, Interrupt work and restart, and Cancel. The warning covers interruption across conversations, partial file changes and lost unsent text. Cancel restores the normal button without a restart request. Concurrent rebuilds and build failures do not expose interruption. A failed build leaves the current server active.

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

Model thoughts use muted 14-pixel text and a left rule without a filled background. Their width follows the content, up to 70 characters.

Thought text expands and fades over 240 milliseconds, then collapses over 180 milliseconds. The thought heading remains visible. Tool disclosures use the same timing. Reduced motion removes the expansion and collapse transitions.

Avatars sit beside desktop author labels and message bodies. Desktop circles measure 56 pixels. The Frink badge fills the avatar without a second background circle.

Mobile avatars sit beside the labels, above full-width content. Mobile circles and Frink badges retain their 32-pixel size.

Message text uses the transcript scale. Status stays beside the author. Revise, Fork from here and Copy appear below the applicable message.

An information icon beside the reply actions opens Reply details in a non-modal popover. It leaves the transcript layout unchanged.

The popover shows a shared model once and aligns request token counts in a table with totals. Each request links to its recorded context. Missing counts stay distinct from zero. Plan cost information appears once. Escape, Close and an outside click dismiss the popover.

A neutral summary above the composer shows recorded conversation totals, cache hit percentage and active-work timers. Context occupancy stays separate from cumulative token usage. Missing provider reports stay unknown or show an incomplete subtotal.

Both timers pause for questions and approvals. Completed durations persist. Earlier replies without recorded durations show no invented time.

Tool results use bordered disclosures with the actual tool label. Unfinished tools show their recorded name without an inferred path or result. Tool disclosures expose recorded call IDs and arguments when available. Separate calls retain separate records, even when their arguments match.

The retained-output control reveals the full record in place. It leaves the browser address unchanged and appears only when the retained record holds more text than the preview.

The mobile activity strip repeats the server's active reply status. It opens Current work without a command. Historical windows and settled replies omit the strip.

The model control opens a searchable popover above the composer. It lists connected providers with an optional provider filter. Favourites appear first, with a separate star control on each row. The active Favourites filter uses a soft tint and a check mark. Local application data stores favourites across browser sessions.

Tinted headings identify Favourites and All models. A divider and additional space separate the groups.

Search and filters retain the picker size and search position. The results fill the available space and reserve a stable scrollbar gutter. Short screens use a viewport-bound picker near the top, with space for the results.

Thinking effort stays visible beside the model as an outlined control with its label, value and caret. Both controls share the same height. The effort popover uses padded options and a tick for the current value. Models without adjustable effort show a disabled Not available control. Saved conversations apply model and effort changes immediately without changes to other settings. Unsent messages and unsaved setup fields survive those commands.

The selected model row offers Set default only when it differs from the saved default. A Default badge identifies the saved default, whether selected or not. The model picker has no defaults footer. The thinking menu retains its explicit default action.

Model selection keeps the picker open. Saved model changes retain search, filters and scroll position. Escape and an outside click dismiss the picker. The Accepts images filter and row badges identify models that accept image attachments.

Default actions apply to new conversations. Local preferences retain both choices across restarts. Existing conversations stay unchanged. A model without the preferred thinking level uses a supported level or Not available.

The composer places its editor above a compact toolbar. The editor limit is 32768 characters. The persisted message bound stays in the conversation store.

The paperclip opens image selection. Selection uploads automatically. The slash control opens skills and prompts. Composer help retains file references and prefix explanations.

Send uses an accessible Send message label. An empty composer disables Send unless it contains an attachment or prepared-change handoff.

Effective directory access appears below the composer. Empty and selected summaries share label geometry, so one-line state changes do not move the composer. The empty label stays transparent.

The empty access summary offers Add a directory. Without recent or selected directories, it opens the native picker directly. Otherwise, it opens the manager.

The execution label stays visible without directory access. Host mode names unrestricted access on the right. The left summary names work locations or their absence.

Job-bound cancellation reads Stop beside the composer submit control. It posts without a confirmation step. Current work keeps the same Stop control when the composer is inert.

Active replies use the follow-up placeholder and Queue label. A labelled selector retains After this work and Correct current work when both choices apply.

The shortcut hint distinguishes Queue from Send. The submit label follows the latest server patch after settlement.

Pending decisions retain their existing queue and consent rules.

Command approval appears at the transcript end, after the latest output and above the composer. It never opens or requires Current work.

The inline request shows the exact command, purpose and execution location. Approve and Reject retain the bound request identity. The host warning names unrestricted access.

A successful decision replaces the request with a collapsed acknowledgement. The next transcript refresh replaces that acknowledgement with the existing command record and its result.

Historical views link to the latest request instead of approval controls. Stop remains beside the composer. Approval retains unsent text.

Disabled model controls disappear during approval to preserve transcript space. A tall request starts at its heading when the reader follows the transcript end. Earlier transcript positions stay unchanged.

Jump to latest appears when the reader leaves the transcript end. New output does not move the reader away from earlier messages.

### Conversation header

New conversations show the title without an explanatory subtitle. Saved records identify their directory context beside the conversation title.

The new-conversation empty state asks what the user wants to work on. It contains no starter buttons or decorative mark.

Without recent or selected directories, Work with your files appears below the question. It explains directory access and places an outlined Add a directory button beneath the explanation. It contains no empty history message.

After selection, the prompt becomes the directory manager. Add another directory remains available. Recent directories shows up to ten shortcuts, with new additions first. Other directories in this conversation contains current grants absent from history.

Each row has an add/remove checkbox and Read and Write radios. Only the checkbox and its directory text activate selection. Selected rows apply access changes immediately. Unselected rows show the remembered mode with disabled radios.

Forget appears only on unselected recent rows. Selected rows reserve the same action space. Forget removes the shared shortcut without changes to other conversation grants or sensitive-directory approval. Access changes never recreate a forgotten shortcut.

Sandbox paths remain visible for selected directories. The row reserves their line height. Unavailable and overlapping paths show the reason beside their controls.

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

Command approval has no companion controls or attention strip. A pending command hides the Current work trigger and closes an open Current work panel. The queue note explains that new messages wait.

Evidence links show requests and bounded output. They start no command and make no current-file claim.

Current work retains the eligible job-bound Stop control. Cancellation leaves earlier file changes intact. Incomplete cleanup retains execution blockers.

The interface contains no file candidate, application decision, before/after preview, rollback or automatic commit control.

The attention strip stays within the toolbar and opens Current work without a command. Mobile rejection closes the companion to expose its authoritative error.

The error occupies a separate grid row instead of the toolbar area. Its focus remains visible, and the unsent draft remains intact.

### Setup

Setup occupies the companion position. Its section controls retain unsaved values when the user changes sections.

The visible sections are:

- Execution.
- Instructions.
- Presets.

The Presets section offers a preview for each preset and a separate Save this setup as a preset action.

Replacement hides the list and shows current and preset values. Each setting labels two value columns within the existing companion width.

A tint and Changed identify different values. Long values wrap. Instruction text retains a bounded scroll region with keyboard focus.

Cancel returns to the list without a command. Apply replacement remains explicit. Its explanation states that directory permissions carry across. Sensitive directories and host access need separate consent.

The save panel contains a name field and a settings summary. A separate disclosure contains the instructions. Back to presets retains the name.

Saved snapshots show stored settings only, even when execution changes await review. Draft snapshots show current choices. Both retain the unsent message.

The name accepts up to 80 UTF-8 bytes without control characters. An empty name uses the existing suggestion. Validation retains the entered name.

Preset controls retain the larger workspace scale and theme accent. Narrow screens stack the decision buttons. Every action remains accessible through the panel scroll area.

The directory manager occupies the same companion shell as Setup, without Setup sections or its defaults footer. Host mode names it Work locations.

Add a directory opens the native picker. Compact rows retain full host paths and visible sandbox paths. Selection applies the last chosen Read or Write mode immediately for ordinary directories.

A directory without history defaults to Write. Sensitive directories require initial consent. Frinkworks retains that consent for the exact canonical path across conversations and restarts.

Each directory row contains a button-style cwd radio, separate from Read and Write. A divider separates the controls. A tint and tick identify the selected cwd.

Selection changes the command start directory without changes to the visual list order or access modes.

Each cwd label uses a native title tooltip, including labels for disabled controls. The browser controls its delay and dismissal. Screen readers receive the same description through the radio control. Unselected and unavailable directories retain disabled cwd controls. Active work disables cwd changes.

Hypergraft retains keyed directory controls during patches. Its command guard blocks concurrent changes without a temporary disable state across the list. Uncertain results disable changes.

The execution summary below the composer names the selected sandbox beside Sandbox. The directory manager contains no repeated sandbox summary.

Saved access changes use a directory command when the user selects a radio. Ordinary directories need no execution review or additional approval. Active work disables access changes.

Drafts and saved conversations share the manager. Directory commands retain the unsent message and uncommitted Setup fields. The server patches their authoritative state together.

The access summary opens the inline manager in drafts and the directory companion in saved conversations. The execution summary opens Execution. Setup retains its independent sections.

Initial sensitive selection exposes an approval block in the manager. Cancel removes the pending request without consent. Existing unapproved grants retain Review access.

Host work locations do not confine access. Read grants block host execution until an explicit Write change. Host consent stays in Execution.

Directory controls expose exactly Read and Write. Write is the default for directories without history. Write changes original files immediately.

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

Save as future defaults is a separate explicit action in the companion footer. Its explanation includes directory permissions. Sensitive directories and host access need separate consent.

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

Resource catalogues and their forms use the shared catalogue layout. Their 24-pixel titles sit in a full-width header, with explanatory text beneath them. The content uses the same background and inset as Settings. Thin rules separate records, and the header stays above the content scroll area.

Decision entries link to conversations and exact gate pages. The decision list contains no approval form.

The Conversations catalogue pairs its directory filter with a title search. Controls use 44-pixel heights and seven-pixel corners. Mobile layouts place the directory filter and submit control beneath the search.

The conversation list has a maximum width of 768 pixels. Pale surfaces and eight-pixel gaps separate records without horizontal rules. Records use seven-pixel corners, 16-pixel titles and 14-pixel directory context.

Status dots align at the right edge, after their labels. Each record shows its update time. Exact UTC timestamps remain available through native time tooltips. Records omit the workflow label when no run exists.

The result count includes every match, not only the current page. The query trims and matches titles without case sensitivity. Both filters stay in the canonical address and native form navigation. Access grants stay unchanged.

Row checkboxes support individual selection, page selection and all matches across pages. The selection toolbar retains the count and Delete selected during scroll. Selected rows use a tint and accent checkboxes.

Permanent deletion requires a modal confirmation with the count and selected titles. Cancel and Escape retain the selection and restore focus. The confirmation fixes the selected identities and revisions before deletion. New matches stay outside that selection.

Active work and pending decisions block deletion. Stale selections delete nothing. Successful deletion removes the selected conversations together. Work directories and workflow run history stay unchanged.

Run history filters by stored canonical directory path. The filter applies before the fifty-record bound with newest matches first. Unavailable directories keep their labels. Run history stays run-centred.

Run details retain the owning conversation. Evidence pages retain their canonical run links.

### Other surfaces

Catalogue pages retain ruled records and inline editors. Workflow pages retain process previews, phase selectors and explicit run consent.

First-time provider connection starts with a focused provider chooser in a compact branded form.

The provider choice precedes the connection form. Method descriptions align right on wider screens and sit below provider names on narrow screens.

Provider choices and connected providers use pale rows with seven-pixel corners and eight-pixel gaps. Horizontal rules do not separate these rows.

Provider links use `/connect?provider=…`. Change provider returns to the chooser. API-only forms omit the redundant introductory sentence.

Pending plan sign-in and validation errors remain within the form. Successful plan polling navigates to conversations.

In-app Providers uses the shared catalogue header and content background. The storage note sits beneath the page title. Connection controls sit directly on the content background, with connected providers beneath them.

The standalone page uses the same background and retains its compact bordered form. Both versions share the provider chooser and connection controls.

## Validation boundary

The implementation status and evidence boundary are in `docs/conversation-system.md`.

Current browser checks cover desktop and mobile navigation with synthetic conversation data. Springfield and Sector 7-G retain their theme tokens.

The mobile handoff page passes the browser accessibility audit. This result does not establish accessibility for every execution state or theme.

Rust tests cover ownership transfer, restart and plan decision integrity. Real Microsandbox tests cover live Read and Write mounts.

An earlier Read/Write browser pass used isolated local data without a connected provider. App access now requires a stored provider connection.

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
