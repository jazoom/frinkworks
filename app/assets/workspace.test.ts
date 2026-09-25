// @vitest-environment happy-dom
import { afterEach, expect, test, vi } from "vitest";
import { initWorkspace } from "./workspace";

afterEach(() => {
    document.body.replaceChildren();
    history.replaceState(null, "", "/");
    vi.unstubAllGlobals();
});

function workOpen(root: HTMLElement) {
    return (
        root
            .querySelector("#conversation-detail")
            ?.hasAttribute("data-work-open") === true
    );
}

test.each(["activity", "workflow"])(
    "a %s companion excludes mobile conversation controls and releases them after a command patch",
    (kind) => {
        vi.stubGlobal("matchMedia", () => ({
            matches: true,
            addEventListener() {},
        }));
        const root = document.createElement("div");
        root.innerHTML = `<section id="conversation-detail"><a data-work-toggle href="/conversations/example/activity">Activity</a><a data-workflow-toggle href="/conversations/example/workflow">Run a workflow</a><section id="transcript"></section><section id="conversation-composer-dock"></section><aside id="conversation-work" data-work-active="true"><section id="${kind}-detail"></section><button data-work-close>Close</button></aside></section>`;
        document.body.append(root);
        const controller = new AbortController();
        const island = initWorkspace(root, {
            signal: controller.signal,
        } as Parameters<typeof initWorkspace>[1]);
        const transcript = root.querySelector<HTMLElement>("#transcript")!;
        expect(transcript.inert).toBe(false);
        island.reconcile?.({
            cause: "location",
            detail: {
                url: `/conversations/example/${kind}`,
                cause: "link-navigation",
            },
        });
        expect(transcript.inert).toBe(true);
        root.querySelector<HTMLButtonElement>("[data-work-close]")!.click();
        expect(transcript.inert).toBe(false);
        expect(document.activeElement).toBe(
            root.querySelector(
                kind === "workflow"
                    ? "[data-workflow-toggle]"
                    : "[data-work-toggle]",
            ),
        );
        root.querySelector(`#${kind}-detail`)!.remove();
        root.querySelector<HTMLElement>(
            "#conversation-work",
        )!.dataset.workActive = "false";
        island.reconcile?.(
            {} as Parameters<NonNullable<typeof island.reconcile>>[0],
        );
        expect(transcript.inert).toBe(false);
        island.destroy?.();
        controller.abort();
    },
);

test.each(["activity", "workflow"])(
    "explicit %s navigation reopens a closed retained companion, unlike patches",
    (kind) => {
        vi.stubGlobal("matchMedia", () => ({
            matches: true,
            addEventListener() {},
        }));
        const root = document.createElement("div");
        root.innerHTML = `<section id="conversation-detail"><section id="transcript"></section><aside id="conversation-work" data-work-active="true"><section id="activity-detail"></section><button data-work-close>Close</button></aside></section>`;
        document.body.append(root);
        const controller = new AbortController();
        const island = initWorkspace(root, { signal: controller.signal });
        const work = root.querySelector<HTMLElement>("#conversation-work")!;
        root.querySelector<HTMLButtonElement>("[data-work-close]")!.click();
        const form = document.createElement("form");
        island.reconcile?.({
            cause: "live-patch",
            detail: { form, url: "/live", targetIds: ["conversation-detail"] },
        });
        island.reconcile?.({
            cause: "patch",
            detail: {
                form,
                url: "/command",
                requestKind: "patch",
                outcome: "applied-patch",
                status: 200,
                targetIds: ["conversation-detail"],
            },
        });
        island.reconcile?.({
            cause: "location",
            detail: {
                url: "/conversations/one/activity",
                cause: "command-patch-replacement",
            },
        });
        island.reconcile?.({
            cause: "location",
            detail: {
                url: "/conversations/one?title=true",
                cause: "get-form-replacement",
            },
        });
        expect(workOpen(root)).toBe(false);
        work.querySelector("section")!.id = `${kind}-detail`;
        island.reconcile?.({
            cause: "location",
            detail: {
                url: `/conversations/one/${kind}`,
                cause: "link-navigation",
            },
        });
        expect(workOpen(root)).toBe(true);
        expect(root.querySelector<HTMLElement>("#transcript")!.inert).toBe(
            true,
        );
        island.destroy();
        controller.abort();
    },
);

