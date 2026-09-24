// @vitest-environment happy-dom
import { afterAll, afterEach, expect, test, vi } from "vitest";
import { emitLivePatch, emitRequestSettled } from "hypergraft/browser";

const lifecycle = vi.hoisted(() => ({ stop: () => {} }));
vi.mock("./hypergraft-bootstrap", async () => {
    const { observeIslands } = await import("hypergraft/browser");
    const { initConversation } = await import("./conversation");
    return {
        startApp() {
            lifecycle.stop = observeIslands({ conversation: initConversation });
        },
    };
});
import "./main";

afterEach(async () => {
    document.body.replaceChildren();
    await new Promise((resolve) => setTimeout(resolve, 0));
});
afterAll(() => lifecycle.stop());

test.each(["command", "live"])(
    "%s patches retain the consent block after requested policy restoration",
    async (cause) => {
        document.body.innerHTML = `
            <section id="conversation-detail" data-island="conversation">
                <template data-conversation-state="saved"></template>
                <form id="conversation-settings-form"></form>
                <input form="conversation-settings-form" type="radio" name="host_approval" value="ask-each-time" checked>
                <input form="conversation-settings-form" type="radio" name="host_approval" value="automatic">
                <button id="conversation-host-consent-preview">Review host access</button>
                <p data-consent-review-note hidden></p>
            </section>`;
        await new Promise((resolve) => setTimeout(resolve, 0));
        const root = document.getElementById("conversation-detail")!;
        const original = root.innerHTML;
        const automatic = root.querySelector<HTMLInputElement>(
            '[name="host_approval"][value="automatic"]',
        )!;
        const consent = () =>
            root.querySelector<HTMLButtonElement>(
                "#conversation-host-consent-preview",
            )!;
        automatic.checked = true;
        automatic.dispatchEvent(new Event("input", { bubbles: true }));
        automatic.dispatchEvent(new Event("change", { bubbles: true }));
        expect(consent().disabled).toBe(true);

        root.innerHTML = original;
        if (cause === "command") {
            const form = document.createElement("form");
            form.id = "conversation-model-form";
            emitRequestSettled({
                requestKind: "patch",
                form,
                url: "/conversations/example/settings/model",
                outcome: "applied-patch",
                status: 200,
                targetIds: [root.id],
            });
        } else {
            emitLivePatch({
                form: document.createElement("form"),
                url: "/conversations/example",
                targetIds: [root.id],
            });
        }
        expect(
            root.querySelector<HTMLInputElement>(
                '[name="host_approval"]:checked',
            )!.value,
        ).toBe("automatic");
        expect(consent().disabled).toBe(true);
        expect(
            root.querySelector<HTMLElement>("[data-consent-review-note]")!
                .hidden,
        ).toBe(false);

        const current = root.querySelector<HTMLInputElement>(
            '[name="host_approval"][value="ask-each-time"]',
        )!;
        current.checked = true;
        current.dispatchEvent(new Event("input", { bubbles: true }));
        current.dispatchEvent(new Event("change", { bubbles: true }));
        expect(consent().disabled).toBe(false);
    },
);
