# AGENTS.md

## Project overview

Power Plant is a local coding agent. The stack is Axum, Askama, Hypergraft and Rig.

There are no user accounts. A local vault stores provider API keys until the user forgets that provider. A browser session holds the transcript in memory.

## Notes to agent

- You can run `mise` commands. At the end of your work, if you have changed any code, you MUST run `mise run clean`. If there are errors or warnings you MUST fix them, then run `mise run clean` again.
- During alpha, keep each persisted format version at 1.
- Do not add compatibility migrations during alpha.
- Do not use numeronyms, for example "a11y".
- Code is liability. Look for opportunities to simplify or remove code.
- Hypergraft lives in the sibling `../hypergraft` project. Read `../hypergraft/README.md` and treat `../hypergraft/protocol-v1.json` as the canonical protocol fixture.
- The CSP for HTML responses is `script-src 'nonce-…' 'self'` with no `unsafe-inline` or `unsafe-eval`. Put client-side logic in `app/assets/main.ts`.
- Use DaisyUI primitives for controls. Use Tailwind utilities in Askama templates for layout.
- Use vertical slice architecture. Feature code belongs in the relevant slice. Reserve `src/shared_templates/` for shared layouts.
- For ordinary navigation, render a real `href` plus `data-graft`.
- The application requires JavaScript.
- Do not add noscript fallbacks.
- When a `GET` supports page navigation and a targeted fragment update, use one canonical route.
- Use `cargo add` when you add dependencies.
- Do not edit `README.md` unless requested.
- Comment a file, module, function or block only when a later reader could break a why, an invariant, a security contract, a protocol bound or a non-obvious constraint.
- Add tests only as described under Tests.

## Feature structure

A slice owns a user flow. Start each leaf slice with the smallest clear shape.

```text
feature/
  mod.rs
  page.rs
  templates/index.html
```

Presentation belongs in `page.rs`. Derive `Template` on the page model when the template has one natural root model.

Put every unit-test body in a companion `tests.rs` file. For `foo.rs`, use `foo/tests.rs`.

Declare the companion module in `foo.rs`:

```rust
#[cfg(test)]
mod tests;
```

Do not put an inline `mod tests { ... }` block in a production file.

Place local test helper methods and trait implementations into the adjacent `tests.rs` file.

Place shared fixtures into the nearest common `tests.rs` file.

Expose a test module as `pub(crate)` only when sibling test modules require its helpers.

Keep production APIs free from `*_for_test` methods.

## Tests

Tests are liability. Add a test only when it pins an invariant the compiler cannot catch.

Write:

- Security and identity: origin checks, CSP, session extraction, secret redaction, safe local redirects.
- Untrusted input: form validation, normalisation and bounds.
- Hypergraft protocol use in a slice: which representations a route supports.

Do not write:

- Assertions that a template still contains a class or DaisyUI primitive, unless that string is a protocol id or security property.
- Tests of Hypergraft library construction in application slices.
- A `tests.rs` for a module that has no invariant.

## Browser testing

- Development assets use `app/static-development`.
- Production asset builds use `app/static-production`.
- Use the `agent-browser` skill when the change can affect the browser.

## Language conventions

- **Australian English** — use Australian English spelling and grammar.
- Do not use title case for the text of titles, buttons or headings. Only capitalise the first letter of the first word.
