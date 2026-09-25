# Development modes

`mise run d` remains an alias for `dev`. Source changes restart the server. Asset changes reload the browser.

`mise run ds` is an alias for `dev-supervised`. Source changes do not restart the server or replace its assets.

The supervisor owns the public loopback HTTP address. It forwards HTTP and WebSocket traffic to a private application port.

Each explicit build gets a separate executable and asset directory under `.cache`. Ordinary asset builds do not change the active instance.

## Use supervised development

1. Stop the ordinary development server.
2. Run `mise run ds`.
3. Open the configured public URL.
4. Save unsent text before a restart.
5. Select **Rebuild and restart** in the navigation or provider connection page.

The button starts the rebuild immediately. A successful restart reloads all open tabs and clears unsent text.

A spinner appears beside the button label during a rebuild. Progress and errors appear below the button.

## Restart behaviour

The supervisor refuses a restart while commands or agent work remain active. This restriction also covers workflow decisions and environment preparation.

Recovery-only reservations permit a restart. Other active work and unfinished cleanup still block it.

The current server serves pages during a build. The supervisor rejects new commands until the build finishes or fails.

A failed build leaves the current executable and assets intact. The terminal shows build output.

After a successful build, the supervisor stops the old server. It starts the new executable with the matching assets.

The browser reloads only after the new server answers its readiness probe. A startup failure leaves the supervisor available for another explicit rebuild.

The supervisor never restarts a failed process automatically. It requires a loopback HTTP origin and the development environment.

Changes to the launcher or its environment require a new `mise run ds` process. The browser control replaces only the application executable and assets.

## Stop the supervisor

Press `Ctrl+C` in the launcher terminal.

The launcher stops its child processes and removes its temporary executable and asset directories. It retains application data.

## Read/Write validation

The ordinary suites cover permission bounds and persisted identity. The real Microsandbox integration test remains ignored by default.

That test requires these resources:

- Microsandbox on `PATH` and access to `/dev/kvm`.
- An isolated `MSB_HOME`.
- An isolated application data directory with a prepared **Alpine Git** environment.

The test uses a scripted provider and a fake credential. It executes real sandbox commands without hosted-model requests.

### Run the suites

1. Run `mise run test`.
2. Run `mise run clean`.
3. Run `git diff --check`.

### Run the real sandbox test

1. Start an application instance with isolated data and runtime paths.
2. Prepare **Alpine Git** through the Environments page.
3. Stop the isolated application instance.
4. Set `MSB_HOME` to that isolated runtime directory.
5. Set `FRINKWORKS_TEST_DATA_DIR` to that isolated application data directory.
6. Add the Microsandbox binary directory to `PATH`.
7. Run the command below.

```sh
mise exec -- cargo test --lib --all-features \
  real_workflow_commands_enforce_mounts_and_approval_without_capture \
  -- --ignored --nocapture
```

### Browser validation

1. Build development assets with `mise exec -- pnpm vite build app --mode development`.
2. Build the server with `mise exec -- cargo build -p frinkworks --features dev`.
3. Start the rebuilt server with isolated data and runtime paths.
4. Load `agent-browser skills get core`.
5. Open the isolated URL in a named browser session.
6. Exercise Read and Write commands against a temporary authorised directory.
7. Exercise Ask each time and Automatic (YOLO).
8. Save future defaults.
9. Open a new draft.
10. Make sure that the draft retains the requested policy without consent.
11. Exercise desktop and mobile layouts.
12. Inspect browser console messages and page errors.
13. Capture screenshots.
14. Close the named browser session.
15. Stop only the isolated server.

Browser evidence and scripted-provider tests do not establish hosted-model success. Command output is not a filesystem audit.
