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
const locations = vi.hoisted(
    () =>
        [] as Parameters<
            typeof import("hypergraft/browser").listenForLocationChanges
        >[0][],
);
vi.mock("hypergraft/browser", () => ({
    commandBlockReason: vi.fn(),
    listenForRequestSettled: (listener: (typeof settled)[number]) =>
        settled.push(listener),
    listenForLivePatches: vi.fn(),
    listenForLocationChanges: (listener: (typeof locations)[number]) =>
        locations.push(listener),
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
            <button id="conversation-model-images-filter" type="button" aria-pressed="false">Images</button>
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
                image_input: true,
                default_effort: "high",
                efforts: [{ value: "high", label: "High" }],
            },
            {
                id: "<Beta>",
                favourite: true,
                image_input: false,
                default_effort: "",
                efforts: [],
            },
        ],
        two: [
            {
                id: "Alpha",
                favourite: false,
                image_input: false,
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
test("catalogue selection receives focus after the runtime navigation target", async () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        `<main class="workspace-catalogue"><div><h2 id="workflow-destination-heading" tabindex="-1">Destination</h2></div></main>`,
    );
    const content = document.querySelector("main > div")!;
    const target = document.getElementById("workflow-destination-heading")!;
    const message =
        document.querySelector<HTMLTextAreaElement>("#composer-message")!;
    message.focus();
    content.scrollTop = 500;
    for (const listener of locations) {
        listener({ url: "/workflows?workflow=one", cause: "link-navigation" });
    }
    message.focus();
    expect(document.activeElement).toBe(message);
    await Promise.resolve();
    expect(document.activeElement).toBe(target);
    expect(content.scrollTop).toBe(0);
    expect(message.value).toBe("Unsent text");
});

test("the strip and Current work mirror only the active server status as text", () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        `
        <div data-reply-strip hidden><span data-reply-strip-text></span></div>
        <h3 data-work-reply-status>Waiting for model</h3><p data-work-retry-message hidden></p>
        <section id="transcript"><article class="conversation-turn">
          <header class="chat-turn-meta"><span data-reply-active="true" data-reply-status="Thinking"></span></header>
          <div class="chat-prose"><span data-reply-active="true" data-reply-status="Untrusted content"></span></div>
        </article></section>`,
    );
    const strip = document.querySelector<HTMLElement>("[data-reply-strip]")!;
    const text = strip.querySelector<HTMLElement>("[data-reply-strip-text]")!;
    const work = document.querySelector<HTMLElement>(
        "[data-work-reply-status]",
    )!;
    const status = document.querySelector<HTMLElement>(
        ".chat-turn-meta > span",
    )!;
    for (const value of [
        "Compacting context",
        "Waiting for model",
        "Thinking",
        "Tool call in progress",
        "<img src=x onerror=alert(1)>",
    ]) {
        status.dataset.replyStatus = value;
        status.dataset.replyRetryMessage = value;
        document.dispatchEvent(new CustomEvent("hypergraft:progress"));
        expect(strip.hidden).toBe(false);
        expect(text.textContent).toBe(value);
        expect(text.children.length).toBe(0);
        expect(work.textContent).toBe(value);
        expect(work.children.length).toBe(0);
        const retry = document.querySelector<HTMLElement>(
            "[data-work-retry-message]",
        )!;
        expect(retry.textContent).toBe(value);
        expect(retry.children.length).toBe(0);
        expect(retry.hidden).toBe(false);
    }
    delete status.dataset.replyRetryMessage;
    status.dataset.replyStatus = "Replying";
    document.dispatchEvent(new CustomEvent("hypergraft:progress"));
    expect(
        document.querySelector<HTMLElement>("[data-work-retry-message]")!
            .hidden,
    ).toBe(true);
    work.removeAttribute("data-work-reply-status");
    work.textContent = "Needs your answer";
    document.dispatchEvent(new CustomEvent("hypergraft:progress"));
    expect(work.textContent).toBe("Needs your answer");
    work.setAttribute("data-work-reply-status", "");
    status.dataset.replyActive = "false";
    document.dispatchEvent(new CustomEvent("hypergraft:progress"));
    expect(strip.hidden).toBe(true);
    status.dataset.replyActive = "true";
    document.body.insertAdjacentHTML(
        "beforeend",
        '<div id="conversation-history-status"></div>',
    );
    document.dispatchEvent(new CustomEvent("hypergraft:progress"));
    expect(strip.hidden).toBe(true);
    expect(work.textContent).toBe("Needs your answer");
    document.getElementById("conversation-history-status")!.remove();
    document.getElementById("transcript")!.replaceChildren();
    for (const listener of locations)
        listener({ url: "/conversations/other", cause: "link-navigation" });
    expect(strip.hidden).toBe(true);
    expect(text.textContent).toBe("");
});

