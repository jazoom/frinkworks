---
version: 1
slug: "route-workflows"
primary_target: "route:/workflows"
related_targets:
    [
        "route:/workflows/new",
        "route:/workflows/{workflow_id}/configuration",
        "route:/conversations/{conversation_id}/workflow",
    ]
---

# Workflows

## Mode

Operate

## Scope

This surface covers the workflow catalogue, workflow authoring and the conversation launch sheet.

## Audience and job

A local developer chooses or edits a repeatable process before a model run starts.

## Hierarchy

The sidebar links directly to the workflow catalogue. Each workflow offers Use workflow beside its configuration link. Process details stay collapsed until requested.

A validated conversation context selects the destination. Without context, Use workflow opens a conversation chooser on the canonical catalogue GET.

The chooser starts no work and grants no access. Invalid or stale workflow identities report an error without substitution.

The ordered process appears before configuration fields.

Run once and For each task appear before phase settings. A repeated group shows per-task phases once.

Each phase shows its purpose, candidate effect, model context boundary and approval route. Independent review is named only when that phase exists.

Configuration uses a phase selector. The selector does not submit the form. Hidden phase panels retain their values.

Environment overrides and technical mappings appear under Advanced settings.

Conversation setup starts with workflow names and short outcomes, not an expanded catalogue of phases.

The canonical conversation workflow GET uses explicit choose, inputs and review states. Navigation creates no run or execution reservation.

Only the selected process shows detailed phases through the shared process template. Its input state shows the required brief, plan or task list.

Model and preset overrides stay collapsed until requested. Each model phase can use Same as run defaults or custom settings. Individual fields can inherit run defaults. An overridden list replaces the corresponding list.

Review shows the exact phase settings and sensitive directory warnings. Approval applies to one run. Error responses retain field values and reveal the first invalid phase.

Model phase fields include Direct write directories, Where tools run, and host command approval. The process preview names immediate host effects separately from reviewed candidates.

This computer uses work locations. Sandbox network and environment stay separate. Host steps need no sandbox environment.

A backend choice updates the inline execution controls. The form retains the host approval preference and unsaved instructions across these previews.

Ask each time binds each command to the exact run, step and attempt. Run without approval needs explicit launch authorisation for that run or task loop.

Direct changes remain after failure, discard or cancellation. Candidate approval covers reviewed roots only, not direct effects on separate roots.

Each run needs destination consent for its resolved phase settings. Copied settings and parent conversation consent supply no run approval.

A launch override cannot change a reviewed root to Direct write when the process requires candidate approval. Host overrides cannot bypass required pre-application candidate guarantees. A commit action stays a registered system command. Read-only narrowing remains valid.

The final review identifies selected context, effects, effective access, environment readiness and approval stops. Workers receive selected inputs, not the entire conversation history.

Workflows… opens setup from the conversation. Start workflow starts execution only after review. Back controls retain entered values.

Stale workflow selections return to the chooser without a substitute. Unavailable target selections retain their value and require an explicit replacement.

Storage integrity explanations stay outside the chooser.

Authoring retains ordinary phase controls. Model steps offer Same as run defaults or the implemented conversation settings. Advanced settings group environment overrides and technical mappings. Human gates and registered system commands expose only settings relevant to their function.

## Interaction

Use real buttons for phase selection. Keep the current phase visible and expose keyboard focus.

Keep all phase fields in the form. Hide inactive panels without disabling their inputs.

Show bounded revision destinations beside the review or approval phase that uses them.

Show invalid field values and server errors without replacement of submitted values.

If a hidden phase contains an invalid control, reveal that phase before browser validation moves focus.

If the server returns phase errors, reveal the first affected phase and open its advanced settings.

Keep the page usable at narrow widths. Let phase tabs scroll horizontally without changing the process order.

## Constraints

Use the existing case-file visual system, square corners and ruled sections.

Use DaisyUI controls and Tailwind layout utilities.

Use Australian English and sentence case for headings and buttons.

Do not introduce a new colour palette.
