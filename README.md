# Power Plant

Power Plant is a local coding agent. The process is a web server. You use it in a browser.

The stack is Rust, Axum, Askama, Hypergraft and Rig.

## Run

Install the tools.

```sh
mise install
pnpm install
```

Build the frontend assets.

```sh
mise run assets:build
```

Start the server.

```sh
mise run dev
```

Open `http://localhost:4000`.

Connect with an API key for one of these providers:

- xAI (Grok)
- OpenAI Codex
- Synthetic

The key stays in process memory. The key is not written to disk.

## Skills

Skills supply reusable instructions. They grant no file or command access.

Global skills are available across projects. The Skills page in the sidebar manages these skills.
Power Plant stores each global skill at `<data_root>/skills/<id>/SKILL.md`.

The data directory follows this order:

- `POWERPLANT_DATA_DIR`, when set.
- `$XDG_DATA_HOME/powerplant`, when set.
- `$HOME/.local/share/powerplant` otherwise.

Project skills stay in the project at `.agents/skills/<name>/SKILL.md`.
Power Plant discovers them below each directory in the conversation or workflow settings.

### Add a global skill

1. Open Skills in the sidebar.
2. Enter a name, description and instructions.
3. Select Save skill.

### Add a project skill

1. Create `.agents/skills/review/SKILL.md` in your project.
2. Add a frontmatter header and the skill instructions:

```markdown
---
name: review
description: Review code for security and correctness.
---

Read the changed files.
Report defects with file paths and line numbers.
```

3. Add the project directory to the conversation or workflow settings.
4. Enable the read tool.
5. Start a new request.

### How the model uses skills

Host and sandbox modes advertise skill names, descriptions and read paths.
The model reads a skill body with the read tool when the task needs it.
Power Plant does not insert every skill body into each request.

Power Plant advertises at most 64 skills per request preparation.
Each skill file has a 256 KiB limit. The frontmatter header has a 4 KiB limit.
The name and description each occupy one line. Skill discovery rejects symbolic links.

The selected tools work in both modes. Host tools use Power Plant's host permissions and need explicit host consent.
Host file changes take effect immediately. Host command approval applies only to Run.

## Tasks

- `mise run dev` starts the development server.
- `mise run clean` formats the code and runs the checks.
- `mise run test` runs the test suite.
