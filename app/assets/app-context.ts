import type { IslandInstance } from "hypergraft/browser";

function text(element: Element | null): string {
    return element?.textContent?.replace(/\s+/g, " ").trim() ?? "";
}

function syncStatus(root: HTMLElement, page: HTMLElement): void {
    const status = root.querySelector<HTMLElement>("[data-app-context-status]");
    const link = root.querySelector<HTMLAnchorElement>(
        "[data-app-context-status-link]",
    );
    const label = root.querySelector<HTMLElement>(
        "[data-app-context-status-label]",
    );
    if (!status || !link || !label) return;

    const source = page.querySelector<HTMLElement>(
        "[data-app-context-status-source]",
    );
    const value = text(source);
    if (!source || !value) {
        status.hidden = true;
        status.removeAttribute("data-state");
        return;
    }

    const href = source.dataset.href?.trim() ?? "";
    status.dataset.state = source.dataset.state?.trim() || value;
    status.hidden = false;
    if (href.startsWith("/")) {
        link.href = href;
        link.textContent = value;
        link.hidden = false;
        label.hidden = true;
        label.textContent = "";
    } else {
        label.textContent = value;
        label.hidden = false;
        link.hidden = true;
        link.textContent = "";
    }
}

function syncAppContext(root: HTMLElement): void {
    const page = document.querySelector<HTMLElement>(
        "#chat-main > [data-section]",
    );
    if (!page) return;
    syncStatus(root, page);
}

export function initAppContext(root: HTMLElement): IslandInstance {
    syncAppContext(root);
    return {
        reconcile() {
            syncAppContext(root);
        },
        destroy() {},
    };
}
