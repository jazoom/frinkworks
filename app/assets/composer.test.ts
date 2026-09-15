// @vitest-environment happy-dom
import { expect, test, vi } from "vitest";
import { initComposer } from "./composer";

function composerForm(): HTMLFormElement {
    const form = document.createElement("form");
    form.innerHTML = `
        <textarea id="composer-message"></textarea>
        <button type="submit" name="mode" value="quick">Send</button>
        <button type="submit" name="mode" value="configured">Send with workflow</button>
    `;
    return form;
}

test("the send shortcut submits Quick task", () => {
    const form = composerForm();
    const textarea = form.querySelector("textarea")!;
    const quick = form.querySelector<HTMLButtonElement>('[value="quick"]')!;
    const requestSubmit = vi.fn();
    form.requestSubmit = requestSubmit;

    initComposer(form, { signal: new AbortController().signal });
    textarea.dispatchEvent(
        new KeyboardEvent("keydown", {
            key: "Enter",
            ctrlKey: true,
            bubbles: true,
            cancelable: true,
        }),
    );

    expect(requestSubmit).toHaveBeenCalledOnce();
    expect(requestSubmit).toHaveBeenCalledWith(quick);
});

test("navigation replaces the draft before a later conversation command patch", () => {
    const form = composerForm();
    const textarea = form.querySelector("textarea")!;
    const controller = new AbortController();
    const island = initComposer(form, { signal: controller.signal });
    textarea.value = "Private draft from the previous conversation";
    textarea.dispatchEvent(new InputEvent("input", { bubbles: true }));
    textarea.value = "";
    island?.reconcile?.({
        cause: "location",
        detail: { url: "/conversations/new", cause: "link-navigation" },
    });
    island?.reconcile?.({
        cause: "patch",
        detail: {
            requestKind: "patch",
            form: document.createElement("form"),
            url: "/conversations/another/settings",
            outcome: "applied-patch",
            status: 422,
            targetIds: ["conversation-detail"],
        },
    });
    expect(textarea.value).toBe("");
    controller.abort();
});

test.each([
    "/conversations/one/handoff",
    "/conversations/one/workflow",
    "/conversations/one",
])("same-conversation navigation to %s retains the draft", (url) => {
    const detail = document.createElement("section");
    detail.id = "conversation-detail";
    const identity = document.createElement("template");
    identity.dataset.conversationUrl = "/conversations/one";
    detail.append(identity);
    const form = composerForm();
    detail.append(form);
    const controller = new AbortController();
    const island = initComposer(form, { signal: controller.signal });
    const textarea = form.querySelector("textarea")!;
    textarea.value = "Unsent private draft";
    textarea.dispatchEvent(new InputEvent("input", { bubbles: true }));
    textarea.value = "";
    island?.reconcile?.({
        cause: "location",
        detail: { url, cause: "link-navigation" },
    });
    expect(textarea.value).toBe("Unsent private draft");
    identity.dataset.conversationUrl = "/conversations/two";
    textarea.value = "";
    island?.reconcile?.({
        cause: "location",
        detail: { url: "/conversations/two", cause: "link-navigation" },
    });
    island?.reconcile?.({
        cause: "live-patch",
        detail: { form, url: "/live", targetIds: ["conversation-detail"] },
    });
    expect(textarea.value).toBe("");
    controller.abort();
});

test("a sandbox projection preserves the unsent message", () => {
    const form = composerForm();
    document.body.append(form);
    const textarea = form.querySelector("textarea")!;
    const island = initComposer(form, {
        signal: new AbortController().signal,
    });
    if (!island) {
        throw new Error("composer island did not mount");
    }
    textarea.value = "Review this draft";
    textarea.focus();
    textarea.setSelectionRange(7, 11);
    textarea.dispatchEvent(new InputEvent("input", { bubbles: true }));

    textarea.value = "";
    textarea.setSelectionRange(0, 0);
    island.reconcile?.({
        cause: "patch",
        detail: {
            requestKind: "patch",
            form: document.createElement("form"),
            url: "/projects/a/agents/b?sandbox=cursor",
            outcome: "applied-patch",
            status: 200,
            targetIds: ["sandbox-status", "composer"],
        },
    });

    expect(textarea.value).toBe("Review this draft");
    expect(textarea.selectionStart).toBe(7);
    expect(textarea.selectionEnd).toBe(11);
    form.remove();
});
