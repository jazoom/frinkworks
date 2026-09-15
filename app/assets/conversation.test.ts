// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { initConversation } from "./conversation";

describe.each(["new", "saved"])("%s conversation", (state) => {
    let controller: AbortController;
    let island: ReturnType<typeof initConversation>;

    afterEach(() => {
        controller.abort();
        island.destroy?.();
        vi.unstubAllGlobals();
    });

    beforeEach(() => {
        // happy-dom does not supply the browser Option constructor.
        vi.stubGlobal("Option", function (text: string, value: string) {
            const option = document.createElement("option");
            option.text = text;
            option.value = value;
            return option;
        });
        document.body.innerHTML = `
        <section id="conversation-detail">
        <template data-conversation-state="${state}"></template>
        <span data-conversation-model-summary>Alpha</span>
        <form id="conversation-composer">
            <select name="provider"><option value="one">One</option><option value="two" data-default-model="Other">Two</option></select>
            <input id="conversation-model" type="hidden" name="model" value="Alpha">
            <select name="thinking"><option>low</option><option selected>high</option></select>
            <textarea name="message">Unsent text</textarea>
        </form>
        <div id="conversation-model-picker">
            <button id="conversation-model-toggle" type="button" aria-expanded="false"><span id="conversation-model-value">Alpha</span></button>
            <div id="conversation-model-options" hidden>
                <input id="conversation-model-search" type="search">
                <p id="conversation-model-search-status" role="status"></p>
                <div id="conversation-model-results"></div>
            </div>
        </div>
        <template data-conversation-model-catalogue></template>
        </section>
    `;
        document.querySelector<HTMLElement>(
            "[data-conversation-model-catalogue]",
        )!.dataset.conversationModelCatalogue = JSON.stringify({
            one: [
                {
                    id: "Alpha",
                    default_effort: "low",
                    efforts: [
                        { value: "low", label: "Low" },
                        { value: "high", label: "High" },
                    ],
                },
                { id: "Beta", default_effort: "", efforts: [] },
            ],
            two: [
                {
                    id: "Other",
                    default_effort: "low",
                    efforts: [{ value: "low", label: "Low" }],
                },
            ],
        });
        const root = document.querySelector("#conversation-detail")!;
        if (state === "saved") {
            root.insertAdjacentHTML(
                "beforeend",
                '<form id="conversation-settings-form"><input name="revision" value="3"></form>',
            );
        }
        for (const name of ["provider", "model", "thinking"]) {
            const control = root.querySelector(`[name="${name}"]`)!;
            control.remove();
            control.setAttribute(
                "form",
                state === "new"
                    ? "conversation-composer"
                    : "conversation-settings-form",
            );
            root.append(control);
        }
        controller = new AbortController();
        island = initConversation(
            document.querySelector<HTMLElement>("#conversation-detail")!,
            { signal: controller.signal },
        );
    });

    function select(name: string, value: string) {
        const control = document.querySelector<HTMLSelectElement>(
            `[name="${name}"]`,
        )!;
        control.value = value;
        control.dispatchEvent(new Event("change", { bubbles: true }));
    }

    function search(value: string) {
        const input = document.querySelector<HTMLInputElement>(
            "#conversation-model-search",
        )!;
        input.value = value;
        input.dispatchEvent(new Event("input", { bubbles: true }));
    }

    function models() {
        return Array.from(
            document.querySelectorAll<HTMLButtonElement>(
                "[data-conversation-model-value]",
            ),
            (option) => option.dataset.conversationModelValue,
        );
    }

    function value(name: string) {
        const control = document.querySelector<HTMLInputElement>(
            `[name="${name}"]`,
        )!;
        return new FormData(control.form!).get(name);
    }

    test("search normalises input and preserves submitted choices and unsent text", () => {
        search("  ALP  ");
        expect(models()).toEqual(["Alpha"]);
        search("<no matches>");
        expect(models()).toEqual([]);
        expect(document.querySelector("[role=status]")!.textContent).toBe(
            "No matching models. Clear the search to see all models.",
        );
        expect(value("model")).toBe("Alpha");
        expect(value("thinking")).toBe("high");
        expect(value("message")).toBe("Unsent text");
        search("");
        expect(models()).toEqual(["Alpha", "Beta"]);
        expect(value("thinking")).toBe("high");
    });

    test("a filtered model change updates effort and a provider change clears the filter", () => {
        search("beta");
        document
            .querySelector<HTMLButtonElement>(
                '[data-conversation-model-value="Beta"]',
            )!
            .click();
        expect(models()).toEqual(["Beta"]);
        expect(
            document.querySelector<HTMLSelectElement>('[name="thinking"]')!
                .disabled,
        ).toBe(true);
        select("provider", "two");
        expect(
            document.querySelector<HTMLInputElement>(
                "#conversation-model-search",
            )!.value,
        ).toBe("");
        expect(models()).toEqual(["Other"]);
        expect(value("model")).toBe("Other");
        expect(value("thinking")).toBe("low");
        expect(document.querySelector("[role=status]")!.textContent).toBe(
            "1 model.",
        );
    });

    function key(target: Element, value: string) {
        target.dispatchEvent(
            new KeyboardEvent("keydown", {
                key: value,
                bubbles: true,
                cancelable: true,
            }),
        );
    }

    test("keyboard navigation and Escape preserve the model and effort without a submission", () => {
        const submit = vi.fn();
        document.querySelector("form")!.addEventListener("submit", submit);
        const toggle = document.querySelector<HTMLButtonElement>(
            "#conversation-model-toggle",
        )!;
        toggle.click();
        const input = document.querySelector<HTMLInputElement>(
            "#conversation-model-search",
        )!;
        expect(document.activeElement).toBe(input);
        expect(toggle.ariaExpanded).toBe("true");
        search("b");
        key(input, "ArrowDown");
        expect(
            (document.activeElement as HTMLElement).dataset
                .conversationModelValue,
        ).toBe("Beta");
        key(document.activeElement!, "ArrowUp");
        expect(document.activeElement).toBe(input);
        key(input, "Escape");
        expect(document.activeElement).toBe(toggle);
        expect(toggle.ariaExpanded).toBe("false");
        expect(
            document.querySelector<HTMLElement>("#conversation-model-options")!
                .hidden,
        ).toBe(true);
        expect(value("model")).toBe("Alpha");
        expect(value("thinking")).toBe("high");
        expect(submit).not.toHaveBeenCalled();
        toggle.click();
        expect(input.value).toBe("");
        expect(models()).toEqual(["Alpha", "Beta"]);
    });

    test("Enter selects only an exact or single result, not arbitrary search text", () => {
        const toggle = document.querySelector<HTMLButtonElement>(
            "#conversation-model-toggle",
        )!;
        toggle.click();
        const input = document.querySelector<HTMLInputElement>(
            "#conversation-model-search",
        )!;
        search("no match");
        key(input, "Enter");
        expect(value("model")).toBe("Alpha");
        expect(toggle.ariaExpanded).toBe("true");
        search("  ALPHA ");
        key(input, "Enter");
        expect(value("thinking")).toBe("high");
        toggle.click();
        search("bet");
        key(input, "Enter");
        expect(value("model")).toBe("Beta");
        expect(
            document.querySelector("#conversation-model-value")!.textContent,
        ).toBe("Beta");
        expect(toggle.ariaExpanded).toBe("false");
        expect(document.activeElement).toBe(toggle);
    });

    test.each(["conversation-settings-form", "conversation-preset-apply-form"])(
        "unrelated patches retain unsaved settings but accept %s responses",
        (responseFormId) => {
            const root = document.querySelector<HTMLElement>(
                "#conversation-detail",
            )!;
            const formId =
                state === "new"
                    ? "conversation-composer"
                    : "conversation-settings-form";
            root.insertAdjacentHTML(
                "beforeend",
                `<textarea name="instructions" form="${formId}"></textarea><input type="checkbox" name="tool_read" value="read" form="${formId}">
            <input type="radio" name="network" value="none" checked form="${formId}">
            <input type="radio" name="network" value="restricted" form="${formId}">
            <textarea name="network_domains" form="${formId}"></textarea>
            <select name="environment" form="${formId}"><option value="starter">Starter</option><option value="custom">Custom</option></select>`,
            );
            const original = root.innerHTML;
            const instructions = root.querySelector<HTMLTextAreaElement>(
                '[name="instructions"]',
            )!;
            instructions.value = "Keep my draft";
            instructions.dispatchEvent(new Event("input", { bubbles: true }));
            const read =
                root.querySelector<HTMLInputElement>('[name="tool_read"]')!;
            read.checked = true;
            read.dispatchEvent(new Event("input", { bubbles: true }));
            const network = root.querySelector<HTMLInputElement>(
                '[name="network"][value="restricted"]',
            )!;
            network.checked = true;
            network.dispatchEvent(new Event("input", { bubbles: true }));
            const domains = root.querySelector<HTMLTextAreaElement>(
                '[name="network_domains"]',
            )!;
            domains.value = "example.com";
            domains.dispatchEvent(new Event("input", { bubbles: true }));
            select("environment", "custom");
            select("provider", "two");
            const unrelated = document.createElement("form");
            unrelated.id = "conversation-rename";
            root.innerHTML = original;
            const detail = {
                requestKind: "patch" as const,
                form: unrelated,
                url: "/conversations/example/rename",
                outcome: "applied-patch" as const,
                status: 200 as const,
                targetIds: ["conversation-detail"],
            };
            island.reconcile?.({ cause: "patch", detail });
            expect(value("network")).toBe("restricted");
            expect(value("environment")).toBe("custom");
            expect(value("network_domains")).toBe("example.com");
            expect(value("instructions")).toBe("Keep my draft");
            expect(value("tool_read")).toBe("read");
            expect(value("provider")).toBe("two");
            expect(value("model")).toBe("Other");
            expect(value("thinking")).toBe("low");
            const submitted = root.querySelector<HTMLFormElement>(
                `#${formId}`,
            )!;
            if (state !== "new") submitted.id = responseFormId;
            root.innerHTML = original;
            island.reconcile?.({
                cause: "patch",
                detail: { ...detail, form: submitted },
            });
            expect(value("instructions")).toBe("");
            expect(value("environment")).toBe("starter");
            expect(value("network")).toBe("none");
            expect(value("network_domains")).toBe("");
            expect(value("tool_read")).toBeNull();
            expect(value("model")).toBe("Alpha");
        },
    );

    test("a validation patch retains server choices and the island stops after abort", () => {
        const root = document.querySelector<HTMLElement>(
            "#conversation-detail",
        )!;
        root.innerHTML = root.innerHTML;
        root.querySelector<HTMLInputElement>("#conversation-model")!.value =
            "Beta";
        root.querySelector("[data-conversation-model-summary]")!.textContent =
            "Beta";
        const thinking =
            root.querySelector<HTMLSelectElement>('[name="thinking"]')!;
        thinking.replaceChildren(new Option("Not available", ""));
        thinking.disabled = true;
        island.reconcile?.({
            cause: "patch",
            detail: {
                requestKind: "patch",
                form: root.querySelector("form")!,
                url: "/conversations/new",
                outcome: "applied-patch",
                status: 422,
                targetIds: ["conversation-detail"],
            },
        });
        expect(
            root.querySelector("[data-conversation-model-summary]")!
                .textContent,
        ).toBe("Beta");
        expect(value("model")).toBe("Beta");
        expect(thinking.disabled).toBe(true);
        expect(value("message")).toBe("Unsent text");
        root.querySelector<HTMLButtonElement>(
            "#conversation-model-toggle",
        )!.click();
        search("Alpha");
        root.querySelector<HTMLButtonElement>(
            "[data-conversation-model-value]",
        )!.click();
        expect(value("model")).toBe("Alpha");
        expect(value("thinking")).toBe("low");
        controller.abort();
        search("Beta");
        expect(models()).toEqual(["Alpha"]);
    });

    test("provider input alone does not change the model or submit a command", () => {
        const submit = vi.fn();
        document
            .querySelector("#conversation-detail")!
            .addEventListener("submit", submit);
        const provider =
            document.querySelector<HTMLSelectElement>('[name="provider"]')!;
        provider.value = "two";
        provider.dispatchEvent(new Event("input", { bubbles: true }));
        expect(value("model")).toBe("Alpha");
        provider.dispatchEvent(new Event("change", { bubbles: true }));
        expect(value("model")).toBe("Other");
        expect(submit).not.toHaveBeenCalled();
        expect(
            document.querySelector("[data-conversation-model-summary]")!
                .textContent,
        ).toBe(state === "new" ? "Other" : "Alpha");
        if (state === "saved") expect(value("revision")).toBe("3");
    });

    if (state === "new")
        test("the retained island adopts saved form ownership after the first-message patch", () => {
            const root = document.querySelector<HTMLElement>(
                "#conversation-detail",
            )!;
            const composer = root.querySelector<HTMLFormElement>(
                "#conversation-composer",
            )!;
            root.querySelector<HTMLElement>(
                "[data-conversation-state]",
            )!.dataset.conversationState = "saved";
            root.insertAdjacentHTML(
                "beforeend",
                '<form id="conversation-settings-form"><input name="revision" value="2"></form>',
            );
            for (const name of ["provider", "model", "thinking"]) {
                root.querySelector(`[name="${name}"]`)!.setAttribute(
                    "form",
                    "conversation-settings-form",
                );
            }
            island.reconcile?.({
                cause: "patch",
                detail: {
                    requestKind: "patch",
                    form: composer,
                    url: "/conversations/new",
                    outcome: "applied-patch",
                    status: 200,
                    targetIds: ["conversation-detail"],
                },
            });
            search("Beta");
            root.querySelector<HTMLButtonElement>(
                "[data-conversation-model-value]",
            )!.click();
            expect(new FormData(composer).has("model")).toBe(false);
            expect(value("model")).toBe("Beta");
            expect(value("revision")).toBe("2");
            expect(
                document.querySelector("[data-conversation-model-summary]")!
                    .textContent,
            ).toBe("Alpha");
        });
});
