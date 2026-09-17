// @vitest-environment happy-dom
import { expect, test, vi } from "vitest";
import { initObserve } from "./observe";

const configurations = [
    {
        name: "job",
        target: "job-observe",
        unrelated: "transcript",
        action: "/",
    },
    {
        name: "environment preparation",
        target: "environment-preparation",
        unrelated: "environment-form",
        action: "/environments/x/configuration",
    },
    {
        name: "sandbox status",
        target: "sandbox-status",
        unrelated: "composer",
        action: "/projects/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/agents/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    },
] as const;

function observeRoot(target: string, active = true, action = "/"): HTMLElement {
    const root = document.createElement("div");
    root.dataset.observeTarget = target;
    if (active) {
        root.dataset.observeActive = "true";
    }
    root.innerHTML = `<form method="get" action="${action}"><button type="submit">Refresh</button></form>`;
    return root;
}

function countSubmissions(form: HTMLFormElement): { count: number } {
    const state = { count: 0 };
    form.requestSubmit = () => {
        state.count += 1;
    };
    return state;
}

test("an ancestor command patch restarts observation after the unsafe guard releases", () => {
    vi.useFakeTimers();
    const parent = document.createElement("section");
    parent.id = "conversation-detail";
    const root = observeRoot("conversation-observe");
    parent.append(root);
    document.body.append(parent);
    const form = root.querySelector("form")!;
    const submissions = countSubmissions(form);
    const island = initObserve(root);
    vi.advanceTimersByTime(0);
    const patch = (target: string) =>
        island.reconcile?.({
            cause: "patch",
            detail: {
                requestKind: "patch",
                form,
                url: "/conversations/example",
                outcome: "applied-patch",
                status: 200,
                targetIds: [target],
            },
        });
    patch("recent-conversations");
    vi.advanceTimersByTime(0);
    expect(submissions.count).toBe(1);
    patch("conversation-detail");
    expect(submissions.count).toBe(1);
    vi.advanceTimersByTime(0);
    expect(submissions.count).toBe(2);
    const preference = document.createElement("form");
    preference.method = "post";
    for (const outcome of [
        "applied-patch",
        "safe-failure",
        "uncertain-unsafe-result",
    ] as const) {
        island.reconcile?.({
            cause: "patch",
            detail: {
                requestKind: "patch",
                form: preference,
                url: "/thinking-visibility",
                outcome,
                status: 200,
                targetIds: ["thinking-visibility"],
            },
        });
        vi.advanceTimersByTime(0);
    }
    expect(submissions.count).toBe(5);
    root.dataset.observeActive = "false";
    patch("conversation-detail");
    vi.advanceTimersByTime(0);
    expect(submissions.count).toBe(5);
    island.destroy();
    parent.remove();
    vi.useRealTimers();
});

for (const config of configurations) {
    test(`${config.name}: an active island submits after mount`, () => {
        vi.useFakeTimers();
        const root = observeRoot(config.target, true, config.action);
        const form = root.querySelector("form")!;
        const submissions = countSubmissions(form);
        const island = initObserve(root);
        expect(submissions.count).toBe(0);
        vi.advanceTimersByTime(0);
        expect(submissions.count).toBe(1);
        island.destroy();
        vi.useRealTimers();
    });

    test(`${config.name}: an idle island does not submit`, () => {
        vi.useFakeTimers();
        const root = observeRoot(config.target, false, config.action);
        const form = root.querySelector("form")!;
        const submissions = countSubmissions(form);
        const island = initObserve(root);
        vi.advanceTimersByTime(0);
        expect(submissions.count).toBe(0);
        island.reconcile?.({
            cause: "patch",
            detail: {
                requestKind: "patch",
                form,
                url: config.action,
                outcome: "applied-patch",
                status: 200,
                targetIds: [config.target],
            },
        });
        expect(submissions.count).toBe(0);
        island.destroy();
        vi.useRealTimers();
    });

    test(`${config.name}: a relevant applied patch continues after settlement`, () => {
        vi.useFakeTimers();
        const root = observeRoot(config.target, true, config.action);
        const form = root.querySelector("form")!;
        const submissions = countSubmissions(form);
        const island = initObserve(root);
        vi.advanceTimersByTime(0);
        expect(submissions.count).toBe(1);
        island.reconcile?.({
            cause: "patch",
            detail: {
                requestKind: "patch",
                form,
                url: config.action,
                outcome: "applied-patch",
                status: 200,
                targetIds: [config.target],
            },
        });
        expect(submissions.count).toBe(1);
        vi.advanceTimersByTime(0);
        expect(submissions.count).toBe(2);
        island.destroy();
        vi.useRealTimers();
    });

    test(`${config.name}: an insertion patch starts one observation request`, () => {
        vi.useFakeTimers();
        const root = observeRoot(config.target, true, config.action);
        const form = root.querySelector("form")!;
        const submissions = countSubmissions(form);
        const island = initObserve(root);
        island.reconcile?.({
            cause: "patch",
            detail: {
                requestKind: "patch",
                form,
                url: config.action,
                outcome: "applied-patch",
                status: 200,
                targetIds: [config.target],
            },
        });
        expect(submissions.count).toBe(0);
        vi.advanceTimersByTime(0);
        expect(submissions.count).toBe(1);
        island.destroy();
        vi.useRealTimers();
    });

    test(`${config.name}: an unrelated patch does not start an observation request`, () => {
        vi.useFakeTimers();
        const root = observeRoot(config.target, true, config.action);
        const form = root.querySelector("form")!;
        const submissions = countSubmissions(form);
        const island = initObserve(root);
        vi.advanceTimersByTime(0);
        island.reconcile?.({
            cause: "patch",
            detail: {
                requestKind: "patch",
                form,
                url: config.action,
                outcome: "applied-patch",
                status: 200,
                targetIds: [config.unrelated],
            },
        });
        expect(submissions.count).toBe(1);
        island.destroy();
        vi.useRealTimers();
    });

    test(`${config.name}: a safe failure retries from the current form`, () => {
        vi.useFakeTimers();
        const root = observeRoot(config.target, true, config.action);
        const form = root.querySelector("form")!;
        const submissions = countSubmissions(form);
        const island = initObserve(root);
        vi.advanceTimersByTime(0);
        expect(submissions.count).toBe(1);
        island.reconcile?.({
            cause: "patch",
            detail: {
                requestKind: "patch",
                form,
                url: config.action,
                outcome: "safe-failure",
            },
        });
        expect(submissions.count).toBe(1);
        vi.advanceTimersByTime(1000);
        expect(submissions.count).toBe(2);
        island.destroy();
        vi.useRealTimers();
    });

    test(`${config.name}: teardown clears the retry timer`, () => {
        vi.useFakeTimers();
        const root = observeRoot(config.target, true, config.action);
        const form = root.querySelector("form")!;
        const submissions = countSubmissions(form);
        const island = initObserve(root);
        island.reconcile?.({
            cause: "patch",
            detail: {
                requestKind: "patch",
                form,
                url: config.action,
                outcome: "safe-failure",
            },
        });
        island.destroy();
        vi.advanceTimersByTime(1000);
        expect(submissions.count).toBe(0);
        vi.useRealTimers();
    });
}
