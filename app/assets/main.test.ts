// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { commandBlockReason } from "hypergraft/browser";
vi.mock("./hypergraft-bootstrap", () => ({ startApp: vi.fn() }));
const settled = vi.hoisted(
    () =>
        [] as Parameters<
            typeof import("hypergraft/browser").listenForRequestSettled
        >[0][],
);
vi.mock("hypergraft/browser", () => ({
    commandBlockReason: vi.fn(),
    listenForRequestSettled: (listener: (typeof settled)[number]) =>
        settled.push(listener),
}));
import "./main";

beforeEach(() => {
    vi.mocked(commandBlockReason).mockReset();
    vi.stubGlobal("Option", function (text: string, value: string) {
        const option = document.createElement("option");
        option.text = text;
        option.value = value;
        return option;
    });
    document.body.innerHTML = `<form id="conversation-composer">
        <input id="conversation-provider" name="provider" type="hidden" value="one">
        <input id="conversation-model" name="model" type="hidden" value="Alpha">
        <select id="conversation-thinking" name="thinking" hidden><option value="high" selected>High</option></select>
        <button id="conversation-thinking-toggle" type="button"><span id="conversation-thinking-value">High</span></button>
        <section id="conversation-thinking-options"><div id="conversation-thinking-results"></div></section>
        <textarea id="composer-message" name="message">Unsent text</textarea>
        <button id="conversation-model-toggle" type="button"><span id="conversation-model-value">Alpha</span></button>
        <section id="conversation-model-options">
            <input id="conversation-model-search" type="search">
            <button id="conversation-model-search-clear" type="button" hidden>Clear model search</button>
            <select id="conversation-model-provider-filter"><option value="">All providers</option><option value="one">One</option><option value="two">Two</option></select>
            <button id="conversation-model-favourites-filter" type="button" aria-pressed="false">Favourites</button>
            <p id="conversation-model-search-status"></p>
            <div id="conversation-model-results"></div>
            <template data-conversation-model-catalogue></template>
        </section>
    </form>`;
    for (const id of [
        "conversation-model-options",
        "conversation-thinking-options",
    ])
        document.getElementById(id)!.hidePopover = vi.fn();
    document.querySelector<HTMLElement>(
        "[data-conversation-model-catalogue]",
    )!.dataset.conversationModelCatalogue = JSON.stringify({
        one: [
            {
                id: "Alpha",
                favourite: false,
                default_effort: "high",
                efforts: [{ value: "high", label: "High" }],
            },
            { id: "<Beta>", favourite: true, default_effort: "", efforts: [] },
        ],
        two: [
            {
                id: "Alpha",
                favourite: false,
                default_effort: "low",
                efforts: [{ value: "low", label: "Low" }],
            },
        ],
    });
});
afterEach(() => {
    document.body.replaceChildren();
    vi.unstubAllGlobals();
});
function search(query: string) {
    const input = document.querySelector<HTMLInputElement>(
        "#conversation-model-search",
    )!;
    input.value = query;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    return input;
}
function value(name: string) {
    return new FormData(document.querySelector("form")!).get(name);
}
function enter(input: HTMLElement) {
    input.dispatchEvent(
        new KeyboardEvent("keydown", {
            key: "Enter",
            bubbles: true,
            cancelable: true,
        }),
    );
}

test("search normalises text without changing the model, effort or message, and renders names as text", () => {
    search("  ALP  ");
    expect(document.querySelectorAll("[data-composer-model]")).toHaveLength(2);
    search("<beta>");
    const option = document.querySelector<HTMLButtonElement>(
        "[data-composer-model]",
    )!;
    expect(option.textContent).toBe("<Beta>One");
    expect(option.querySelector("beta")).toBeNull();
    search("no match");
    expect(document.querySelectorAll("[data-composer-model]")).toHaveLength(0);
    const clear = document.querySelector<HTMLButtonElement>(
        "#conversation-model-search-clear",
    )!;
    const submit = vi.fn();
    document.querySelector("form")!.addEventListener("submit", submit);
    expect(clear.hidden).toBe(false);
    clear.click();
    expect(document.querySelectorAll("[data-composer-model]")).toHaveLength(3);
    expect(clear.hidden).toBe(true);
    expect(document.activeElement?.id).toBe("conversation-model-search");
    expect(submit).not.toHaveBeenCalled();
    expect(value("model")).toBe("Alpha");
    expect(value("thinking")).toBe("high");
    expect(value("message")).toBe("Unsent text");
});

