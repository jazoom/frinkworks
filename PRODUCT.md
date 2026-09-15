# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

A developer wants a local agent that can change a project on their machine.

They already have a provider API key or a ChatGPT or SuperGrok plan. They want a local conversation workspace, not a cloud agent account.

## Product Purpose

Power Plant is a local coding agent. The user runs a web server on their machine. They talk to a hosted model in a browser. Agents work in sandboxes.

The Power Plant process stays on the host. It owns model calls, credentials, and sandbox lifecycle. Tools use a sandbox or explicitly authorised host execution.

The product stays local and account-free. The user brings a key or a plan login.

Independent conversations are first-class work destinations. A conversation can discuss a general topic or receive explicit directory authority.

Success is a project file that an agent changes.

## Positioning

Power Plant is a thin local desk for coding agents. It is not an IDE. It is not a cloud workspace.

The user brings the provider key or plan login. Power Plant does not sell model access and does not create an account.

The user registers projects, agents, environments and workflows on this machine. Agents work in sandboxes on the local machine. Conversations retain their discussion history on this machine.

A conversation does not require a named agent or project. The user selects a model directly or applies an optional preset. Project attachments provide context references only until the user grants access.

A preset stores one complete snapshot of the implemented conversation settings. It includes the model, instructions, tools, environment, network policy and directory grants.

Preset application replaces the conversation settings after a full preview. It does not merge hidden permissions or copy sensitive-access consent.

A preset retains descriptive source provenance. Later source changes do not alter conversations or workflow phases that already copied its values.

The sidebar links directly to Presets at `/presets`. The page supports explicit creation, revision-bound edits and revision-bound deletion.

The preset editor shares instruction, tool and environment presentation with conversations. Model and thinking effort use catalogue dropdowns with model-specific effort choices and Not available for models without adjustable effort. A catalogue-listed model needs a supported effort and an empty value never selects a default. Preset edits request access but authorise no execution or sensitive access.

Unavailable providers, models, recipes and directories retain their requested values. Preset deletion needs no available provider or execution resource.

Conversation Setup identifies the source snapshot and local customisation. Local edits never update the source preset. The Presets page provides that explicit action.

Preset previews expire after 30 minutes. Each preview binds its replacement to the session and the destination settings revision or draft digest.

Preset application removes legacy project permissions. Missing providers, environments and directories remain requested values without substitutions or access approval.

Model steps use conversation settings at start, or explicit field overrides. Each field can use Same as run defaults independently. An empty overridden list removes that access for the phase.

The final review shows each phase's directories, network policy and environment. Additional access needs run-only approval and never changes the parent conversation. A single-use preview binds approval to the exact settings and one run.

Project-backed phases still reject presets that omit their required source directory.

A conversation can grant access to as many as eight host directories. A directory needs no project or Git registration.

Each directory uses one strategy:

- Read only exposes the host directory without write access through that mount.
- Review before apply exposes an isolated copy and requires approval before file application.
- Direct write exposes the actual directory with immediate host write access.

Direct write remains inside the sandbox. Directory boundaries and sandbox network policy still apply.

Each Direct write destination needs explicit session-bound consent for its exact directory identity and effective settings. Presets and copied settings retain requests, not consent.

Sensitive Direct write grants can alter or corrupt live configuration, permissions and execution evidence. The warning names the actual directory before approval.

Direct-only messages need no candidate capture or application gate. Mixed messages capture and review only reviewed roots.

Direct host changes remain after discard, cancellation and environment changes. A cleanup failure retains execution reservations until recovery settles the guest.

Configured model phases accept Direct write directories through inherited settings, presets or explicit per-phase paths.

The run preview names each direct destination and its immediate effects. Sensitive destinations include the stronger configuration warning.

Each run needs its own single-use consent for the exact resolved phase settings. Conversation consent never authorises a configured run.

A root cannot use both reviewed and direct write strategies within one run. Read-only narrowing remains valid.

