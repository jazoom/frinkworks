---
name: debugging
description: Diagnose a reproducible failure and identify its cause before a code change. Use when the user reports an error, regression or unexpected behaviour.
---

# Debugging

## When to use

Use this skill when the user reports an error, regression or unexpected behaviour.

## Instructions

1. Establish the expected behaviour and the observed result.
2. Read the project instructions and any relevant logs.
3. Create the smallest safe reproduction with the available tools.
4. Trace the failure from its first incorrect state.
5. State a hypothesis and the evidence that can disprove it.
6. Change one relevant variable at a time.
7. If the user authorises a fix, make the smallest change that addresses the cause.
8. Repeat the reproduction after the change.
9. Exercise nearby error paths and relevant existing tests.
10. Report the cause, the change and any remaining uncertainty.

Do not expose credentials from logs or configuration.
Do not run destructive experiments against user data.
Follow the project's test policy before you add a regression test.