test.each([false, true])(
    "current work navigation opens after a companion replacement (new detail: %s)",
    (replaceDetail) => {
        vi.stubGlobal("matchMedia", () => ({
            matches: false,
            addEventListener() {},
        }));
        const root = document.createElement("div");
        root.innerHTML = `<section id="conversation-detail"><a data-graft data-work-toggle href="/conversations/one?work=true">Current work</a><aside id="conversation-work" data-work-active="true"><section id="workflow-detail"></section><button data-work-close>Close</button></aside></section>`;
        document.body.append(root);
        const controller = new AbortController();
        const island = initWorkspace(root, { signal: controller.signal });
        root.querySelector<HTMLButtonElement>("[data-work-close]")!.click();
        const link =
            root.querySelector<HTMLAnchorElement>("[data-work-toggle]")!;
        const click = new MouseEvent("click", {
            bubbles: true,
            cancelable: true,
        });
        link.dispatchEvent(click);
        expect(click.defaultPrevented).toBe(false);
        expect(workOpen(root)).toBe(false);
        if (replaceDetail) {
            root.innerHTML = `<section id="conversation-detail"><button data-work-toggle>Current work</button><aside id="conversation-work" data-work-active="false"></aside></section>`;
        } else {
            root.querySelector("#workflow-detail")!.remove();
            root.querySelector<HTMLElement>(
                "#conversation-work",
            )!.dataset.workActive = "false";
        }
        island.reconcile?.({
            cause: "location",
            detail: {
                url: "/conversations/one?work=true",
                cause: "link-navigation",
            },
        });
        expect(workOpen(root)).toBe(true);
        island.reconcile?.({
            cause: "live-patch",
            detail: {
                form: document.createElement("form"),
                url: "/live",
                targetIds: ["conversation-detail"],
            },
        });
        expect(workOpen(root)).toBe(true);
        island.destroy();
        controller.abort();
    },
);

test("a mobile resize moves focus out of the excluded composer and keeps Escape available", () => {
    let resize = () => {};
    const mobile = {
        matches: false,
        addEventListener: (_: string, listener: () => void) => {
            resize = listener;
        },
    };
    vi.stubGlobal("matchMedia", () => mobile);
    const root = document.createElement("div");
    root.innerHTML = `<section id="conversation-detail"><button data-work-toggle>Current work</button><section id="conversation-composer-dock"><textarea></textarea></section><aside id="conversation-work" tabindex="-1"></aside></section>`;
    document.body.append(root);
    const controller = new AbortController();
    const island = initWorkspace(root, { signal: controller.signal });
    const toggle = root.querySelector<HTMLButtonElement>("[data-work-toggle]")!;
    toggle.click();
    root.querySelector("textarea")!.focus();
    mobile.matches = true;
    resize();
    expect(
        root.querySelector<HTMLElement>("#conversation-composer-dock")!.inert,
    ).toBe(true);
    expect(document.activeElement?.id).toBe("conversation-work");
    document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    expect(workOpen(root)).toBe(false);
    expect(document.activeElement).toBe(toggle);
    expect(
        root.querySelector<HTMLElement>("#conversation-composer-dock")!.inert,
    ).toBe(false);
    island.destroy();
    controller.abort();
});

