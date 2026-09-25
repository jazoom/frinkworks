import type { IslandInstance, IslandMountContext } from "hypergraft/browser";

export function initWorkspace(
    root: HTMLElement,
    { signal }: IslandMountContext,
): IslandInstance {
    const mobile = matchMedia("(max-width: 700px)");
    let conversation: Element | null = null;
    let workOpen = false;
    let expanded = false;
    let returnFocus: HTMLElement | null = null;
    let previousCompanion = false;
    let setupReturnFocus: HTMLElement | null = null;
    let menuTrigger: HTMLElement | null = null;
    let header: HTMLElement | null = null;
    const positionPanels = () => {
        if (header)
            root.style.setProperty(
                "--workspace-panel-top",
                `${header.getBoundingClientRect().bottom}px`,
            );
    };
    const headerResize = new ResizeObserver(positionPanels);

    function sync(openWork = false) {
        root.querySelectorAll<HTMLElement>(
            ".prose pre, .chat-prose pre",
        ).forEach((pre) => {
            pre.tabIndex = 0;
            pre.setAttribute("role", "region");
            pre.setAttribute("aria-label", "Code block");
        });
        const detail = root.querySelector<HTMLElement>("#conversation-detail");
        const nextHeader =
            detail?.querySelector<HTMLElement>(":scope > header") ?? null;
        if (nextHeader !== header) {
            headerResize.disconnect();
            header = nextHeader;
            if (header) headerResize.observe(header);
        }
        positionPanels();
        const work = root.querySelector<HTMLElement>("#conversation-work");
        if (detail !== conversation) {
            conversation = detail;
            workOpen = false;
            expanded = false;
        }
        const companion = !!root.querySelector(
            "#workflow-detail, #activity-detail, #tree-detail",
        );
        if (previousCompanion && !companion) workOpen = false;
        previousCompanion = companion;
        if (openWork) workOpen = true;
        detail?.toggleAttribute("data-work-open", workOpen);
        if (work) {
            work.setAttribute("aria-hidden", String(!workOpen));
        }
        // The review strip only applies while Current work stays closed.
        root.querySelectorAll<HTMLElement>("[data-attention-strip]").forEach(
            (strip) => {
                strip.hidden = workOpen;
            },
        );
        const setup = root.querySelector(
            "#conversation-settings:popover-open, #conversation-actions:popover-open",
        );
        if (work) work.inert = !workOpen || !!setup;
        detail?.toggleAttribute(
            "data-review-expanded",
            expanded && workOpen && !setup,
        );
        const excludeConversation =
            (mobile.matches && (workOpen || !!setup)) || expanded;
        for (const selector of [
            "#transcript",
            "#conversation-composer-dock",
            ".workspace-effective",
            ".workspace-toolbar",
        ]) {
            const element = detail?.querySelector<HTMLElement>(selector);
            if (element) element.inert = excludeConversation;
        }
        root.querySelectorAll("[data-work-toggle]").forEach((toggle) => {
            toggle.setAttribute("aria-expanded", String(workOpen));
        });
        root.querySelectorAll<HTMLAnchorElement>(
            "[data-recent-conversation]",
        ).forEach((link) => {
            const owner = root.querySelector<HTMLElement>(
                "[data-conversation-url]",
            )?.dataset.conversationUrl;
            if (link.pathname === (owner || location.pathname))
                link.setAttribute("aria-current", "page");
            else link.removeAttribute("aria-current");
        });
        const section = root.querySelector<HTMLElement>(
            "#chat-main > [data-section]",
        )?.dataset.section;
        root.querySelectorAll<HTMLAnchorElement>(
            ".workspace-navigation > a, .workspace-resources a, .workspace-local a",
        ).forEach((link) => {
            if (
                section &&
                (link.pathname === `/${section}` ||
                    link.dataset.nav === section)
            )
                link.setAttribute("aria-current", "page");
            else link.removeAttribute("aria-current");
        });
        applyRecentFilter();
        syncAttentionLink();
        syncSkipLink();
        syncExpandControls();
        if (!mobile.matches) setMenuOpen(false);
        const menuOpen = !!root.querySelector(
            "#workspace-index[data-menu-open]",
        );
        root.querySelectorAll<HTMLElement>(".app-file").forEach((page) => {
            page.inert = menuOpen;
        });
    }

    function recentFilterQuery(): string {
        return (
            root
                .querySelector<HTMLInputElement>("#recent-filter-input")
                ?.value.trim()
                .toLowerCase() ?? ""
        );
    }

    // The sidebar input filters the bounded recent list without navigation.
    // Untrusted titles stay inert because matching reads text, never markup.
    function applyRecentFilter() {
        const query = recentFilterQuery();
        const container = root.querySelector<HTMLElement>(
            "#recent-conversations",
        );
        if (!container) return;
        const items = container.querySelectorAll<HTMLElement>(
            "[data-recent-conversation]",
        );
        let visible = 0;
        items.forEach((link) => {
            const title =
                link.querySelector("strong")?.textContent?.toLowerCase() ??
                link.textContent?.toLowerCase() ??
                "";
            const match = query === "" || title.includes(query);
            link.closest("li")?.toggleAttribute("hidden", !match);
            if (match) visible += 1;
        });
        let notice = container.querySelector<HTMLElement>(
            "[data-recent-no-match]",
        );
        if (query !== "" && visible === 0 && items.length > 0) {
            if (!notice) {
                notice = document.createElement("p");
                notice.className = "recent-empty";
                notice.dataset.recentNoMatch = "";
                notice.textContent = "No conversations match.";
                container.append(notice);
            }
            notice.hidden = false;
        } else if (notice) notice.hidden = true;
    }

    // The sidebar keeps native catalogue fallbacks.
    // On a conversation those links carry that record so the destination can
    // return without inferring an identity.
    function syncAttentionLink() {
        const owner = root
            .querySelector<HTMLElement>("[data-conversation-url]")
            ?.dataset.conversationUrl?.trim();
        const id = owner?.startsWith("/conversations/")
            ? owner.slice("/conversations/".length).split("/")[0]
            : "";
        const attention = root.querySelector<HTMLAnchorElement>(
            "[data-attention-link]",
        );
        attention?.setAttribute(
            "href",
            id
                ? `/attention?conversation=${encodeURIComponent(id)}`
                : "/attention",
        );
        for (const section of ["workflows", "presets"]) {
            root.querySelector<HTMLAnchorElement>(
                `[data-${section}-link]`,
            )?.setAttribute(
                "href",
                id
                    ? `/${section}?conversation=${encodeURIComponent(id)}`
                    : `/${section}`,
            );
        }
        const history = root.querySelector<HTMLAnchorElement>(
            "[data-history-link]",
        );
        history?.setAttribute(
            "href",
            id
                ? `/conversations?conversation=${encodeURIComponent(id)}`
                : "/conversations",
        );
    }

    function syncSkipLink() {
        const skip = document.querySelector<HTMLAnchorElement>("#skip-link");
        if (!skip) return;
        const detail = root.querySelector("#conversation-detail");
        if (!detail) {
            skip.textContent = "Skip to main content";
            skip.setAttribute("href", "#chat-main");
            return;
        }
        skip.textContent = "Skip to conversation";
        const companion = root.querySelector<HTMLElement>("#conversation-work");
        const companionVisible =
            !!companion &&
            detail.hasAttribute("data-work-open") &&
            mobile.matches;
        if (companionVisible) skip.setAttribute("href", "#conversation-work");
        else if (root.querySelector("#transcript"))
            skip.setAttribute("href", "#transcript");
        else skip.setAttribute("href", "#conversation-detail");
    }

    function setMenuOpen(open: boolean) {
        const menu = root.querySelector<HTMLElement>("#workspace-index");
        const wasOpen = menu?.hasAttribute("data-menu-open");
        if (open) menu?.setAttribute("data-menu-open", "");
        else menu?.removeAttribute("data-menu-open");
        root.querySelectorAll("[data-workspace-menu]").forEach((trigger) => {
            trigger.setAttribute("aria-expanded", String(open));
        });
        // The open navigation is a modal dialog: the page behind it stays
        // visible through the dimmed backdrop but never interactive.
        root.querySelectorAll<HTMLElement>(".app-file").forEach((page) => {
            page.inert = open;
        });
        if (open) {
            menu?.querySelector<HTMLElement>(".workspace-menu-close")?.focus();
        } else if (wasOpen && mobile.matches) {
            menuTrigger?.focus();
        }
    }

    function syncExpandControls() {
        const label = expanded ? "Restore conversation" : "Expand review";
        root.querySelectorAll<HTMLElement>("[data-expand-review]").forEach(
            (control) => {
                control.setAttribute("aria-expanded", String(expanded));
                control.setAttribute("aria-label", label);
                control.setAttribute("title", label);
                // Icon buttons keep their icon; text controls keep a label.
                if ((control.textContent ?? "").trim() !== "")
                    control.textContent = label;
            },
        );
    }

    function continueConversation() {
        workOpen = false;
        expanded = false;
        sync();
        root.querySelector<HTMLElement>("#composer-message")?.focus();
    }

    function showActionsPanel(name: string | null) {
        const dialog = root.querySelector<HTMLElement>("#conversation-actions");
        if (!dialog) return;
        if (name) {
            // Reveal before focusing: the menu button loses visibility in
            // the same handler, and the click default action can otherwise
            // return focus to that hidden button and drop it to the body.
            const panel = dialog.querySelector<HTMLElement>(
                `[data-actions-panel="${name}"]`,
            );
            if (panel) panel.hidden = false;
            // Destructive confirmation keeps focus on its safe exit, not
            // the confirming submitter.
            const field = panel?.querySelector<HTMLElement>(
                "input:not([type=hidden]), [data-actions-back]",
            );
            field?.focus();
            // Reassert after the click settles in case the mouse default
            // action moved focus back to the hidden menu button first.
            setTimeout(() => {
                if (
                    field?.isConnected &&
                    (document.activeElement === document.body ||
                        dialog.contains(document.activeElement) === false)
                )
                    field?.focus();
            }, 0);
        }
        dialog
            .querySelectorAll<HTMLElement>("[data-actions-panel]")
            .forEach((panel) => {
                panel.hidden = panel.dataset.actionsPanel !== name;
            });
        dialog
            .querySelector<HTMLElement>("[data-actions-menu]")
            ?.toggleAttribute("hidden", name !== null);
    }

    function closeWork() {
        workOpen = false;
        expanded = false;
        sync();
        const destination = returnFocus?.isConnected
            ? returnFocus
            : root.querySelector<HTMLElement>(
                  root.querySelector("#tree-detail")
                      ? "[data-tree-toggle]"
                      : root.querySelector("#workflow-detail")
                        ? "[data-workflow-toggle]"
                        : "[data-work-toggle]",
              );
        destination?.focus();
    }

    root.addEventListener(
        "click",
        (event) => {
            const target =
                event.target instanceof Element ? event.target : null;
            const setupTrigger = target?.closest<HTMLElement>(
                '[popovertarget="conversation-settings"]',
            );
            if (
                setupTrigger &&
                setupTrigger.getAttribute("popovertargetaction") !== "hide"
            )
                setupReturnFocus = setupTrigger;
            if (target?.closest("button[data-work-toggle]")) {
                returnFocus = target.closest<HTMLElement>("[data-work-toggle]");
                workOpen = !workOpen;
                root.querySelector<HTMLElement>(
                    "#conversation-settings:popover-open, #conversation-actions:popover-open",
                )?.hidePopover();
                sync();
                if (workOpen)
                    root.querySelector<HTMLElement>(
                        "#conversation-work",
                    )?.focus({ preventScroll: true });
            } else if (target?.closest("[data-actions-show]")) {
                const trigger = target.closest<HTMLElement>(
                    "[data-actions-show]",
                );
                showActionsPanel(trigger?.dataset.actionsShow ?? null);
            } else if (target?.closest("[data-actions-back]")) {
                const panel = target.closest<HTMLElement>(
                    "[data-actions-panel]",
                );
                showActionsPanel(null);
                panel?.parentElement
                    ?.querySelector<HTMLElement>(
                        `[data-actions-show="${panel.dataset.actionsPanel}"]`,
                    )
                    ?.focus();
            } else if (target?.closest("[data-revision-cancel]")) {
                const disclosure = target.closest<HTMLDetailsElement>(
                    "details.workspace-revision",
                );
                const summary =
                    disclosure?.querySelector<HTMLElement>("summary");
                if (disclosure) disclosure.open = false;
                summary?.focus();
            } else if (target?.closest("[data-work-close]")) closeWork();
            else if (target?.closest("[data-continue-conversation]"))
                continueConversation();
            else if (
                target?.closest("[data-expand-review]") &&
                !event.ctrlKey &&
                !event.metaKey &&
                !event.shiftKey &&
                !event.altKey &&
                event.button === 0
            ) {
                event.preventDefault();
                expanded = !expanded;
                sync();
            }
            const menu = root.querySelector<HTMLElement>("#workspace-index");
            if (target?.closest("[data-workspace-menu]")) {
                const open = !menu?.hasAttribute("data-menu-open");
                if (open)
                    menuTrigger = target.closest<HTMLElement>(
                        "[data-workspace-menu]",
                    );
                setMenuOpen(open);
            } else if (target?.closest("a[data-graft]")) {
                setMenuOpen(false);
            } else if (
                menu?.hasAttribute("data-menu-open") &&
                !target?.closest("#workspace-index")
            ) {
                // The dimmed backdrop is a pseudo-element, so a click on it
                // lands outside the panel. Either way the dialog closes.
                setMenuOpen(false);
            }
        },
        { signal },
    );
    root.addEventListener(
        "keydown",
        (event) => {
            if (
                event.key === "Escape" &&
                !root.querySelector(":popover-open, dialog[open]")
            ) {
                const menu = root.querySelector(
                    "#workspace-index[data-menu-open]",
                );
                if (menu) {
                    setMenuOpen(false);
                } else if (workOpen) closeWork();
            }
        },
        { signal },
    );
    root.addEventListener(
        "toggle",
        (event) => {
            if (
                event.target instanceof HTMLElement &&
                (event.target.id === "conversation-settings" ||
                    event.target.id === "conversation-actions")
            ) {
                if (
                    event.target.id === "conversation-actions" &&
                    !event.target.matches(":popover-open")
                )
                    showActionsPanel(null);
                sync();
                if (
                    !event.target.matches(":popover-open") &&
                    !root.querySelector(
                        "#conversation-settings:popover-open, #conversation-actions:popover-open",
                    )
                ) {
                    const actions = event.target.id === "conversation-actions";
                    const trigger = actions ? null : setupReturnFocus;
                    const fallback = actions
                        ? '[popovertarget="conversation-actions"]'
                        : '[popovertarget="conversation-settings"]';
                    const destination = trigger?.isConnected
                        ? trigger
                        : root.querySelector<HTMLElement>(fallback);
                    destination?.focus();
                }
            }
        },
        { capture: true, signal },
    );
    root.addEventListener(
        "input",
        (event) => {
            if (
                event.target instanceof HTMLInputElement &&
                event.target.id === "recent-filter-input"
            )
                applyRecentFilter();
        },
        { signal },
    );
    mobile.addEventListener(
        "change",
        () => {
            const focus = document.activeElement;
            sync();
            if (
                mobile.matches &&
                workOpen &&
                focus instanceof HTMLElement &&
                focus.closest("[inert]")
            )
                root.querySelector<HTMLElement>("#conversation-work")?.focus({
                    preventScroll: true,
                });
        },
        { signal },
    );
    sync(!!root.querySelector("#tree-detail"));
    return {
        reconcile(context) {
            let openWork = false;
            if (
                context.cause === "location" &&
                context.detail.cause !== "command-patch-replacement" &&
                root.querySelector(
                    "#workflow-detail, #activity-detail, #tree-detail",
                )
            ) {
                const url = new URL(context.detail.url, location.href);
                if (
                    /^\/conversations\/[^/]+\/(?:workflow|activity|tree)$/.test(
                        url.pathname,
                    )
                ) {
                    openWork = true;
                    expanded = false;
                    root.querySelector<HTMLElement>(
                        "#conversation-settings:popover-open, #conversation-actions:popover-open",
                    )?.hidePopover();
                }
            }
            // The mobile companion can cover a focused error after a rejected command.
            // Closure exposes the authoritative error without a replay.
            if (
                context.cause === "patch" &&
                context.detail.outcome === "applied-patch" &&
                mobile.matches &&
                workOpen &&
                document.activeElement?.id === "conversation-error"
            ) {
                workOpen = false;
                expanded = false;
            }
            sync(
                openWork ||
                    (context.cause === "location" &&
                        context.detail.cause !== "command-patch-replacement" &&
                        new URL(
                            context.detail.url,
                            location.href,
                        ).searchParams.get("work") === "true"),
            );
            const work = root.querySelector<HTMLElement>("#conversation-work");
            if (
                context.cause === "patch" &&
                context.detail.outcome === "applied-patch" &&
                context.detail.targetIds.includes("conversation-detail") &&
                workOpen &&
                work &&
                !work.inert &&
                document.activeElement === document.body
            )
                work.focus({ preventScroll: true });
        },
        destroy() {
            headerResize.disconnect();
            root.style.removeProperty("--workspace-panel-top");
            root.querySelectorAll<HTMLElement>("[inert]").forEach((element) => {
                element.inert = false;
            });
        },
    };
}