test("preset names use the UTF-8 bound without control characters", () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        `<input id="conversation-preset-name"><p id="preset-name-error" hidden></p><button data-preset-save-cancel></button>`,
    );
    const field = document.querySelector<HTMLInputElement>(
        "#conversation-preset-name",
    )!;
    for (const [value, valid] of [
        ["é".repeat(40), true],
        ["é".repeat(41), false],
        ["Bad\u0001name", false],
        ["", true],
        ["  Review  ", true],
    ] as const) {
        field.value = value;
        field.dispatchEvent(new Event("input", { bubbles: true }));
        expect(field.validity.valid).toBe(valid);
        expect(field.value).toBe(value);
    }
    field.value = "é".repeat(41);
    field.dispatchEvent(new Event("input", { bubbles: true }));
    document
        .querySelector<HTMLButtonElement>("[data-preset-save-cancel]")!
        .click();
    expect(field.disabled).toBe(true);
    expect(field.validationMessage).toBe("");
});

test.each(["new", "saved"])(
    "%s preset snapshots keep the correct source of settings",
    (state) => {
        document.body.insertAdjacentHTML(
            "beforeend",
            `<template data-conversation-state="${state}"></template>
        <textarea name="instructions" form="conversation-composer">&lt;img src=x&gt;</textarea>
        <select name="environment" form="conversation-composer"><option value="second-id" data-environment-name="Same name" selected>Same name</option></select>
        <button data-settings-tab="settings-presets"></button>
        <p data-preset-summary="instructions">Saved instructions</p>
        <p data-preset-summary="tools">Read</p>
        <div data-preset-directories hidden><span data-path="/tmp/review-specs" data-access="Direct write" data-available="true"></span></div>
        <p data-preset-summary="directories">Saved directories</p>
        <div data-preset-row="environment" data-preset-requested-identity="first-id"><p data-preset-current="environment">Same name</p><span data-preset-changed hidden>Changed</span></div>`,
        );
        document
            .querySelector<HTMLButtonElement>(
                '[data-settings-tab="settings-presets"]',
            )!
            .click();
        expect(
            document.querySelector('[data-preset-summary="instructions"]')!
                .textContent,
        ).toBe(state === "new" ? "<img src=x>" : "Saved instructions");
        expect(
            document.querySelector('[data-preset-summary="tools"]')!
                .textContent,
        ).toBe(state === "new" ? "None" : "Read");
        expect(
            document.querySelector<HTMLElement>("[data-preset-changed]")!
                .hidden,
        ).toBe(state !== "new");
        expect(
            document.querySelector('[data-preset-summary="directories"]')!
                .textContent,
        ).toBe(
            state === "new"
                ? "Start: /tmp/review-specs\nDirect write requested"
                : "Saved directories",
        );
        expect(
            document.querySelector('[data-preset-summary="instructions"] img'),
        ).toBeNull();
    },
);

test("instruction validation counts UTF-8 bytes and retains invalid text", () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        `
        <textarea id="conversation-instructions" data-instruction-limit="32768"></textarea>
        <p id="conversation-instructions-error" hidden></p>`,
    );
    const field = document.querySelector<HTMLTextAreaElement>(
        "#conversation-instructions",
    )!;
    for (const [text, valid] of [
        ["é".repeat(16384), true],
        ["é".repeat(16385), false],
        ["Rules\n\tNext line", true],
        ["Bad\u0000rule", false],
        ["Bad\u0085rule", false],
        ["", true],
    ] as const) {
        field.value = text;
        field.dispatchEvent(new Event("input", { bubbles: true }));
        expect(field.validity.valid).toBe(valid);
        expect(field.value).toBe(text);
        expect(field.getAttribute("aria-invalid")).toBe(String(!valid));
        expect(
            document.getElementById("conversation-instructions-error")!.hidden,
        ).toBe(valid);
    }
});

