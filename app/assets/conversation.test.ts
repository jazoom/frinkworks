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
                <input name="thinking" value="high">
                <textarea name="instructions"></textarea>
                <input name="tool_read" type="checkbox" value="read">
                <input name="network" type="radio" value="none" checked>
                <input name="network" type="radio" value="restricted">
                <textarea name="network_domains"></textarea>
                <input name="location" type="radio" value="sandbox" checked>
                <input name="location" type="radio" value="host">
                <input name="directory_access" value="">
                <select name="git_destination"><option value="">None</option><option value="repository">Repository</option></select>
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

    function reconcile(id: string) {
        const form = document.createElement("form");
        form.id = id;
        island.reconcile?.({
            cause: "patch",
            detail: {
                requestKind: "patch",
                form,
                url: "/conversations/example",
                outcome: "applied-patch",
                status: 200,
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
        expect(value("network")).toBe("restricted");
        expect(value("network_domains")).toBe("example.com");
        root.innerHTML = original;
        reconcile(formId);
        expect(value("instructions")).toBe("");
        expect(value("tool_read")).toBeNull();
        expect(value("network")).toBe("none");
    });

    if (state === "saved") {
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
            expect(submitted).toEqual([
                { location: "sandbox", network: "restricted" },
            ]);
            expect(
                root.querySelector<HTMLInputElement>(
                    '[name="location"][value="host"]',
                )!.checked,
            ).toBe(true);
        });

        test("a queued tool revocation survives the earlier save response", () => {
            const original = root.innerHTML;
            const form = root.querySelector<HTMLFormElement>("form")!;
            const submitted: Array<FormDataEntryValue | null> = [];
            form.requestSubmit = () =>
                submitted.push(new FormData(form).get("tool_read"));
            const read = form.elements.namedItem(
                "tool_read",
            ) as HTMLInputElement;
            read.checked = true;
            read.dispatchEvent(new Event("change", { bubbles: true }));
            read.checked = false;
            read.dispatchEvent(new Event("change", { bubbles: true }));
            root.innerHTML = original;
            const updated = root.querySelector<HTMLFormElement>("form")!;
            (
                updated.elements.namedItem("tool_read") as HTMLInputElement
            ).checked = true;
            updated.requestSubmit = () =>
                submitted.push(new FormData(updated).get("tool_read"));
            reconcile("conversation-settings-form");
            expect(submitted).toEqual(["read", null]);
            expect(value("tool_read")).toBeNull();
        });

        test("Git destination changes save and survive unrelated patches", () => {
            const original = root.innerHTML;
            const form = root.querySelector<HTMLFormElement>("form")!;
            const submitted: Array<FormDataEntryValue | null> = [];
            form.requestSubmit = () =>
                submitted.push(new FormData(form).get("git_destination"));
            const destination = form.elements.namedItem(
                "git_destination",
            ) as HTMLSelectElement;
            destination.value = "repository";
            destination.dispatchEvent(new Event("change", { bubbles: true }));
            expect(submitted).toEqual(["repository"]);
            root.innerHTML = original;
            reconcile("conversation-rename");
            expect(value("git_destination")).toBe("repository");
        });

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