A launch override cannot replace a reviewed root with Direct write when the process requires candidate approval. Required candidate outputs still need a reviewed root.

Distinct roots can combine reviewed and direct strategies. Candidate evidence and approval cover reviewed roots only.

Completed-direct task outcomes require successful outputs and managed cleanup. Earlier direct changes remain after a later failure.

Each grant stores the canonical directory identity and a stable `/access/<alias>` guest path. Duplicate and overlapping roots are invalid.

The first authorised root is the default command directory. Without a grant, tools use private scratch storage at `/workspace`.

Power Plant revalidates the device and inode before execution. A missing or replaced root remains visible as unavailable and does not retarget.

Sandbox network access defaults to Off. Conversation settings permit restricted domains or public internet access. Private and host networks remain excluded. Provider traffic remains separate and leaves the host for model inference.

A tool-enabled conversation message starts the system-owned Quick task. Without directory grants, tools use private scratch storage at `/workspace`. Power Plant mounts each directory according to its approved access strategy beside that scratch storage.

Scratch storage belongs to one attempt, not the conversation. Source-free tools create no Git candidate, code approval gate or commit.

A Review before apply message creates an isolated candidate. The user reviews the exact candidate before Power Plant updates ordinary host files.

Generic application creates no Git commit. Power Plant checks every directory, baseline, candidate object and pinned exclusion before the first write.

Each reviewed directory stays bound to its stable grant identity. The preview identifies changes by directory and relative path.

Application journals retain progress and outcomes for each directory. Writes across filesystems are not atomic.

A later conflict can leave an earlier directory applied. Recovery retains known per-directory outcomes and releases the execution blocker when no uncertainty remains. Partial application never appears as Completed.

Recovery restores only known transaction states. Unrelated host edits cause a conflict. Uncertain recovery retains evidence and blocks execution.

Reviewed capture excludes the application journals and other listed engine paths. Generic application never writes to those exclusions or Git administration.

## Operating Context

The user starts the Power Plant process on their machine. The default origin is `http://localhost:4000`.

They open that origin in a browser. They add a key or a plan login for a provider that is not stored. Power Plant can store one credential for each provider at the same time. The connect page offers only providers that are not stored.

They choose a provider, model and model-specific thinking effort in each conversation.

New conversation is the primary navigation action. The persistent index shows up to twelve recent conversations with real titles and status.

The sidebar search filters those recent titles live as plain text, with the catalogue link as the native fallback. Needs your attention shows the positive server decision count, refreshed live. Untouched saved records read Draft, while responsive idle records read Ready.

Needs your attention lists unresolved candidate, plan and host-command decisions across conversations. Each entry opens its owning context without execution approval.

The decision view uses pages of thirty entries. Refresh decisions reads the current server state. An optional conversation identifier carries the return destination from a conversation: valid context shows Back to conversation and survives refresh and pagination, while all decisions remain visible.

History links the newest fifty runs and task loops to their owning conversations. Child runs retain parent links and immutable evidence.

The sidebar links directly to Workflows, Presets, Environments and Providers. Settings stays separate below those resources.

Workflows at `/workflows` offers Use workflow beside the authoring controls. Presets at `/presets` offers Preview setup beside the revision-bound editors.

An optional conversation identifier carries the destination from a conversation. Valid context hands off to that record. Any other value keeps resource selection on the canonical page beside a destination chooser. The chooser never selects a conversation automatically. If no conversations exist, the page offers New conversation and explains that setup resumes after the user saves a conversation.

Resource navigation creates no record and starts no workflow. It grants no access. Workflow handoff opens the existing setup flow with the selected workflow. Preset handoff opens the destination conversation where the existing session-bound preview command runs before the explicit Apply action. Stale resource identities report an error without substitution.

More resources in the sidebar contains Projects and Agents. The sidebar has no local status footer.

Catalogue headers omit generic return links to conversations. A validated conversation context retains its specific return link.

Directory filters in conversation and run history provide work discovery without additional access grants. Conversation history also offers a title search that trims and matches without case sensitivity.