test("requested execution changes block consent for the old configuration without a command", () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        `
        <form id="conversation-settings-form"></form>
        <input form="conversation-settings-form" type="radio" name="host_approval" value="ask-each-time" checked>
        <input form="conversation-settings-form" type="radio" name="host_approval" value="automatic">
        <button id="conversation-host-consent-preview">Review host access</button>
        <p data-consent-review-note hidden></p>`,
    );
    const form = document.querySelector<HTMLFormElement>(
        "#conversation-settings-form",
    )!;
    const submit = vi.spyOn(form, "requestSubmit");
    const automatic = document.querySelector<HTMLInputElement>(
        '[name="host_approval"][value="automatic"]',
    )!;
    const consent = document.querySelector<HTMLButtonElement>(
        "#conversation-host-consent-preview",
    )!;
    automatic.checked = true;
    automatic.dispatchEvent(new Event("change", { bubbles: true }));
    expect(consent.disabled).toBe(true);
    expect(
        document.querySelector<HTMLElement>("[data-consent-review-note]")!
            .hidden,
    ).toBe(false);
    const ask = document.querySelector<HTMLInputElement>(
        '[name="host_approval"][value="ask-each-time"]',
    )!;
    ask.checked = true;
    ask.dispatchEvent(new Event("change", { bubbles: true }));
    expect(consent.disabled).toBe(false);
    expect(submit).not.toHaveBeenCalled();
});

test("directory radios stage saved access without a consent or execution command", () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        `
        <form id="conversation-settings-form"><input id="execution-directory-access" name="directory_access"></form>
        <input type="radio" name="access-one" data-execution-directory="one" value="read-only" checked>
        <input type="radio" name="access-one" data-execution-directory="one" value="direct-write">
        <input type="radio" name="access-two" data-execution-directory="two" value="review-before-apply" checked>`,
    );
    const form = document.querySelector<HTMLFormElement>(
        "#conversation-settings-form",
    )!;
    const submit = vi.spyOn(form, "requestSubmit");
    const radio = document.querySelector<HTMLInputElement>(
        '[data-execution-directory][value="direct-write"]',
    )!;
    radio.checked = true;
    radio.dispatchEvent(new Event("change", { bubbles: true }));
    expect(new FormData(form).get("directory_access")).toBe(
        JSON.stringify([
            ["one", "direct-write"],
            ["two", "review-before-apply"],
        ]),
    );
    expect(submit).not.toHaveBeenCalled();
});

test("draft directory radios use the access command rather than the message command", () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        `
        <input type="radio" data-draft-directory-access="directory-access-one" value="direct-write">
        <button id="directory-access-one" form="conversation-composer" formaction="/conversations/new/directories/one/access" type="submit" name="action" hidden></button>`,
    );
    const form = document.querySelector<HTMLFormElement>(
        "#conversation-composer",
    )!;
    const submit = vi.spyOn(form, "requestSubmit").mockImplementation(() => {});
    const radio = document.querySelector<HTMLInputElement>(
        "[data-draft-directory-access]",
    )!;
    radio.checked = true;
    radio.dispatchEvent(new Event("change", { bubbles: true }));
    const button = document.querySelector<HTMLButtonElement>(
        "#directory-access-one",
    )!;
    expect(button.value).toBe("direct-write");
    expect(submit).toHaveBeenCalledExactlyOnceWith(button);
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

function attachmentMarkup(action: string): string {
    return `<div data-attachments>
        <p data-attachment-error hidden></p>
        <p data-attachment-pending hidden></p>
        <form
            method="post"
            action="${action}"
            enctype="multipart/form-data"
            data-attachment-upload
        >
            <input
                type="file"
                name="image"
                multiple
                data-attachment-input
            />
        </form>
    </div>`;
}

