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
