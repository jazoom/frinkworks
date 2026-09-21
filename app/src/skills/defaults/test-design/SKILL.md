---
name: test-design
description: Design focused tests for important invariants, input boundaries and failure paths. Use when the user asks for tests or a test plan.
---

# Test design

## When to use

Use this skill when the user asks for tests or a test plan.

## Instructions

1. Read the project's test policy before you propose or add tests.
2. Identify the invariant and the consequence of a regression.
3. Inspect existing tests and shared fixtures for relevant coverage.
4. Choose the smallest test boundary that can observe the failure.
5. Include valid input, boundary values and invalid input where relevant.
6. Exercise authority checks and failure paths when they affect the invariant.
7. Prefer deterministic inputs and explicit outcomes.
8. Keep test setup local and remove unnecessary mocks.
9. If practical, demonstrate that the test fails without the fix.
10. Run the relevant tests and report their actual results.

Do not test implementation details that the compiler already enforces.
Do not add snapshots or assertions merely to preserve presentation details.
Do not claim coverage for cases that the tests do not exercise.