function transferredFiles(files: { name: string; type: string }[]) {
    const transfer = new DataTransfer();
    for (const file of files)
        transfer.items.add(
            new File([new Uint8Array([1])], file.name, { type: file.type }),
        );
    return transfer;
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

test("images filter keeps only models with catalogue image support", () => {
    const filter = document.querySelector<HTMLButtonElement>(
        "#conversation-model-images-filter",
    )!;
    filter.click();
    expect(filter.ariaPressed).toBe("true");
    const models = Array.from(
        document.querySelectorAll<HTMLButtonElement>("[data-composer-model]"),
    );
    expect(models.map((model) => model.dataset.composerModel)).toEqual([
        "Alpha",
    ]);
    filter.click();
    expect(filter.ariaPressed).toBe("false");
    expect(document.querySelectorAll("[data-composer-model]")).toHaveLength(3);
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

test("patches keep the frozen revision without restoring deleted text or a successful Send", () => {
    document.body.innerHTML = `<section id="conversation-detail">
        <form id="conversation-composer">
            <div data-revision-state
                data-revision-source="abc"
                data-revision-parent=""
                data-revision-active-leaf="def"
                data-revision-revision="5"
                data-revision-text="Original prompt">
                <a href="/conversations/one" data-cancel-revision>Cancel revision</a>
            </div>
            <textarea id="composer-message"></textarea>
        </form>
    </section>`;
    const settle = (targetIds: string[]) => {
        const form = document.createElement("form");
        for (const listener of settled)
            listener({
                requestKind: "patch",
                form,
                url: "/conversations/one/model",
                outcome: "applied-patch",
                status: 200,
                targetIds,
            });
    };
    settle(["conversation-detail"]);
    document.querySelector("[data-revision-state]")!.remove();
    document.querySelector<HTMLInputElement>('input[name="revision"]')!.value =
        "6";
    document.querySelector<HTMLTextAreaElement>("#composer-message")!.value =
        "";
    settle(["conversation-detail"]);
    const form = document.querySelector<HTMLFormElement>(
        "#conversation-composer",
    )!;
    expect(
        (form.elements.namedItem("revise_source") as HTMLInputElement).value,
    ).toBe("abc");
    expect(
        (form.elements.namedItem("revise_active_leaf") as HTMLInputElement)
            .value,
    ).toBe("def");
    expect(
        document.querySelector<HTMLTextAreaElement>("#composer-message")!.value,
    ).toBe("");
    expect(document.querySelector("[data-revision-state]")).not.toBeNull();
    expect(
        (form.elements.namedItem("revision") as HTMLInputElement).value,
    ).toBe("5");

    document.querySelector("[data-revision-state]")!.remove();
    document.querySelector<HTMLTextAreaElement>("#composer-message")!.value =
        "";
    for (const listener of settled)
        listener({
            requestKind: "patch",
            form,
            url: "/messages",
            outcome: "applied-patch",
            status: 200,
            targetIds: ["conversation-detail"],
        });
    settle(["conversation-detail"]);
    expect(document.querySelector("[data-revision-state]")).toBeNull();
    expect(
        document.querySelector<HTMLTextAreaElement>("#composer-message")!.value,
    ).toBe("");
});

test("Revise confirms before it discards an unrelated unsent draft", () => {
    document.body.innerHTML = `<section id="conversation-detail">
        <form id="conversation-composer">
            <textarea id="composer-message">Unsent draft</textarea>
        </form>
        <a id="revise-link" href="/conversations/one?revise=abc" data-revise-prompt>Revise</a>
    </section>`;
    const confirm = vi.fn().mockReturnValue(false);
    vi.stubGlobal("confirm", confirm);
    const link = document.getElementById("revise-link")!;
    const dispatched = link.dispatchEvent(
        new MouseEvent("click", { bubbles: true, cancelable: true }),
    );
    expect(confirm).toHaveBeenCalled();
    expect(dispatched).toBe(false);
    expect(
        document.querySelector<HTMLTextAreaElement>("#composer-message")!.value,
    ).toBe("Unsent draft");
});

test("a link navigation keeps the revision target across an unrelated view", () => {
    document.body.innerHTML = `<section id="conversation-detail">
        <template data-conversation-url="/conversations/one"></template>
        <form id="conversation-composer">
            <div data-revision-state
                data-revision-source="abc"
                data-revision-parent=""
                data-revision-active-leaf="def"
                data-revision-revision="5"
                data-revision-text="Original prompt">
                <a href="/conversations/one" data-cancel-revision>Cancel revision</a>
            </div>
            <textarea id="composer-message"></textarea>
        </form>
    </section>`;
    const navigate = () => {
        for (const listener of locations)
            listener({
                url: "http://localhost/conversations/one/tree",
                cause: "link-navigation",
            });
    };
    navigate();
    document.querySelector("[data-revision-state]")!.remove();
    document.querySelector<HTMLTextAreaElement>("#composer-message")!.value =
        "";
    navigate();
    const form = document.querySelector<HTMLFormElement>(
        "#conversation-composer",
    )!;
    expect(
        (form.elements.namedItem("revise_source") as HTMLInputElement).value,
    ).toBe("abc");
    expect(document.querySelector("[data-revision-state]")).not.toBeNull();
    expect(
        document.querySelector<HTMLTextAreaElement>("#composer-message")!.value,
    ).toBe("");

    // A different conversation never inherits the previous target.
    const identity = document.querySelector<HTMLElement>(
        "[data-conversation-url]",
    )!;
    identity.dataset.conversationUrl = "/conversations/two";
    document.querySelector("[data-revision-state]")!.remove();
    navigate();
    expect(document.querySelector("[data-revision-state]")).toBeNull();
});

test("mixed paste inserts the text once and stages each image once", () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        attachmentMarkup("/conversations/new/attachments?draft=aaa"),
    );
    const form = document.querySelector<HTMLFormElement>(
        "[data-attachment-upload]",
    )!;
    form.requestSubmit = vi.fn();
    const message =
        document.querySelector<HTMLTextAreaElement>("#composer-message")!;
    message.value = "Hello world";
    message.setSelectionRange(5, 5);
    const transfer = transferredFiles([
        { name: "shot.png", type: "image/png" },
    ]);
    const repeated = transfer.items[0].getAsFile()!;
    transfer.items.add(repeated);
    transfer.setData("text/plain", " and paste");
    const paste = new ClipboardEvent("paste", {
        bubbles: true,
        cancelable: true,
        clipboardData: transfer,
    });
    message.dispatchEvent(paste);
    expect(paste.defaultPrevented).toBe(true);
    expect(message.value).toBe("Hello and paste world");
    const input = document.querySelector<HTMLInputElement>(
        "[data-attachment-input]",
    )!;
    expect(input.files).toHaveLength(1);
    expect(form.requestSubmit).toHaveBeenCalledOnce();
    expect(
        document.querySelector<HTMLElement>("[data-attachment-pending]")!
            .hidden,
    ).toBe(false);
});

test("mixed files retain distinct images and rejection details after the upload patch", () => {
    const action = "/conversations/new/attachments?draft=aaa";
    document.body.insertAdjacentHTML("beforeend", attachmentMarkup(action));
    const form = document.querySelector<HTMLFormElement>(
        "[data-attachment-upload]",
    )!;
    form.requestSubmit = vi.fn();
    const transfer = new DataTransfer();
    for (const byte of [1, 2]) {
        transfer.items.add(
            new File([new Uint8Array([byte])], "image.png", {
                type: "image/png",
                lastModified: 1,
            }),
        );
    }
    transfer.items.add(
        new File(["pdf"], "notes.pdf", { type: "application/pdf" }),
    );
    document.querySelector("#composer-message")!.dispatchEvent(
        new ClipboardEvent("paste", {
            bubbles: true,
            cancelable: true,
            clipboardData: transfer,
        }),
    );
    expect(form.querySelector<HTMLInputElement>("input")!.files).toHaveLength(
        2,
    );
    expect(
        document.querySelector("[data-attachment-error]")!.textContent,
    ).toContain("PNG");
    expect(form.requestSubmit).toHaveBeenCalledOnce();
    const submittedForm = form.querySelector("input")!.closest("form")!;
    document
        .querySelector("[data-attachments]")!
        .replaceWith(
            document
                .createRange()
                .createContextualFragment(attachmentMarkup(action)),
        );
    for (const listener of settled)
        listener({
            requestKind: "patch",
            form: submittedForm,
            url: form.action,
            outcome: "applied-patch",
            status: 200,
            targetIds: ["conversation-attachment-controls"],
        });
    const error = document.querySelector<HTMLElement>(
        "[data-attachment-error]",
    )!;
    expect(error.hidden).toBe(false);
    expect(error.textContent).toContain("PNG");
    expect(
        document.querySelector<HTMLTextAreaElement>("#composer-message")!.value,
    ).toBe("Unsent text");
});

test("a text-only paste keeps the browser's ordinary insertion", () => {
    const transfer = new DataTransfer();
    transfer.setData("text/plain", "plain text");
    const paste = new ClipboardEvent("paste", {
        bubbles: true,
        cancelable: true,
        clipboardData: transfer,
    });
    document
        .querySelector<HTMLTextAreaElement>("#composer-message")!
        .dispatchEvent(paste);
    expect(paste.defaultPrevented).toBe(false);
    expect(
        document.querySelector<HTMLInputElement>("[data-attachment-input]"),
    ).toBeNull();
});

test("an unsupported drop reports beside the draft and stages nothing", () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        attachmentMarkup("/conversations/new/attachments?draft=aaa"),
    );
    const dock = document.createElement("div");
    dock.dataset.attachmentDropZone = "";
    const hint = document.createElement("p");
    hint.dataset.attachmentDropHint = "";
    hint.className = "hidden";
    dock.append(hint);
    document.body.prepend(dock);
    const transfer = transferredFiles([
        { name: "notes.txt", type: "text/plain" },
    ]);
    const drop = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(drop, "dataTransfer", { value: transfer });
    dock.dispatchEvent(drop);
    expect(drop.defaultPrevented).toBe(true);
    expect(hint.classList.contains("hidden")).toBe(true);
    const input = document.querySelector<HTMLInputElement>(
        "[data-attachment-input]",
    )!;
    expect(input.files ?? []).toHaveLength(0);
    const error = document.querySelector<HTMLElement>(
        "[data-attachment-error]",
    )!;
    expect(error.hidden).toBe(false);
    expect(error.textContent).toContain("PNG");
});