Each legacy project has a name and one immutable host path. The path must be a supported Git worktree. Users can create projects and rename them. They cannot edit paths or delete project records.

The product marks a project unavailable when its stored path no longer resolves. Unavailable records stay visible.

The `/` route opens `/conversations`. The conversations route lists local history and offers a New conversation action. A conversation can start without a project or preset.

The new-conversation page has no record or conversation identity. Navigation and invalid submissions create nothing in memory or local storage.

New with same settings opens an unsaved draft from a conversation, including one with pending work. It copies the implemented settings as independent values, not a live source reference.

The draft copies no title, messages, documents, review associations, reservations or runtime consent. Sensitive directory grants show Pending approval. Source edits or deletion do not change the draft. The source retains its pending work.

The first message revalidates the submitted settings and directory identities. Unavailable resources receive no substitutions.

Only a valid first message creates the record and replaces the page URL with its canonical address. Model, effort, preset, directory and title choices remain unsaved page state before that message.

The folder chooser can add grants before that message. Chooser cancellation creates no grant or conversation record.

Provider choices contain connected providers only. Model and thinking effort dropdowns use the models.dev snapshot. A model without adjustable effort shows Not available.

Project-scoped entry carries an intended project reference, not file access. Saved conversations offer explicit project grants, documents, workflows and review work.

At the conversation limit, Power Plant rejects a new save. Existing conversations remain unchanged.

The project catalogue is not host access authority. Explicit conversation grants authorise conversation work. Saved-agent execution uses agent directory grants. An agent is eligible when one stored canonical grant path equals the project path. Prefix matches give no authority. Project registration gives no authority.

They create agents in the local catalogue. Each agent has a name, instructions, tools, a network policy and host directory grants. The agent form offers connected providers and catalogue models with model-specific effort choices, and shows `Not available` for models without adjustable effort. The selected project grant maps to `/project` during sandbox-backed steps. A workflow step can expose selected secondary grants as read-only context under `/access/<alias>`. Grant aliases are labels only. The typed folder path decides host access.

An agent network policy permits no network, selected domain suffixes or the public internet. No network is the default. Public access excludes the host and private networks. Restricted access includes each listed domain, its subdomains and all ports.

They create environment recipes with an OCI image and an optional setup script. A new installation includes a starter Git environment. Power Plant selects that starter for new conversations by default. The user can select another recipe for later attempts.

An idle conversation owns no persistent sandbox. An environment change selects the ready snapshot for subsequent attempts only.

A change of backend, approval policy, directory strategy or environment has one combined preview in Settings. The preview describes effective values, not preset names.

An idle conversation offers Change execution settings. Active or reviewed work needs explicit settlement first. Effective settings stay unchanged until settlement succeeds.

An active task needs an explicit Stop task and switch action. Power Plant cancels waiting host commands, waits for managed cancellation and sandbox cleanup, then saves the selection. Uncertain completion blocks the switch.

Pending reviewed changes need review or an exact-candidate discard before a switch. A discard retains immutable evidence, conversation history and existing host files. Direct writes and host side effects remain unchanged by the switch.

A switch to Sandbox needs a ready snapshot, including an idle switch or a preset replacement from host mode. This computer needs no sandbox runtime or available recipe. Obsolete consent is invalid. Sensitive directories, Direct write and automatic host commands need new destination consent in the replacement configuration.

Power Plant queues preparation for each recipe. A successful preparation creates a local snapshot. Each tool attempt pins the selected ready snapshot. The conversation does not own a persistent sandbox. Tool-free chat requires no sandbox runtime or snapshot.

They create workflow definitions with a default environment, roles and ordered steps. A definition can run once or repeat one group of phases for each remaining task. A task list is not required for a one-shot definition. Ordinary chat still needs no workflow.

A new installation includes seven starter processes.

The starters are:

- Implement a saved plan
- Plan a change
- Review current code
- Implement with approval
- Implement and review
- Plan then implement
- Task loop

