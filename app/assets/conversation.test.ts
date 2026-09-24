// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { initConversation } from "./conversation";

describe.each(["new", "saved"])("%s conversation settings", (state) => {
    let controller: AbortController;
    let island: ReturnType<typeof initConversation>;
    let root: HTMLElement;
    const formId =
        state === "new"
            ? "conversation-composer"
            : "conversation-settings-form";

    beforeEach(() => {
        document.body.innerHTML = `<section id="conversation-detail">
            <template data-conversation-state="${state}"></template>
            <form id="${formId}">
                <input name="revision" value="3">
                <input name="provider" value="xai">
                <input name="model" value="Alpha">
                <span id="conversation-model-value">Alpha</span>
                <input name="thinking" value="high">
                <textarea id="conversation-instructions" name="instructions"></textarea>
                <input type="checkbox" data-enable-tools>
                <input name="tool_read" type="checkbox" value="read" data-tool-field>
                <input name="tool_edit" type="checkbox" value="edit" data-tool-field>
                <p data-instructions-status></p>
                <input name="network" type="radio" value="none" checked>
                <input name="network" type="radio" value="restricted">
                <textarea name="network_domains"></textarea>
                <input name="location" type="radio" value="sandbox" checked>
                <input name="location" type="radio" value="host">
                <input name="directory_access" value="">
            </form>
        </section>`;
        root = document.querySelector<HTMLElement>("#conversation-detail")!;
        controller = new AbortController();
        island = initConversation(root, { signal: controller.signal });
    });

    afterEach(() => {
        controller.abort();
        island.destroy?.();
        document.body.replaceChildren();
    });

    function value(name: string) {
        return new FormData(root.querySelector("form")!).get(name);
    }

    function edit() {
        const instructions = root.querySelector<HTMLTextAreaElement>(
            '[name="instructions"]',
        )!;
        instructions.value = "Keep my draft";
        const read =
            root.querySelector<HTMLInputElement>('[name="tool_read"]')!;
        read.checked = true;
        root.querySelector<HTMLInputElement>('[name="tool_edit"]')!.checked =
            true;
        const network = root.querySelector<HTMLInputElement>(
            '[name="network"][value="restricted"]',
        )!;
        network.checked = true;
        const domains = root.querySelector<HTMLTextAreaElement>(
            '[name="network_domains"]',
        )!;
        domains.value = "example.com";
        instructions.dispatchEvent(new Event("input", { bubbles: true }));
    }

    function reconcile(
        id: string,
        status: 200 | 409 | 422 = 200,
        url = "/conversations/example",
    ) {
        const form = document.createElement("form");
        form.id = id;
        island.reconcile?.({
            cause: "patch",
            detail: {
                requestKind: "patch",
                form,
                url,
                outcome: "applied-patch",
                status,
                targetIds: ["conversation-detail"],
            },
        });
    }

    test("unrelated patches retain requested authority without applying it", () => {
        const original = root.innerHTML;
        edit();
        root.innerHTML = original;
        reconcile("conversation-rename");
        expect(value("instructions")).toBe("Keep my draft");
        expect(value("tool_read")).toBe("read");
        expect(value("tool_edit")).toBe("edit");
        expect(value("network")).toBe("restricted");
        expect(value("network_domains")).toBe("example.com");
        root.innerHTML = original;
        reconcile(formId);
        expect(value("instructions")).toBe("");
        expect(value("tool_read")).toBeNull();
        expect(value("network")).toBe(
            state === "saved" ? "restricted" : "none",
        );
    });

    test("bulk tool selection preserves explicit empty and partial choices through patches", () => {
        const form = root.querySelector<HTMLFormElement>("form")!;
        form.requestSubmit = () => {};
        const original = root.innerHTML;
        const read =
            root.querySelector<HTMLInputElement>('[name="tool_read"]')!;
        read.checked = true;
        read.dispatchEvent(new Event("change", { bubbles: true }));
        const toggle = root.querySelector<HTMLInputElement>(
            "[data-enable-tools]",
        )!;
        expect(toggle.indeterminate).toBe(true);
        toggle.checked = true;
        toggle.dispatchEvent(new Event("change", { bubbles: true }));
        expect(value("tool_read")).toBe("read");
        expect(value("tool_edit")).toBe("edit");
        toggle.checked = false;
        toggle.dispatchEvent(new Event("change", { bubbles: true }));
        root.innerHTML = original;
        reconcile("conversation-rename");
        expect(value("tool_read")).toBeNull();
        expect(value("tool_edit")).toBeNull();
        expect(
            root.querySelector<HTMLInputElement>("[data-enable-tools]")!
                .checked,
        ).toBe(false);
    });

    test.each([200, 409, 422] as const)(
        "preset replacement at status %s preserves the confirmation boundary",
        (status) => {
            const original = root.innerHTML;
            edit();
            root.querySelector("form")!.dispatchEvent(
                new Event("submit", { bubbles: true }),
            );
            const field = root.querySelector<HTMLTextAreaElement>(
                '[name="instructions"]',
            )!;
            field.value = "Later edit";
            field.dispatchEvent(new Event("input", { bubbles: true }));
            root.innerHTML = original;
            reconcile(
                state === "new" ? formId : "conversation-preset-apply-form",
                status,
                "/conversations/example/settings/presets/apply",
            );
            expect(value("instructions")).toBe(
                status === 200 ? "" : "Later edit",
            );
            expect(value("network")).toBe(
                status === 200 ? "none" : "restricted",
            );
            expect(value("tool_read")).toBe(status === 200 ? null : "read");
        },
    );

    if (state === "new") {
        test("draft preview responses retain instruction and tool edits after submission", () => {
            const original = root.innerHTML;
            const form = root.querySelector<HTMLFormElement>("form")!;
            edit();
            form.dispatchEvent(new Event("submit", { bubbles: true }));
            const instructions = root.querySelector<HTMLTextAreaElement>(
                '[name="instructions"]',
            )!;
            instructions.value = "Later instructions";
            const read =
                root.querySelector<HTMLInputElement>('[name="tool_read"]')!;
            read.checked = false;
            instructions.dispatchEvent(new Event("input", { bubbles: true }));
            root.innerHTML = original;
            reconcile("conversation-composer");
            expect(value("instructions")).toBe("Later instructions");
            expect(value("tool_read")).toBeNull();
            expect(
                root.querySelector("#conversation-model-value")!.textContent,
            ).toBe("Alpha");
        });
    }

    if (state === "saved") {
        test.each(["preview", "save"])(
            "preset %s retains unreviewed execution settings",
            (command) => {
                const original = root.innerHTML;
                edit();
                root.innerHTML = original;
                reconcile(
                    command === "save"
                        ? "conversation-preset-save-form"
                        : "conversation-preset-form",
                    200,
                    `/conversations/example/settings/presets/${command}`,
                );
                expect(value("instructions")).toBe("Keep my draft");
                expect(value("network")).toBe("restricted");
                expect(value("network_domains")).toBe("example.com");
            },
        );

        test("a rejected save retains later edits without an automatic retry", () => {
            const original = root.innerHTML;
            let submits = 0;
            const form = root.querySelector<HTMLFormElement>("form")!;
            form.requestSubmit = () => {
                submits++;
            };
            edit();
            const field = root.querySelector<HTMLTextAreaElement>(
                '[name="instructions"]',
            )!;
            field.dispatchEvent(new Event("focusout", { bubbles: true }));
            field.value = "Newer draft";
            field.dispatchEvent(new Event("input", { bubbles: true }));
            field.dispatchEvent(new Event("focusout", { bubbles: true }));
            root.innerHTML = original;
            root.querySelector<HTMLFormElement>("form")!.requestSubmit = () => {
                submits++;
            };
            reconcile("conversation-settings-form", 422);
            expect(value("instructions")).toBe("Newer draft");
            expect(value("tool_read")).toBe("read");
            expect(submits).toBe(1);
            expect(
                root.querySelector("[data-instructions-status]")!.textContent,
            ).toContain("not saved");
        });

        test("an uncertain save retains the draft and never reports success", () => {
            const form = root.querySelector<HTMLFormElement>("form")!;
            form.requestSubmit = () => {};
            edit();
            root.querySelector<HTMLTextAreaElement>(
                '[name="instructions"]',
            )!.dispatchEvent(new Event("focusout", { bubbles: true }));
            island.reconcile?.({
                cause: "patch",
                detail: {
                    requestKind: "patch",
                    form,
                    url: "/conversations/example/settings",
                    outcome: "uncertain-unsafe-result",
                },
            });
            expect(value("instructions")).toBe("Keep my draft");
            expect(
                root.querySelector("[data-instructions-status]")!.textContent,
            ).toContain("unknown");
        });

        test("model responses retain unsaved authority fields but accept the new model and revision", () => {
            const original = root.innerHTML;
            edit();
            root.innerHTML = original;
            root.querySelector<HTMLInputElement>('[name="revision"]')!.value =
                "4";
            root.querySelector<HTMLInputElement>('[name="model"]')!.value =
                "Beta";
            reconcile("conversation-model-form");
            expect(value("instructions")).toBe("Keep my draft");
            expect(value("network")).toBe("restricted");
            expect(value("model")).toBe("Beta");
            expect(value("revision")).toBe("4");
            root.innerHTML = original;
            reconcile("conversation-rename");
            expect(value("revision")).toBe("4");
        });

        test("a command-directory response retains staged access but accepts the new revision", () => {
            root.querySelector<HTMLInputElement>(
                '[name="directory_access"]',
            )!.id = "execution-directory-access";
            root.insertAdjacentHTML(
                "beforeend",
                `
                <input type="radio" name="access-one" data-execution-directory="one" value="read-only" checked>
                <input type="radio" name="access-one" data-execution-directory="one" value="direct-write">`,
            );
            const original = root.innerHTML;
            const access = root.querySelector<HTMLInputElement>(
                "#execution-directory-access",
            )!;
            access.value = JSON.stringify([["one", "direct-write"]]);
            access.dispatchEvent(new Event("input", { bubbles: true }));
            root.innerHTML = original;
            root.querySelector<HTMLInputElement>('[name="revision"]')!.value =
                "4";
            reconcile("command-directory-form");
            expect(
                root.querySelector<HTMLInputElement>(
                    '[data-execution-directory][value="direct-write"]',
                )!.checked,
            ).toBe(true);
            expect(
                root.querySelector<HTMLInputElement>(
                    '[data-execution-directory][value="direct-write"]',
                )!.defaultChecked,
            ).toBe(false);
            expect(value("revision")).toBe("4");
            root.innerHTML = original;
            reconcile("conversation-rename");
            expect(value("revision")).toBe("4");
        });

        test("ordinary saved fields submit without dirty execution values", () => {
            const form = root.querySelector<HTMLFormElement>("form")!;
            const submitted: Array<{
                location: FormDataEntryValue | null;
                network: FormDataEntryValue | null;
            }> = [];
            form.requestSubmit = () => {
                const data = new FormData(form);
                submitted.push({
                    location: data.get("location"),
                    network: data.get("network"),
                });
            };
            const host = root.querySelector<HTMLInputElement>(
                '[name="location"][value="host"]',
            )!;
            host.checked = true;
            host.dispatchEvent(new Event("input", { bubbles: true }));
            const network = root.querySelector<HTMLInputElement>(
                '[name="network"][value="restricted"]',
            )!;
            network.checked = true;
            network.dispatchEvent(new Event("change", { bubbles: true }));
            expect(submitted).toEqual([]);
            const tool =
                root.querySelector<HTMLInputElement>('[name="tool_read"]')!;
            tool.checked = true;
            tool.dispatchEvent(new Event("change", { bubbles: true }));
            expect(submitted).toEqual([
                { location: "sandbox", network: "none" },
            ]);
            expect(value("network")).toBe("restricted");
            expect(
                root.querySelector<HTMLInputElement>(
                    '[name="location"][value="host"]',
                )!.checked,
            ).toBe(true);
        });

        test.each(["read", "edit"])(
            "a queued %s revocation survives the earlier save response",
            (tool) => {
                const name = `tool_${tool}`;
                const original = root.innerHTML;
                const form = root.querySelector<HTMLFormElement>("form")!;
                const submitted: Array<FormDataEntryValue | null> = [];
                form.requestSubmit = () =>
                    submitted.push(new FormData(form).get(name));
                const field = form.elements.namedItem(name) as HTMLInputElement;
                field.checked = true;
                field.dispatchEvent(new Event("change", { bubbles: true }));
                field.checked = false;
                field.dispatchEvent(new Event("change", { bubbles: true }));
                root.innerHTML = original;
                const updated = root.querySelector<HTMLFormElement>("form")!;
                (updated.elements.namedItem(name) as HTMLInputElement).checked =
                    true;
                updated.requestSubmit = () =>
                    submitted.push(new FormData(updated).get(name));
                reconcile("conversation-settings-form");
                expect(submitted).toEqual([tool, null]);
                expect(value(name)).toBeNull();
            },
        );

        test("settings patches keep uncommitted execution fields", () => {
            const original = root.innerHTML;
            const host = root.querySelector<HTMLInputElement>(
                '[name="location"][value="host"]',
            )!;
            host.checked = true;
            host.dispatchEvent(new Event("input", { bubbles: true }));
            edit();
            root.innerHTML = original;
            reconcile("conversation-settings-form");
            expect(value("instructions")).toBe("");
            expect(value("location")).toBe("host");
        });
    }
});