test("Escape closes navigation before the companion and restores the menu trigger", () => {
    vi.stubGlobal("matchMedia", () => ({
        matches: true,
        addEventListener() {},
    }));
    const root = document.createElement("div");
    root.innerHTML = `<button data-workspace-menu aria-expanded="false">Menu</button><aside id="workspace-index"><a href="/workflows">Workflows</a></aside><section id="conversation-detail"><section id="transcript"></section><aside id="conversation-work" data-work-active="true"></aside></section>`;
    document.body.append(root);
    const controller = new AbortController();
    const island = initWorkspace(root, {
        signal: controller.signal,
    } as Parameters<typeof initWorkspace>[1]);
    island.reconcile?.({
        cause: "location",
        detail: {
            url: "/conversations/one?work=true",
            cause: "link-navigation",
        },
    });
    const menu = root.querySelector<HTMLButtonElement>(
        "[data-workspace-menu]",
    )!;
    menu.click();
    root.querySelector<HTMLAnchorElement>("a")!.focus();
    root.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    expect(menu.getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(menu);
    expect(root.querySelector<HTMLElement>("#transcript")!.inert).toBe(true);
    expect(workOpen(root)).toBe(true);
    island.destroy?.();
    controller.abort();
});

test("revision Cancel closes the inline form and returns focus to Request changes", () => {
    vi.stubGlobal("matchMedia", () => ({
        matches: false,
        addEventListener() {},
    }));
    const root = document.createElement("div");
    root.innerHTML = `<section id="conversation-detail"><section id="transcript"></section><aside id="conversation-work" data-work-active="true"><details class="workspace-revision" open><summary>Request changes</summary><form><textarea name="note"></textarea><button type="button" data-revision-cancel>Cancel</button></form></details></aside></section>`;
    document.body.append(root);
    const controller = new AbortController();
    const island = initWorkspace(root, {
        signal: controller.signal,
    } as Parameters<typeof initWorkspace>[1]);
    island.reconcile?.({
        cause: "location",
        detail: {
            url: "/conversations/one?work=true",
            cause: "link-navigation",
        },
    });
    const disclosure = root.querySelector<HTMLDetailsElement>(
        "details.workspace-revision",
    )!;
    root.querySelector<HTMLButtonElement>("[data-revision-cancel]")!.click();
    expect(disclosure.open).toBe(false);
    expect(document.activeElement).toBe(disclosure.querySelector("summary"));
    island.destroy?.();
    controller.abort();
});

test.each([
    "/runs/run/gates/gate/cancel",
    "/conversations/one/runs/run/settle-partial",
])("a settled decision at %s retains mobile focus after removal", (url) => {
    const { root, island, controller } = mountSidebar(
        true,
        `<section id="conversation-detail"><button data-work-toggle>Current work</button><section id="conversation-composer-dock"></section><aside id="conversation-work" tabindex="-1"><form><button>Discard</button></form></aside></section>`,
    );
    root.querySelector<HTMLButtonElement>("[data-work-toggle]")!.click();
    const form = root.querySelector("form")!;
    form.querySelector("button")!.focus();
    form.remove();
    island.reconcile?.({
        cause: "patch",
        detail: {
            requestKind: "patch",
            outcome: "applied-patch",
            status: 200,
            form,
            url,
            targetIds: ["conversation-detail"],
        },
    });
    const work = root.querySelector<HTMLElement>("#conversation-work")!;
    expect(document.activeElement).toBe(work);
    work.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    expect(
        root.querySelector<HTMLElement>("#conversation-composer-dock")!.inert,
    ).toBe(false);
    expect(document.activeElement).toBe(
        root.querySelector("[data-work-toggle]"),
    );
    island.destroy?.();
    controller.abort();
});

test("a rejected command exposes its focused error outside the mobile companion", () => {
    const { root, island, controller } = mountSidebar(
        true,
        `<section id="conversation-detail"><button data-work-toggle>Current work</button><section id="conversation-composer-dock"></section><div id="conversation-error" tabindex="-1">The application changed.</div><aside id="conversation-work" tabindex="-1"><form><button>End task</button></form></aside></section>`,
    );
    root.querySelector<HTMLButtonElement>("[data-work-toggle]")!.click();
    const error = root.querySelector<HTMLElement>("#conversation-error")!;
    error.focus();
    island.reconcile?.({
        cause: "patch",
        detail: {
            requestKind: "patch",
            outcome: "applied-patch",
            status: 409,
            form: root.querySelector("form")!,
            url: "/conversations/one/runs/run/settle-partial",
            targetIds: ["conversation-detail"],
        },
    });
    expect(document.activeElement).toBe(error);
    expect(root.querySelector<HTMLElement>("#conversation-work")!.inert).toBe(
        true,
    );
    expect(
        root.querySelector<HTMLElement>("#conversation-composer-dock")!.inert,
    ).toBe(false);
    island.destroy?.();
    controller.abort();
});

function mountSidebar(matches: boolean, body: string) {
    vi.stubGlobal("matchMedia", () => ({
        matches,
        addEventListener() {},
    }));
    const skip = document.createElement("a");
    skip.id = "skip-link";
    skip.href = "#chat-main";
    skip.textContent = "Skip to main content";
    document.body.append(skip);
    const root = document.createElement("div");
    root.innerHTML = body;
    document.body.append(root);
    const controller = new AbortController();
    const island = initWorkspace(root, {
        signal: controller.signal,
    } as Parameters<typeof initWorkspace>[1]);
    return { root, skip, island, controller };
}

const sidebarBody = `<form id="recent-filter-form" method="get" action="/conversations" data-graft><label for="recent-filter-input">Find a conversation<input id="recent-filter-input" type="search" autocomplete="off" /></label><a href="/conversations" data-graft>All conversations</a></form><div id="recent-conversations"><ul class="recent-list"><li><a href="/conversations/one" data-graft data-recent-conversation><strong>Quarterly planning</strong><span class="recent-meta">Ready</span></a></li><li><a href="/conversations/two" data-graft data-recent-conversation><strong>&lt;script&gt;alert(1)&lt;/script&gt;</strong><span class="recent-meta">Draft</span></a></li></ul></div><section id="conversation-detail"><section id="transcript"></section><aside id="conversation-work" data-work-active="false" hidden></aside></section>`;

test("sidebar search filters recent titles as plain text and survives replacement", () => {
    const { root, island, controller } = mountSidebar(false, sidebarBody);
    const input = root.querySelector<HTMLInputElement>("#recent-filter-input")!;
    input.value = "<script>alert(1)";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    const links = Array.from(
        root.querySelectorAll<HTMLElement>("[data-recent-conversation]"),
    );
    expect(links[0].closest("li")?.hasAttribute("hidden")).toBe(true);
    expect(links[1].closest("li")?.hasAttribute("hidden")).toBe(false);
    expect(root.querySelector("script")).toBeNull();
    input.value = "no such title";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(root.querySelector("[data-recent-no-match]")?.textContent).toBe(
        "No conversations match.",
    );
    // A live replacement rebuilds the list. The filter stays applied.
    root.querySelector("#recent-conversations")!.innerHTML =
        `<ul class="recent-list"><li><a href="/conversations/one" data-graft data-recent-conversation><strong>Quarterly planning</strong><span class="recent-meta">Ready</span></a></li></ul>`;
    island.reconcile?.(
        {} as Parameters<NonNullable<typeof island.reconcile>>[0],
    );
    const replaced = root.querySelector<HTMLElement>(
        "[data-recent-conversation]",
    )!;
    expect(replaced.closest("li")?.hasAttribute("hidden")).toBe(true);
    expect(root.querySelector("[data-recent-no-match]")?.textContent).toBe(
        "No conversations match.",
    );
    island.destroy?.();
    controller.abort();
});

test("skip link targets the transcript and the open mobile companion", () => {
    const { root, skip, island, controller } = mountSidebar(false, sidebarBody);
    expect(skip.textContent).toBe("Skip to conversation");
    expect(skip.getAttribute("href")).toBe("#transcript");
    island.destroy?.();
    controller.abort();
    document.body.replaceChildren();
    const mobile = mountSidebar(true, sidebarBody);
    const companion =
        mobile.root.querySelector<HTMLElement>("#conversation-work")!;
    companion.dataset.workActive = "true";
    mobile.island.reconcile?.({
        cause: "location",
        detail: {
            url: "/conversations/one?work=true",
            cause: "link-navigation",
        },
    } as Parameters<NonNullable<typeof mobile.island.reconcile>>[0]);
    expect(workOpen(mobile.root)).toBe(true);
    expect(mobile.skip.getAttribute("href")).toBe("#conversation-work");
    mobile.island.destroy?.();
    mobile.controller.abort();
    root.remove();
});

test("skip link keeps the main content destination on catalogue pages", () => {
    const { skip, island, controller } = mountSidebar(
        false,
        `<div id="recent-conversations"><ul class="recent-list"></ul></div>`,
    );
    expect(skip.textContent).toBe("Skip to main content");
    expect(skip.getAttribute("href")).toBe("#chat-main");
    island.destroy?.();
    controller.abort();
});

test("every menu trigger shares open state and Escape restores its trigger", () => {
    const { root, island, controller } = mountSidebar(
        true,
        `<button data-workspace-menu aria-expanded="false">Menu</button><aside id="workspace-index"></aside><section id="conversation-detail"><button data-workspace-menu aria-expanded="false">Menu</button><section id="transcript"></section><aside id="conversation-work" data-work-active="false" hidden></aside></section>`,
    );
    const triggers = Array.from(
        root.querySelectorAll<HTMLButtonElement>("[data-workspace-menu]"),
    );
    triggers[1].click();
    expect(triggers[0].getAttribute("aria-expanded")).toBe("true");
    expect(triggers[1].getAttribute("aria-expanded")).toBe("true");
    root.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    expect(triggers[0].getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(triggers[1]);
    island.destroy?.();
    controller.abort();
});
