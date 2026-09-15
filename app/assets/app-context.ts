import type { IslandInstance } from "hypergraft/browser";

type SectionContext = {
    group: string;
    label: string;
    href: string;
};

type Breadcrumb = {
    label: string;
    href: string;
    current: boolean;
};

const breadcrumbSignatures = new WeakMap<HTMLOListElement, string>();

const SECTIONS: Record<string, SectionContext> = {
    conversations: {
        group: "Work",
        label: "Conversations",
        href: "/conversations",
    },
    projects: { group: "Resources", label: "Projects", href: "/projects" },
    runs: { group: "Work", label: "History", href: "/runs" },
    attention: {
        group: "Work",
        label: "Needs your attention",
        href: "/attention",
    },
    presets: { group: "Resources", label: "Presets", href: "/presets" },
    agents: { group: "Resources", label: "Agents", href: "/agents" },
    workflows: {
        group: "Resources",
        label: "Workflows",
        href: "/workflows",
    },
    environments: {
        group: "Resources",
        label: "Environments",
        href: "/environments",
    },
    providers: { group: "Resources", label: "Providers", href: "/connect" },
    settings: { group: "System", label: "Settings", href: "/settings" },
};

function text(element: Element | null): string {
    return element?.textContent?.replace(/\s+/g, " ").trim() ?? "";
}

function documentLabel(): string {
    return document.title.replace(/\s*\|\s*Power Plant$/, "").trim();
}

function appendCrumb(
    list: HTMLOListElement,
    label: string,
    href: string,
    current: boolean,
): void {
    const item = document.createElement("li");
    if (current) item.className = "app-context-current";

    if (href) {
        const link = document.createElement("a");
        link.href = href;
        link.dataset.graft = "";
        link.textContent = label;
        item.append(link);
    } else {
        const value = document.createElement("span");
        value.textContent = label;
        if (current) value.setAttribute("aria-current", "page");
        item.append(value);
    }
    list.append(item);
}

function pageBreadcrumbs(page: HTMLElement): Breadcrumb[] {
    const section = SECTIONS[page.dataset.section ?? ""];
    const current = text(page.querySelector("h1")) || documentLabel();
    const leaf = page.dataset.contextLeaf?.trim() ?? "";
    const leafPrefix = page.dataset.contextLeafPrefix?.trim() ?? "";
    const leafLabel = leaf && leafPrefix ? `${leafPrefix}: ${leaf}` : leaf;
    const currentHref = page.dataset.contextCurrentHref?.trim() ?? "";
    if (!section) return [{ label: current, href: "", current: true }];

    const hasCurrent =
        page.hasAttribute("data-context-detail") ||
        currentHref !== "" ||
        current.toLocaleLowerCase() !== section.label.toLocaleLowerCase();
    const breadcrumbs = [
        { label: section.group, href: "", current: false },
        {
            label: section.label,
            href: hasCurrent || leaf ? section.href : "",
            current: !hasCurrent && !leaf,
        },
    ];
    if (hasCurrent) {
        breadcrumbs.push({
            label: current,
            href: leaf ? currentHref : "",
            current: !leaf,
        });
    }
    if (leaf) breadcrumbs.push({ label: leafLabel, href: "", current: true });
    return breadcrumbs;
}

function syncBreadcrumbs(root: HTMLElement, page: HTMLElement): void {
    const list = root.querySelector<HTMLOListElement>(
        "[data-app-context-breadcrumbs]",
    );
    if (!list) return;

    const breadcrumbs = pageBreadcrumbs(page);
    const signature = JSON.stringify(breadcrumbs);
    if (breadcrumbSignatures.get(list) === signature) return;

    list.replaceChildren();
    for (const breadcrumb of breadcrumbs) {
        appendCrumb(
            list,
            breadcrumb.label,
            breadcrumb.href,
            breadcrumb.current,
        );
    }
    breadcrumbSignatures.set(list, signature);
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
    syncBreadcrumbs(root, page);
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