test("effort choices accept only enabled options and preserve the unsent message", () => {
    const select = document.querySelector<HTMLSelectElement>(
        "#conversation-thinking",
    )!;
    select.add(new Option("Low", "low"));
    select.dispatchEvent(new Event("change", { bubbles: true }));
    const results = document.getElementById("conversation-thinking-results")!;
    const forged = document.createElement("button");
    forged.type = "button";
    forged.dataset.thinkingValue = "unknown";
    results.append(forged);
    forged.click();
    expect(value("thinking")).toBe("high");
    results
        .querySelector<HTMLButtonElement>('[data-thinking-value="low"]')!
        .click();
    expect(value("thinking")).toBe("low");
    expect(
        document.getElementById("conversation-thinking-value")!.textContent,
    ).toBe("Low");
    select.disabled = true;
    results
        .querySelector<HTMLButtonElement>('[data-thinking-value="high"]')!
        .click();
    expect(select.value).toBe("low");
    expect(value("message")).toBe("Unsent text");
});

test("model command patches preserve the latest unsent message and do not submit it", async () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        `<form id="conversation-model-form" action="/conversations/example/model"></form>
        <form id="conversation-settings-form"><input name="provider" value="one"><input name="model" value="Alpha"><input name="thinking" value="high"></form>`,
    );
    // happy-dom only resolves external form owners for controls outside another form.
    for (const id of [
        "conversation-provider",
        "conversation-model",
        "conversation-thinking",
    ]) {
        const control = document.getElementById(id)!;
        control.remove();
        control.setAttribute("form", "conversation-model-form");
        document.body.append(control);
    }
    const form = document.querySelector<HTMLFormElement>(
        "#conversation-model-form",
    )!;
    const submit = vi.spyOn(form, "requestSubmit").mockImplementation(() => {
        form.dispatchEvent(
            new Event("submit", { bubbles: true, cancelable: true }),
        );
    });
    const composerSubmit = vi.fn();
    document
        .querySelector("#conversation-composer")!
        .addEventListener("submit", composerSubmit);
    const thinking = document.querySelector<HTMLSelectElement>(
        "#conversation-thinking",
    )!;
    thinking.dispatchEvent(new Event("change", { bubbles: true }));
    await Promise.resolve();
    expect(submit).toHaveBeenCalledOnce();
    expect(settled.length).toBeGreaterThan(0);
    const message =
        document.querySelector<HTMLTextAreaElement>("#composer-message")!;
    message.value = "More text during the request";
    message.dispatchEvent(new Event("input", { bubbles: true }));
    document.body.innerHTML = document.body.innerHTML;
    document.querySelector<HTMLTextAreaElement>("#composer-message")!.value =
        "";
    for (const listener of settled)
        listener({
            requestKind: "patch",
            form,
            url: form.action,
            outcome: "applied-patch",
            status: 200,
            targetIds: ["conversation-detail"],
        });
    expect(
        document.querySelector<HTMLTextAreaElement>("#composer-message")!.value,
    ).toBe("More text during the request");
    expect(submit).toHaveBeenCalledOnce();
    expect(composerSubmit).not.toHaveBeenCalled();
});