test("a stale upload completion after navigation leaves the new draft alone", () => {
    document.body.insertAdjacentHTML(
        "beforeend",
        attachmentMarkup("/conversations/new/attachments?draft=aaa"),
    );
    const oldForm = document.querySelector<HTMLFormElement>(
        "[data-attachment-upload]",
    )!;
    oldForm.requestSubmit = vi.fn();
    const oldInput = oldForm.querySelector<HTMLInputElement>(
        "[data-attachment-input]",
    )!;
    oldInput.files = transferredFiles([
        { name: "a.png", type: "image/png" },
    ]).files;
    oldInput.dispatchEvent(new Event("change", { bubbles: true }));
    // Navigation replaces the composer scope with another draft.
    document.querySelector("[data-attachments]")!.innerHTML = `
        <p data-attachment-error hidden></p>
        <p data-attachment-pending hidden></p>
        <form method="post" action="/conversations/new/attachments?draft=bbb" data-attachment-upload>
            <input type="file" name="image" multiple data-attachment-input />
        </form>`;
    const newInput = document.querySelector<HTMLInputElement>(
        "[data-attachment-input]",
    )!;
    newInput.files = transferredFiles([
        { name: "b.png", type: "image/png" },
    ]).files;
    for (const listener of settled)
        listener({
            requestKind: "patch",
            form: oldInput.closest("form")!,
            url: oldForm.action,
            outcome: "applied-patch",
            status: 200,
            targetIds: ["conversation-attachment-controls"],
        });
    expect(newInput.files).toHaveLength(1);
    expect(newInput.files?.[0]?.name).toBe("b.png");
});