Each process preview shows its required inputs, candidate effects and approval stops. Independent review appears only when that process contains a separate read-only review phase. The task loop repeats implementation and optional review-and-fix. It does not add an extra independent review. A step can run an agent, a registered system command or a human gate.

They open a project to see its conversations. The project page lists conversations that reference the project and offers New conversation. The new conversation form carries the project reference without granting file access.

The old project-and-agent desk is not a conversation route. Project work starts at a conversation. Agent configuration stays at `/agents` and `/agents/{agent_id}/configuration`.

A writable conversation message starts the system-owned Quick task. Quick task is a system-owned one-agent run. It needs no configured workflow. It uses the environment selected in the conversation. The product does not start a run when that selected snapshot is absent or unavailable.

A read-only project grant produces a one-step Quick task. The step can answer and inspect files. It cannot produce a candidate revision.

A writable project grant produces three steps. An agent step creates a candidate. A human gate approves it. The current commit executor applies it.

An unchanged Quick task completes automatically after the assistant reply. The product creates no gate, decision artefact or commit attempt.

A changed Quick task waits at a human gate. The user must approve the exact candidate. The host worktree does not change before that approval. Approval creates a local Git commit through the current transaction path.

Configured workflows start from Run a workflow above the transcript. Setup opens in the conversation companion at its canonical URL.

Choose, inputs and review retain the brief and selected revisions through Back controls. Preview starts no work.

The content scrolls separately from the action footer. Mobile closure restores focus to Run a workflow.

Review distinguishes run defaults from effective phase settings. Host summaries name unrestricted access and the requested command policy, not sandbox boundaries.

Provider and environment failures retain direct recovery links. Large workflow forms use the standalone representation at the same URL.

New starters use Task loop. Untouched older starters receive that presentation label without a stored definition change. User-authored names and pinned run names remain unchanged.

Ordinary one-shot workflows copy conversation settings and directory grants into their phase snapshots. They need no project registration or saved agent.

Plan a change can run without directories. Its tools use private scratch storage at `/workspace`. Processes with required directory inputs reject missing roots before execution.

The ordinary implementation starters prepare candidate sets across authorised reviewed directories. Apply changes updates those directories after approval and creates no Git commit.

A read-only review inspects isolated candidate copies, not live host files. Other authorised reference directories remain read only. The first authorised directory supplies the command directory.

Each model phase receives its pinned model, instructions, tools, directories, sandbox network policy and environment snapshot. Same as run defaults copies the conversation settings. Custom or preset values replace the corresponding lists for that phase.

Preset changes and deletion do not change those pinned values. Additional requested access needs explicit run-only approval.

Dispatch revalidates pinned directory identities and session-bound destination consent. Transcript and title changes do not constitute access approval.

The preview lists exact engine paths that source capture excludes. Candidate manifests retain those exclusions. File application cannot change excluded paths.

Model context identifies actual guest paths and effective access. Root instruction files come only from authorised directories, with the existing combined text bound.

Explicit Git commands retain an explicit supported project binding. Directory order never selects a Git destination. Task-loop conversion remains separate from ordinary one-shot workflows.

Task-loop completion recognises verified generic file application separately from commits. Every application root must report Applied or Unchanged, and managed cleanup must finish. Unsettled application transactions block completion, continuation and retry. Host task loops need no project registration or sandbox environment.

Plans opens the conversation companion. It contains explicit plans and standalone task lists. Generated breakdowns stay under their source plans.

Current work contains execution progress, host command approval and candidate review. The transcript and composer remain separate from those controls.

Current work also exposes eligible job-bound cancellation while the mobile composer is inert. Cancellation leaves direct writes and host command effects unchanged.

Observation resumes after parent command patches, including cancellation. New output preserves the transcript offset when the reader leaves the end.

Candidate review shows bounded file previews and the exact application destination. Expanded review retains the owning conversation and its URL.

