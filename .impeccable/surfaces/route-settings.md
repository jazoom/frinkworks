---
version: 1
slug: "route-settings"
primary_target: "route:/settings"
related_targets:
  - "route:/settings/local-data/reset"
---

# Settings

## Scope

The primary target is `route:/settings`. The visitor mode is Operate.

## Job

A local developer chooses the colour theme for Frinkworks. The selected theme applies immediately and persists on the local machine.

The same page offers a confirmed local data reset. Reset records a request. The next start removes owned local data before stores open.

## Content and constraints

Theme is the first setting. Six radio choices show miniature workspaces with the actual theme colours. The selector offers System preference and five colour themes.

The grid uses three columns on wider screens and two on narrow screens. A border and tick identify the selected theme. Keyboard focus outlines the whole choice. Selection applies immediately. Hover leaves the current theme unchanged.

System preference is the default. Its preview shows one workspace with a diagonal colour boundary. Springfield occupies the upper-left half and Stonecutters occupies the lower-right half. Both halves share the same layout.

System preference uses Springfield for light mode and Stonecutters for dark mode. An explicit theme choice overrides the system preference.

Presets and Environments have direct sidebar links. Settings contains no resource catalogue or generic return link to conversations.

Springfield, Evergreen Terrace, Leftorium, Stonecutters and Sector 7-G form one Springfield-inspired collection.

Local data is a separate danger section. Reset removes providers, projects, agents, environments, workflows, runs, artefacts and preferences.

Project source directories outside the Frinkworks data directory remain unchanged.

The destructive action requires the checkbox labelled "I understand that this deletes all local Frinkworks data." The command label is "Reset local data".

A successful command replaces the main page with "Stop and restart Frinkworks to finish the reset." The next start removes local data before normal store initialisation.

The command does not stop the process. The browser does not submit a deletion path.

The page inherits the repository case-file shell. It uses a canonical GET route with document and Hypergraft navigation representations. Reset is a patch-only POST command.

## Memorable moment

The full desk changes colour while the selector stays in place.

The reset status page gives one instruction: stop and restart Frinkworks.

## Unresolved decisions

None.
