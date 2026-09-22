// @vitest-environment happy-dom
import { expect, test, vi } from "vitest";
import { classifyComposerInput, initComposer } from "./composer";

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

test.each([
    ["plain message", "literal"],
    ["/skill:review inspect", "skill"],
    ["/skill", "skill"],
    ["/review args", "prompt"],
    ["!pwd", "command"],
    ["!!pwd", "command"],
    ["  \\!literal", "literal"],
    ["\\/literal", "literal"],
])("classifyComposerInput(%s) reports %s", (value, kind) => {
    expect(classifyComposerInput(value).kind).toBe(kind);
});

test("an excluded direct command stays excluded", () => {
    expect(classifyComposerInput("!!echo hi")).toEqual({
        kind: "command",
        excluded: true,
    });
});

test.each([
    ["hello", "/ hello", 1],
    ["!!pwd", "/ !!pwd", 1],
    ["/review hello", "/review hello", 7],
])(
    "the slash action preserves %s at the leading command position",
    (value, expected, caret) => {
        const form = composerForm();
        const action = document.createElement("button");
        action.type = "button";
        action.dataset.composerInsert = "/";
        form.append(action);
        document.body.append(form);
        const controller = new AbortController();
        initComposer(form, { signal: controller.signal });
        const field = form.querySelector("textarea")!;
        field.value = value;
        field.setSelectionRange(1, value.length);
        action.click();
        expect(field.value).toBe(expected);
        expect(field.selectionStart).toBe(caret);
        expect(field.selectionEnd).toBe(caret);
        expect(classifyComposerInput(field.value).kind).not.toBe("command");
        controller.abort();
        form.remove();
    },
);

test.each([
    ["host", "sandbox", "This computer"],
    ["sandbox", "host", "Sandbox"],
])(
    "command feedback uses the applied %s location, not requested %s",
    (applied, requested, label) => {
        const form = composerForm();
        form.dataset.commandLocation = applied;
        const status = document.createElement("p");
        status.dataset.composerCommandStatus = "";
        form.append(status);
        const radio = document.createElement("input");
        radio.type = "radio";
        radio.name = "location";
        radio.value = requested;
        radio.checked = true;
        document.body.append(form, radio);
        const controller = new AbortController();
        const field = form.querySelector("textarea")!;
        field.value = "!pwd";
        initComposer(form, { signal: controller.signal });
        expect(status.textContent).toContain(`Direct command · ${label} ·`);
        controller.abort();
        form.remove();
        radio.remove();
    },
);

test("the skills action inserts a prefix and the command action explains exclusion", () => {
    const form = document.createElement("form");
    form.innerHTML = `
        <textarea id="composer-message"></textarea>
        <button type="button" data-composer-insert="/">Skills and prompts</button>
        <span data-composer-submit-label>Send</span>
        <button type="submit" data-composer-submit data-default-aria-label="Send message"></button>
        <p data-composer-command-status hidden></p>
    `;
    document.body.append(form);
    const textarea = form.querySelector("textarea")!;
    const status = form.querySelector<HTMLElement>(
        "[data-composer-command-status]",
    )!;
    const label = form.querySelector<HTMLElement>(
        "[data-composer-submit-label]",
    )!;
    initComposer(form, { signal: new AbortController().signal });

    form.querySelector<HTMLButtonElement>("[data-composer-insert]")!.click();
    expect(textarea.value).toBe("/");

    textarea.value = "!!pwd";
    textarea.dispatchEvent(new InputEvent("input", { bubbles: true }));
    expect(label.textContent).toBe("Run without context");
    expect(status.hidden).toBe(false);
    expect(status.textContent).toContain(
        "Not added to automatic model context",
    );
    expect(status.textContent).toContain("No provider connection is needed");

    textarea.value = "Hello";
    textarea.dispatchEvent(new InputEvent("input", { bubbles: true }));
    expect(label.textContent).toBe("Send");
    expect(status.hidden).toBe(true);
    form.remove();
});