Independent plan review and candidate review choose the reviewer through the same catalogue controls as conversations. Provider choices contain connected providers only. Model and thinking effort dropdowns use the models.dev snapshot. A model without adjustable effort shows Not available. Optional reviewer presets stay explicit, the source revision stays pinned, and the review starts only through its explicit command.

Setup occupies the companion position. One native selector replaces the former hidden section tabs. Section changes retain unsaved fields.

Effective access stays beside the composer until a successful settings switch. Setup closure restores focus after a command replaces its original trigger.

On mobile, an open companion excludes the hidden transcript and composer from interaction. Closure returns focus to a conversation control.

Ordinary responses create no plan automatically. Tool-free conversation exposes explicit plan actions separately from file and command tools.

The model can submit these actions:

- `create_plan`
- `revise_plan`
- `create_task_breakdown`

Each response permits one validated action. The action stores its contents and provenance together, after the provider stream completes successfully.

Create a plan from this requests a model action. Add your own plan stores supplied text without a model call.

Plan revisions add immutable action entries with their original titles and contents. Action details identify the author and exact revision.

Plan actions grant no execution or application approval. Explicit requests use no directory access or sandbox, even when the conversation permits execution tools.

Revision and breakdown requests bind publication to the selected document and revision. Their model context contains the selected source, not newer plan contents.

Prepare implementation opens the existing workflow preview with an exact plan revision. The Implement a saved plan starter requires no task breakdown.

Break into tasks requests an explicit breakdown from the selected plan revision. Generated breakdowns permit at most 128 tasks.

A later source revision marks an older breakdown as outdated for a new run. Existing runs retain their own pinned inputs.

Standalone task imports remain independent documents. Completed replies still offer an explicit Save as task list action. A saved standalone list records an Added tasks transcript action with its immutable revision and provenance and no plan association.

Task lists offer Run tasks, which opens workflow setup without execution. Document views retain export and independent review.

The document catalogue retains action provenance after association removal. Existing records remain readable without a migration or format-version change.

An explicit import reads one file from an authorised directory through a read-only sandbox with the selected environment and no network. Import calls no model and executes no tasks.

Import rejects parent paths and symlinks. It revalidates directory identity and applicable session-bound consent before access and publication. Missing environments receive no substitution.

Task lists retain their preamble, checked tasks and indented details. Unchecked tasks are eligible. Fenced examples are not executable entries. Preview, linked review and Export use the saved revision.

The Task loop starter runs each remaining task in order. It pins the task list, workflow body and phase selections at launch. Each task receives fresh implementation and optional review-and-fix contexts. The complete task file remains context, but each worker receives only one assigned task. Human approval precedes each task commit by default. An extra independent review is present only when the selected definition includes that phase.

One parent record shows task progress and links to child evidence. A successful commit or explicit no-change result permits the next task. The next task starts from the completed code, not an earlier worker transcript. A child approval gate retains the parent conversation reservation but releases execution reservations for another conversation. Earlier commits remain if a later task fails.

A workflow can declare a saved plan input. The authoring form offers Implementation from a saved plan without manual input keys or a task list.

The launch sheet selects an immutable plan revision from the conversation. The run retains its own copy and the source reference. Only phases that declare that input receive the plan. Later corrections or association removal cannot change the run copy. The run inspector exposes that copy.

Project-backed runs record the actual project identity. Directory-backed and source-free conversation workflows record neither a project identity nor an agent identity.

Run kinds are Configured and Quick task. A run can belong to an independent conversation.

The browser session remains a command boundary, not a conversation identity. The session permits one active command while a conversation reservation protects its unfinished operation. A safe approval gate releases the session reservation so another conversation can run a command.

Each run pins the workflow definition and prepared environments. Run records track attempts, artefact references and human gates. Artefacts store candidate revisions and typed workflow outputs, such as plans, reviews, tests and human decisions.

A human-gate step pauses the run for a decision about an immutable candidate diff. Writable Quick tasks also support Request changes. Configured gates offer this action only with a declared revision route.

