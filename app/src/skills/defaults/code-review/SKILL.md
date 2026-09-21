---
name: code-review
description: Review proposed code changes for defects, security risks and unnecessary complexity. Use when the user asks for a code review.
---

# Code review

## When to use

Use this skill when the user asks for a code review.

## Instructions

1. Read the project instructions and the requested scope.
2. Inspect the complete diff, including staged changes when relevant.
3. Read the surrounding code and trace the affected callers.
4. Examine input validation, authority boundaries, error paths and data integrity.
5. Reproduce suspected defects with the available tools when practical.
6. Distinguish confirmed defects from questions and assumptions.
7. Report actionable findings with severity, file paths and line numbers.
8. Explain each finding's trigger and consequence.
9. State which tests ran and which checks remain incomplete.

Do not change files unless the user requests changes.
Do not invent findings to fill a quota.
Follow the project's test policy.