function fileLookupMarkup(): void {
    document.body.innerHTML = `
        <template data-conversation-state="new"></template>
        <div data-file-suggestions hidden></div>
        <form id="conversation-composer">
            <input type="hidden" name="draft_nonce" value="${"a".repeat(64)}">
            <textarea id="composer-message">see @src/ma</textarea>
        </form>`;
    const field =
        document.querySelector<HTMLTextAreaElement>("#composer-message")!;
    field.focus();
    field.setSelectionRange(field.value.length, field.value.length);
    field.dispatchEvent(new Event("input", { bubbles: true }));
}

function suggestionResponse(path: string): Response {
    return {
        json: async () => ({
            suggestions: [{ path, scope: "project", directory: false }],
            message: "",
        }),
    } as unknown as Response;
}

test("at-sign lookup quotes a path with spaces at the caret", async () => {
    const fetchMock = vi.fn(async () =>
        suggestionResponse("/access/project/src/my file.rs"),
    );
    vi.stubGlobal("fetch", fetchMock);
    fileLookupMarkup();
    await new Promise((resolve) => setTimeout(resolve, 200));
    expect(fetchMock).toHaveBeenCalledOnce();
    const field =
        document.querySelector<HTMLTextAreaElement>("#composer-message")!;
    field.dispatchEvent(
        new KeyboardEvent("keydown", {
            key: "Enter",
            bubbles: true,
            cancelable: true,
        }),
    );
    expect(field.value).toBe('see "/access/project/src/my file.rs" ');
    expect(
        document.querySelector<HTMLElement>("[data-file-suggestions]")!.hidden,
    ).toBe(true);
});

test("lookup discards a response after draft scope changes", async () => {
    let complete!: (response: Response) => void;
    vi.stubGlobal(
        "fetch",
        vi.fn(
            () =>
                new Promise<Response>((resolve) => {
                    complete = resolve;
                }),
        ),
    );
    fileLookupMarkup();
    await new Promise((resolve) => setTimeout(resolve, 200));
    document.querySelector<HTMLInputElement>(
        'input[name="draft_nonce"]',
    )!.value = "b".repeat(64);
    complete(suggestionResponse("/access/old/secret.rs"));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(
        document.querySelector<HTMLElement>("[data-file-suggestions]")!.hidden,
    ).toBe(true);
});