test.each([
    "pending-command",
    "pending-navigation",
    "uncertain-command",
] as const)(
    "%s leaves model actions unchanged and permits a later retry",
    async (reason) => {
        document.body.insertAdjacentHTML(
            "beforeend",
            `<form id="conversation-model-form"></form>
             <form id="conversation-favourite-form"><input name="provider"><input name="model"></form>`,
        );
        for (const id of [
            "conversation-provider",
            "conversation-model",
            "conversation-thinking",
        ]) {
            const control = document.getElementById(id)!;
            control.remove();
            control.setAttribute("form", "conversation-model-form");
            document.body.append(control);
        }
        const modelForm = document.querySelector<HTMLFormElement>(
            "#conversation-model-form",
        )!;
        const favouriteForm = document.querySelector<HTMLFormElement>(
            "#conversation-favourite-form",
        )!;
        const modelSubmit = vi.spyOn(modelForm, "requestSubmit");
        const favouriteSubmit = vi
            .spyOn(favouriteForm, "requestSubmit")
            .mockImplementation(() => {});
        const thinking = document.querySelector<HTMLSelectElement>(
            "#conversation-thinking",
        )!;
        thinking.add(new Option("Low", "low"));
        document.getElementById("conversation-thinking-results")!.innerHTML =
            '<button type="button" data-thinking-value="low">Low</button>';
        search("<beta>");
        vi.mocked(commandBlockReason).mockReturnValue(reason);
        document
            .querySelector<HTMLButtonElement>("[data-composer-model]")!
            .click();
        document
            .querySelector<HTMLButtonElement>("[data-thinking-value]")!
            .click();
        document
            .querySelector<HTMLButtonElement>("[data-model-favourite]")!
            .click();
        await Promise.resolve();
        expect(modelSubmit).not.toHaveBeenCalled();
        expect(favouriteSubmit).not.toHaveBeenCalled();
        expect(new FormData(modelForm).get("model")).toBe("Alpha");
        expect(thinking.value).toBe("high");
        expect(thinking.disabled).toBe(false);
        expect(
            document.querySelector<HTMLButtonElement>(
                "#conversation-model-toggle",
            )!.disabled,
        ).toBe(false);
        expect(new FormData(favouriteForm).get("model")).toBe("");

        vi.mocked(commandBlockReason).mockReturnValue(undefined);
        document
            .querySelector<HTMLButtonElement>("[data-model-favourite]")!
            .click();
        expect(favouriteSubmit).toHaveBeenCalledOnce();
        for (const listener of settled)
            listener({
                requestKind: "patch",
                form: favouriteForm,
                url: favouriteForm.action,
                outcome: "applied-patch",
                status: 200,
                targetIds: ["conversation-model-catalogue"],
            });
        modelSubmit.mockRestore();
        favouriteSubmit.mockRestore();
    },
);

test("Enter rejects arbitrary search text and ambiguous model identities across providers", () => {
    enter(search("no match"));
    enter(search("Alpha"));
    expect(value("provider")).toBe("one");
    expect(value("thinking")).toBe("high");
    const filter = document.querySelector<HTMLSelectElement>(
        "#conversation-model-provider-filter",
    )!;
    filter.value = "two";
    filter.dispatchEvent(new Event("change", { bubbles: true }));
    expect(value("provider")).toBe("one");
    enter(search("Alpha"));
    expect(value("provider")).toBe("two");
    expect(value("model")).toBe("Alpha");
    expect(value("thinking")).toBe("low");
    expect(value("message")).toBe("Unsent text");
});

test("copy uses the source bound to the clicked response and reports success", async () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        `<article id="message-one"><div data-response-copy>` +
            `<button type="button" data-copy-response="message-one">Copy</button>` +
            `<span data-copy-status></span>` +
            `<template data-copy-source data-copy-for="message-one"># Title\n\n\`\`\`rust\nfn main() {}\n\`\`\`\nLiteral &lt;b&gt;HTML&lt;/b&gt;</template>` +
            `</div></article>` +
            `<article id="message-two"><div data-response-copy>` +
            `<button type="button" data-copy-response="message-two">Copy</button>` +
            `<span data-copy-status></span>` +
            `<template data-copy-source data-copy-for="message-two">Second response</template>` +
            `</div></article>`,
    );
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    document
        .querySelector<HTMLButtonElement>('[data-copy-response="message-one"]')!
        .click();
    await Promise.resolve();
    expect(writeText).toHaveBeenCalledWith(
        "# Title\n\n```rust\nfn main() {}\n```\nLiteral <b>HTML</b>",
    );
    expect(
        document.querySelector<HTMLElement>("#message-one [data-copy-status]")!
            .textContent,
    ).toBe("Copied");
    expect(
        document.querySelector<HTMLElement>("#message-two [data-copy-status]")!
            .textContent,
    ).toBe("");
});
