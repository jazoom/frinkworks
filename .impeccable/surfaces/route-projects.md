---
version: 1
slug: "route-projects"
primary_target: "route:/projects"
related_targets:
    [
        "route:/",
        "route:/conversations",
        "route:/projects/new",
        "route:/projects/folder",
        "route:/projects/{project_id}",
        "route:/projects/{project_id}/configuration",
        "route:/runs/{run_id}/gates/{gate_id}",
    ]
---

# Projects

## Mode

Operate

## Scope

This surface covers the project catalogue, project detail and the conversation hand-off.

## Audience and job

A local developer opens a project and starts work in an independent conversation.

## Operate-mode hierarchy

The desktop product index groups routes into Work, History and System.

Conversations remain the primary work destination. History contains Runs.

The sidebar links directly to Workflows, Presets, Environments and Providers. More resources contains Projects and Agents. Settings stays separate below those resources.

Conversations is the main work destination. The brand mark and the first index link go to `/conversations`. Providers uses `/connect` with full-page native navigation.

The project title and the New conversation action form the first visual level. The host path sits under the title as quiet monospace metadata.

The project page lists conversations that reference the project. Each row has a canonical conversation link.

A New conversation action submits a POST to `/conversations` with the project identifier. The response opens the new conversation directly.

A GET never creates a conversation. A project reference does not grant file access. The conversation page supplies explicit read-only and writable access controls.

Project registration does not grant model or filesystem access. The conversation page shows access effects before each grant.

## Primary action

On the catalogue, the primary action is New project. Each project row has Rename as a secondary action.

On project detail, the primary action is New conversation.

The project page also offers Start project work. Both actions create and open a conversation with the project reference.

On the new project page, the primary action is Add project.

The project folder group always shows the path field and the Choose folder action.

## Readiness states

Project detail shows the saved folder state before the conversation list.

An unavailable folder keeps its project record and shows a warning. The page does not offer an implicit access grant.

Conversation detail owns model readiness, project access, sandbox readiness and workflow readiness.

A configured workflow starts from Run workflow beside the conversation title. The launch sheet shows its task brief, target, access, environment readiness and fresh-context boundary.

A writable conversation message starts Quick task through the conversation route. It uses the pinned Alpine Git environment.

## Empty states

No projects: `/projects` opens the new project page. That page accepts an existing Git folder.

No conversations: project detail explains that the first conversation can start without a preset. New conversation remains the primary action.

No provider: the conversation page links to `/connect` before Send can start model work.

No project access: the conversation page labels each attachment as context only. Grant controls show their tool effects.

## Mobile topology

The mobile shell uses a compact masthead and a primary row of Conversations, Workflows, Runs and More.

The mobile menu uses the same resource links as the desktop sidebar.

The More control is a native details disclosure with a DaisyUI menu. Every destination is a real link.

The Conversations page and its directory filters provide work discovery.

Do not put project names in the permanent mobile row.

Project detail actions wrap at narrow widths. New conversation remains visible without a mandatory preset choice.

## Flow

`/` opens `/conversations`.

`/conversations` lists local history and offers New conversation.

The New conversation form submits directly to `/conversations`. Its optional project identifier becomes an attachment without a grant.

Creation has no intermediate page or title field.

`/projects` lists registered projects. It also opens the new project page when the catalogue is empty.

`/projects/{project_id}` lists conversations that reference the selected project. It does not select an agent or redirect to an old project desk.

The old project-and-agent desk URL is not a conversation URL. The project page uses canonical conversation links or direct creation commands.

After a project is created, the product opens its project detail page. The user can then create a conversation without choosing a preset.

After a provider connects, the product returns to `/conversations`. The Providers route remains available from the product index.

Human-gate links for conversation-owned runs return to the owning conversation. Legacy run links return to project detail instead of an old desk.

## Constraints

Keep a real `href` on every ordinary navigation action.

Do not create a conversation during a GET request.

Do not put a host path in conversation route parameters or query values.

Do not reinterpret `/projects/{project_id}/agents/{agent_id}` as a conversation.

Do not require a preset to create or open a conversation.

Keep configured workflow launch available from conversation detail.

Keep project access explicit. Project registration and attachment do not grant filesystem access.

Use the path field on `/projects/new` for automated first-task checks.

## Unresolved

None. This brief records the conversation-first navigation.