test("escape cancels a pending lookup before dispatch", async () => {
    const fetchMock = vi.fn(async () =>
        suggestionResponse("/access/project/main.rs"),
    );
    vi.stubGlobal("fetch", fetchMock);
    fileLookupMarkup();
    document.querySelector("#composer-message")!.dispatchEvent(
        new KeyboardEvent("keydown", {
            key: "Escape",
            bubbles: true,
            cancelable: true,
        }),
    );
    await new Promise((resolve) => setTimeout(resolve, 200));
    expect(fetchMock).not.toHaveBeenCalled();
});

test("escape closes suggestions without changing the draft", async () => {
    vi.stubGlobal(
        "fetch",
        vi.fn(async () => suggestionResponse("/access/project/src/main.rs")),
    );
    fileLookupMarkup();
    await new Promise((resolve) => setTimeout(resolve, 200));
    const field =
        document.querySelector<HTMLTextAreaElement>("#composer-message")!;
    const before = field.value;
    field.dispatchEvent(
        new KeyboardEvent("keydown", {
            key: "Escape",
            bubbles: true,
            cancelable: true,
        }),
    );
    expect(field.value).toBe(before);
    expect(
        document.querySelector<HTMLElement>("[data-file-suggestions]")!.hidden,
    ).toBe(true);
});

function completionResponse(path: string, directory: boolean): Response {
    return {
        json: async () => ({
            suggestions: [{ path, scope: "project", directory }],
            message: "",
        }),
    } as unknown as Response;
}

function composerMarkup(value: string, caret: number): HTMLTextAreaElement {
    document.body.innerHTML = `
        <template data-conversation-state="new"></template>
        <div data-file-suggestions hidden></div>
        <form id="conversation-composer">
            <input type="hidden" name="draft_nonce" value="${"a".repeat(64)}">
            <textarea id="composer-message"></textarea>
        </form>`;
    const field =
        document.querySelector<HTMLTextAreaElement>("#composer-message")!;
    field.value = value;
    field.focus();
    field.setSelectionRange(caret, caret);
    field.dispatchEvent(new Event("input", { bubbles: true }));
    return field;
}

function press(field: HTMLTextAreaElement, key: string, shiftKey = false) {
    const event = new KeyboardEvent("keydown", {
        key,
        shiftKey,
        bubbles: true,
        cancelable: true,
    });
    field.dispatchEvent(event);
    return event;
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 200));

test("tab completes only the token at the caret", async () => {
    vi.stubGlobal(
        "fetch",
        vi.fn(async () =>
            completionResponse("/access/project/src/main.rs", false),
        ),
    );
    const field = composerMarkup(
        "before @src/ma after",
        "before @src/ma".length,
    );
    await settle();
    const event = press(field, "Tab");
    expect(event.defaultPrevented).toBe(true);
    expect(field.value).toBe("before /access/project/src/main.rs  after");
});

test("completion replaces a quoted token without touching its suffix", async () => {
    vi.stubGlobal(
        "fetch",
        vi.fn(async () =>
            completionResponse("/access/project/src/main.rs", false),
        ),
    );
    const value = 'see @"/access/project/src/main old file" tail';
    const caret = value.indexOf("main") + 2;
    const field = composerMarkup(value, caret);
    await settle();
    const event = press(field, "Tab");
    expect(event.defaultPrevented).toBe(true);
    expect(field.value).toBe("see /access/project/src/main.rs  tail");
    const url = new URL(
        vi.mocked(fetch).mock.calls[0][0] as string,
        "http://localhost",
    );
    expect(url.searchParams.get("q")).toBe("/access/project/src/ma");
    expect(url.searchParams.get("mode")).toBe("complete");
});

test("an unclosed path quote does not consume the next line", async () => {
    vi.stubGlobal(
        "fetch",
        vi.fn(async () => completionResponse("/access/project/my dir/", true)),
    );
    const field = composerMarkup(
        'open @"my dir\nkeep this line',
        'open @"my dir'.length,
    );
    await settle();
    press(field, "Tab");
    expect(field.value).toBe('open @"/access/project/my dir/"\nkeep this line');
    await settle();
});

test("tab completes a directory and keeps the caret inside the quotes", async () => {
    vi.stubGlobal(
        "fetch",
        vi.fn(async () => completionResponse("/access/project/my dir/", true)),
    );
    const value = 'open @"my ';
    const field = composerMarkup(value, value.length);
    await settle();
    press(field, "Tab");
    const insert = '"/access/project/my dir/"';
    expect(field.value).toBe(`open @${insert}`);
    expect(field.selectionStart).toBe("open ".length + insert.length);
    await settle();
});

