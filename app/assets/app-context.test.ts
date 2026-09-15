// @vitest-environment happy-dom
import { afterEach, expect, test } from "vitest";
import { initAppContext } from "./app-context";

afterEach(() => {
    document.body.replaceChildren();
    document.title = "";
});

function fixture(page: string): HTMLElement {
    document.body.innerHTML = `
        <header data-island="app-context">
            <nav><ol data-app-context-breadcrumbs></ol></nav>
            <div data-app-context-status hidden>
                <a href="/runs" data-app-context-status-link hidden></a>
                <span data-app-context-status-label hidden></span>
            </div>
        </header>
        <div id="chat-main">${page}</div>
    `;
    const root = document.querySelector<HTMLElement>(
        "[data-island='app-context']",
    );
    if (!root) throw new Error("the app context fixture is missing");
    return root;
}

test("the context identifies the conversation work section", () => {
    const root = fixture(`
        <main data-section="conversations"><h1>Discussion</h1></main>
    `);

    initAppContext(root);

    expect(root.querySelector("ol")?.textContent).toContain(
        "WorkConversationsDiscussion",
    );
    const links = [...root.querySelectorAll<HTMLAnchorElement>("ol a")];
    expect(links.map((link) => link.pathname)).toEqual(["/conversations"]);
});

test("the context identifies a nested project desk", () => {
    const root = fixture(`
        <main
            data-section="projects"
            data-context-current-href="/projects/project-1"
            data-context-leaf="Ada"
            data-context-leaf-prefix="Agent"
        >
            <h1>Workshop</h1>
            <span
                hidden
                data-app-context-status-source
                data-state="Active"
                data-href="/runs/run-1"
            >Running agent</span>
        </main>
    `);

    const island = initAppContext(root);

    expect(root.querySelector("ol")?.textContent).toContain(
        "ResourcesProjectsWorkshopAgent: Ada",
    );
    const links = [...root.querySelectorAll<HTMLAnchorElement>("ol a")];
    expect(links.map((link) => link.pathname)).toEqual([
        "/projects",
        "/projects/project-1",
    ]);
    links[0]?.focus();
    island.reconcile?.({} as never);
    expect(document.activeElement).toBe(links[0]);
    const status = root.querySelector<HTMLElement>("[data-app-context-status]");
    const statusLink = root.querySelector<HTMLAnchorElement>(
        "[data-app-context-status-link]",
    );
    expect(status?.hidden).toBe(false);
    expect(status?.dataset.state).toBe("Active");
    expect(statusLink?.pathname).toBe("/runs/run-1");
    expect(statusLink?.textContent).toBe("Running agent");
});

test("the context refreshes after a page patch", () => {
    const root = fixture(`
        <main data-section="projects"><h1>Projects</h1></main>
    `);
    const island = initAppContext(root);
    const chatMain = document.querySelector("#chat-main");
    if (!chatMain) throw new Error("the page fixture is missing");
    chatMain.innerHTML = `
        <main data-section="settings"><h1>Settings</h1></main>
    `;

    island.reconcile?.({} as never);

    expect(root.querySelector("ol")?.textContent).toContain("SystemSettings");
    expect(
        root.querySelector<HTMLElement>("[data-app-context-status]")?.hidden,
    ).toBe(true);
});
