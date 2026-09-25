---
version: 1
slug: "route-connect"
primary_target: "route:/connect"
related_targets:
  - "app/src/slices/connect/templates/connection.html"
  - "app/src/shared_templates/layout/connect.html"
---

# Provider connection

## Scope

The visitor mode is Operate. A first-time user chooses a provider before a connection method.

The focused provider chooser has right-aligned method descriptions on wider screens.

## Layout and flow

The standalone page places a compact form below the Frinkworks mark and wordmark. The form has no introductory sidebar or product tagline.

Both versions use the resource page background. In-app Providers uses the shared catalogue header with the storage note beneath its title.

The chooser exposes every available provider. Method descriptions sit below provider names on narrow screens.

A native provider link opens `/connect?provider=…`. The form identifies that provider explicitly. Change provider returns to `/connect`.

Plan-capable providers offer plan login and API key entry. API-only providers show the key field without an introductory sentence.

The storage note reads: “Frinkworks stores your key or plan login on this machine.”

## Constraints

The route supports documents, navigation fragments, and command patches. Submitted forms and pending polls retain the selected provider identity.

Validation errors identify the affected control. The server never echoes an API key. Pending plan sign-in shows the provider URL and device code.

A successful connection leads to conversations. Later provider management retains the shared application shell.

Both layouts share the provider chooser and connection form.

In-app connection controls sit directly on the content background, without a separate panel. Connected providers appear beneath the controls with a separate Forget control for each provider.

The standalone form retains its border. Both versions share the provider controls and command patch content.

The chooser omits connected providers. When every provider is connected, the page retains the connected-provider list without an empty chooser.

System preference uses Springfield for light mode and Sector 7-G for dark mode. A saved explicit theme overrides this default.