test("shift+tab keeps ordinary focus traversal", async () => {
    vi.stubGlobal(
        "fetch",
        vi.fn(async () =>
            completionResponse("/access/project/src/main.rs", false),
        ),
    );
    const field = composerMarkup("see @src/ma", "see @src/ma".length);
    await settle();
    const event = press(field, "Tab", true);
    expect(event.defaultPrevented).toBe(false);
    expect(field.value).toBe("see @src/ma");
});

function commandMarkup(): HTMLTextAreaElement {
    for (const listener of locations) listener({} as never);
    document.body.innerHTML = `<template data-conversation-state="new"></template>
        <div data-command-suggestions hidden></div>
        <form id="conversation-composer">
            <input type="hidden" name="draft_nonce" value="draft-one">
            <input type="hidden" name="command_source">
            <input type="hidden" name="command_hash">
            <textarea id="composer-message">/skill:probe </textarea>
            <button type="submit">Send</button>
        </form>`;
    const field =
        document.querySelector<HTMLTextAreaElement>("#composer-message")!;
    field.focus();
    field.setSelectionRange(field.value.length, field.value.length);
    field.dispatchEvent(new Event("input", { bubbles: true }));
    return field;
}

function commandResponse(): Response {
    return {
        json: async () => ({
            preview: {
                command: "/skill:probe",
                binding: "source-identity",
                hash: "old-hash",
                source: "/skills/probe/SKILL.md",
                expanded: "Frozen body",
                base: "/skills/probe",
                scope: "global",
            },
        }),
    } as Response;
}

test("skill preview survives trailing edits, caret movement and a pointer Send", async () => {
    const fetchMock = vi.fn(async () => commandResponse());
    vi.stubGlobal("fetch", fetchMock);
    const field = commandMarkup();
    await settle();
    field.value += "inspect this";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    field.setSelectionRange(2, 2);
    document.dispatchEvent(new Event("selectionchange"));
    document
        .querySelector("button")!
        .dispatchEvent(new Event("pointerdown", { bubbles: true }));
    document
        .querySelector("form")!
        .dispatchEvent(
            new Event("submit", { bubbles: true, cancelable: true }),
        );
    expect(value("command_hash")).toBe("old-hash");
    expect(value("command_source")).toBe("source-identity");
    expect(fetchMock).toHaveBeenCalledOnce();
});

test("template preview includes arguments and refreshes after argument edits", async () => {
    const fetchMock = vi.fn(async (url: string) => {
        const query = new URL(url, "http://localhost").searchParams.get("q");
        return {
            json: async () => ({
                preview: {
                    command: query,
                    kind: "prompt",
                    binding: "prompt-source",
                    hash: "prompt-hash",
                    source: "prompts/review.md",
                    expanded: query,
                    base: "",
                    scope: "Global prompts",
                },
            }),
        } as Response;
    });
    vi.stubGlobal("fetch", fetchMock);
    const field = commandMarkup();
    for (const typed of ['/review "a b"', '/review "c d"']) {
        field.value = typed;
        field.setSelectionRange(typed.length, typed.length);
        field.dispatchEvent(new Event("input", { bubbles: true }));
        await settle();
        expect(
            document.querySelector("[data-command-suggestions] pre")
                ?.textContent,
        ).toBe(typed);
        expect(value("command_hash")).toBe("prompt-hash");
    }
    expect(fetchMock).toHaveBeenCalledTimes(2);
});

test("skill preview discards a response from another draft scope", async () => {
    let complete!: (response: Response) => void;
    vi.stubGlobal(
        "fetch",
        vi.fn(
            () =>
                new Promise<Response>((resolve) => {
                    complete = resolve;
                }),
        ),
    );
    commandMarkup();
    await settle();
    document.querySelector<HTMLInputElement>(
        'input[name="draft_nonce"]',
    )!.value = "draft-two";
    complete(commandResponse());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(value("command_hash")).toBe("");
    expect(
        document.querySelector<HTMLElement>("[data-command-suggestions]")!
            .hidden,
    ).toBe(true);
});

test("tab without a completion keeps ordinary focus traversal", async () => {
    vi.stubGlobal(
        "fetch",
        vi.fn(
            async () =>
                ({
                    json: async () => ({
                        suggestions: [],
                        message: "",
                    }),
                }) as unknown as Response,
        ),
    );
    const field = composerMarkup("see @src/ma", "see @src/ma".length);
    await settle();
    const event = press(field, "Tab");
    expect(event.defaultPrevented).toBe(false);
    expect(field.value).toBe("see @src/ma");
});