A human revision starts a fresh implementation attempt from the rejected candidate. It retains the original task brief and diff base, plus candidate-bound feedback. Earlier implementation transcripts stay outside the new context. Required reviews and code approval repeat before commit.

Revision limits apply across reopened gates. An exhausted limit blocks the run and retains its evidence. Model verdict routes remain distinct from human revision routes.

Explicit Git processes can offer human approval before commit or automatic commit after an approved exact-candidate review. Automatic commit requires review assurance.

Ordinary reviewed starters require a human decision before file application. Read-only and source-free processes show no application policy controls.

The run list shows the newest fifty runs.

Rig streams model replies on the host. Sandbox steps can use the allowed list, read, write and run tools. Host conversations expose Run only. The transcript shows the reply as HTML. Tool traces appear in the transcript.

Forget removes one provider. The connect page stays available so they can add another provider.

Hypergraft updates page fragments. Ordinary links still work without it.

Conversation history and project configuration persist locally. Browser sessions hold request reservations, not conversation identities.

Theme and thinking visibility preferences persist on the local machine.

## Capabilities and Constraints

Current capabilities:

- Accept an API key for xAI, OpenAI Codex, Synthetic, OpenRouter or DeepSeek.
- Sign in with a ChatGPT plan or a SuperGrok plan from the connect page.
- Store more than one provider on the local machine.
- Choose the provider, model and model-specific thinking effort in a conversation.
- Use bundled models.dev metadata when a model advertises adjustable thinking effort.
- Refresh the models.dev capability catalogue manually from Settings.
- Create projects in the local catalogue.
- Rename projects.
- Open a project and see its conversations.
- Create a conversation with a project reference.
- Grant conversation access to a project.
- Create agents in the local catalogue.
- Edit and delete agents.
- Grant host directories to each agent.
- Configure the list, read, write and run tools for each agent.
- Deny agent network access, allow selected domain suffixes or allow the public internet.
- Create environment recipes from an OCI image and a setup script.
- Edit and delete environment recipes.
- Prepare environment snapshots for workflow use.
- Select an environment for each conversation and its later tool attempts.
- Save conversation or draft settings as a named preset.
- Preview and apply a preset as a complete settings replacement.
- Stop active work or discard reviewed changes before a change of backend, approval policy or environment.
- Keep current effective settings visible until an execution switch succeeds.
- Require a ready snapshot before Sandbox becomes the next tool backend.
- Invalidate waiting host commands on an accepted switch. Direct writes and host side effects remain.
- Create workflow definitions, including Run once and For each task modes.
- Edit and delete workflow definitions.
- Send a Quick task from an independent conversation with no workflow selection.
- Choose Sandbox or This computer for where tools run.
- Use sandbox tools with private scratch storage and no project.
- Run host commands without a sandbox runtime.
- Retain redacted command evidence, including rejection, dispatch and completion states. Evidence never authorises replay after restart.
- Choose Ask each time or Run without approval for host commands. Ask each time is the default.
- Approve or reject each host command before it runs when Ask each time is in force.
- Require a fresh destination consent that names Run without approval before automatic host commands.
- Copy the requested host approval policy into presets and new conversations without approval tokens or automatic authorisation.
- Show unrestricted host access, the effective approval policy and work locations when tools run on this computer.
- Set sandbox network access to Off, restricted domains or public internet.
- Grant Read only, Review before apply or Direct write access to multiple directories without project registration.
- Preserve stable guest aliases and canonical host identity for conversation directories.
- Grant read-only or writable project authority from a conversation through the legacy project flow.
- Review a candidate diff and Apply or Discard it from the owning conversation.
- Review and apply one immutable candidate set across multiple ordinary directories without a Git commit.
- Recover interrupted file application from retained preimages and transaction progress.
- Launch a configured workflow from a conversation.
- Offer This computer and Ask each time or Run without approval on workflow model steps.
- Use work locations for host workflow steps. Sandbox network and environment stay separate.
- Preview each host phase approval policy, including Run without approval, before start.
- Bind host workflow consent to the run or task loop. Parent conversation consent does not authorise the run.
- Bind Ask each time host commands to the exact run, step and attempt.
- Record host workflow completion separately from file application.
- Stop host workflows after command failure, timeout or termination at the output limit. Earlier host effects remain unchanged.
- Show the effective host approval mode and pending commands on the run and conversation.
- Restart never resumes automatic host workflow commands.
- Stream the reply into the transcript as HTML.
- Run sandbox-backed workflow steps in isolated guests.
- Expose the selected project at `/project` during sandbox-backed steps.
- Expose selected secondary directory grants as read-only context.
- List, read and write project files in the guest.
- Run a command in the guest.
- Complete an unchanged Quick task after the assistant reply.
- Show a candidate diff at a human gate.
- Apply an approved Quick task candidate as a local Git commit.
- Discard a Quick task candidate with no host change.
- Approve a candidate at a configured workflow gate.
- Request a revision at a configured workflow gate.
- Cancel a run at a human gate.
- Store run records, artefacts and human gate decisions on the local machine.
- Inspect run records and their artefacts.
- Show the newest fifty runs in the run list.
- Show tool traces in the transcript.
- Choose whether model replies show thinking details.
- Choose from five colour themes stored on the local machine.

Current constraints:

- The product does not create user accounts.
- A conversation stays a transcript plus composer with an optional work companion and explicit access controls.
- Sandbox tools run only in the guest. The guest does not receive a provider key or plan token. Agent steps use the saved network policy. System-command steps have no network access. Environment preparation permits public destinations but excludes the host and private networks. Model inference stays on the host through Rig. Host mode runs approved shell commands as the Power Plant process user. It adds no privileges. Approval covers the submitted command, not script internals. Command output is sent to the hosted model.
- Conversation history, presets, project records, run records and artefacts persist locally. The old project desk routes no longer exist.
- One session command can be active at a time.
- One unfinished operation can reserve a conversation.
- One workflow execution can be active process-wide.
- The project catalogue grants no file access. Conversation directory grants and saved-agent grants supply execution authority.
- Directory grants support Read only, Review before apply and Direct write. Writable strategies and sensitive access require explicit destination consent.
- Tools cannot combine legacy project access with conversation directory grants.
- Project paths cannot change. Project records cannot be deleted in this release.
- Sandbox-backed Quick task needs the selected ready snapshot. The product does not fall back to another environment.
- Environment changes wait until active execution and pending reviewed work settle.
- Tool-free chat needs no sandbox runtime or prepared environment.
- Host commands need no sandbox runtime or prepared environment.
- Host workflow steps cannot bypass required candidate approval. A commit action stays a registered system command.
- Quick task never enters the workflow catalogue.

Later work:

- Task boards
- Parallel workflow execution

## Brand Commitments

The product name is Power Plant. The wordmark in the product is `Power Plant`. The mark is `app/public/images/logo.svg`.

UI copy uses Australian English. Capitalise only the first letter of a title, button or heading.

## UI validation

[The final UI report](docs/ui-overhaul/MILESTONE-4.md) records the capability audit and browser evidence for milestones 1–4.

The UI overhaul preserves the existing execution engine, consent boundaries and persisted format versions. It removes no catalogue capability.

Final browser exercises include scripted tool-free responses and cancellation without directory access. They also include unavailable-environment rejection and provider recovery.

Successful hosted responses and successful reviewed file application remain unverified in a browser. Automated recovery tests do not replace that execution evidence.

## Evidence on Hand

There are no testimonials, customers, benchmarks or launch claims. Do not invent them.

The product has connect, project, agent, workflow, environment and run surfaces. The logo file exists.

## Product Principles

1. The user runs Power Plant. There is no product account.
2. A hosted model is a connection the user brings, not a Power Plant service.
3. A conversation is the main work destination. Project access remains explicit. Power Plant is not an IDE or a cloud workspace.
4. Power Plant stores a key or a plan login on this machine until the user forgets that provider.
5. Agents work in sandboxes. Success is a project file that an agent changes.
