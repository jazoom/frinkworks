import type { IslandInstance, IslandMountContext } from "hypergraft/browser";

function usesCommandKey(): boolean {
    const nav = navigator as Navigator & {
        userAgentData?: { platform?: string };
    };
    const platform = nav.userAgentData?.platform ?? nav.platform;
    return /Mac|iPhone|iPad|iPod/.test(platform);
}

function sendShortcutHint(action: string): string {
    const key = usesCommandKey() ? "⌘" : "Ctrl";
    return `${key} + Enter to ${action}.`;
}

export function initShortcutHint(root: HTMLElement): IslandInstance {
    const sync = () => {
        root.textContent = sendShortcutHint(
            root.dataset.shortcutAction === "queue" ? "queue" : "send",
        );
    };
    sync();
    return { reconcile: sync, destroy() {} };
}

function messageField(root: HTMLElement): HTMLTextAreaElement | null {
    return root.querySelector("#composer-message");
}

export type ComposerMode =
    | { kind: "literal" }
    | { kind: "skill" }
    | { kind: "prompt" }
    | { kind: "command"; excluded: boolean };

/** Classify the leading prefix exactly as the server does. A backslash before
 * `/` or `!` keeps the text literal. */
export function classifyComposerInput(value: string): ComposerMode {
    const start = value.search(/\S/);
    if (start < 0) return { kind: "literal" };
    const after = value.slice(start);
    if (after.startsWith("\\") && (after[1] === "/" || after[1] === "!")) {
        return { kind: "literal" };
    }
    if (after.startsWith("!!")) return { kind: "command", excluded: true };
    if (after.startsWith("!")) return { kind: "command", excluded: false };
    if (after.startsWith("/skill:") || after === "/skill") {
        return { kind: "skill" };
    }
    if (after.startsWith("/")) return { kind: "prompt" };
    return { kind: "literal" };
}

function commandLocationLabel(root: HTMLElement): string {
    // Setup can contain an unapproved execution switch. Only the server's
    // applied location describes where a saved conversation runs a command.
    return root.dataset.commandLocation === "host"
        ? "This computer"
        : "Sandbox";
}

// Catalogue navigation removes the composer. Keep one ordinary draft in this
// document only, and never restore it into another conversation or revision.
let rememberedDraft: { owner: string; text: string } | undefined;

export function initComposer(
    root: HTMLElement,
    { signal }: IslandMountContext,
): IslandInstance | void {
    if (!(root instanceof HTMLFormElement)) {
        return;
    }

    const conversationIdentity = () =>
        root
            .closest("#conversation-detail")
            ?.querySelector<HTMLElement>("[data-conversation-url]")?.dataset
            .conversationUrl;
    let owner = conversationIdentity();
    let draft = messageField(root)?.value ?? "";
    if (
        owner &&
        rememberedDraft?.owner === owner &&
        draft === "" &&
        !root.querySelector("[data-revision-state]")
    ) {
        draft = rememberedDraft.text;
        const message = messageField(root);
        if (message) message.value = draft;
    }
    rememberedDraft = undefined;
    let selectionStart = 0;
    let selectionEnd = 0;

    const captureDraft = () => {
        const message = messageField(root);
        if (!message) {
            return;
        }
        draft = message.value;
        selectionStart = message.selectionStart;
        selectionEnd = message.selectionEnd;
        rememberedDraft =
            owner && !root.querySelector("[data-revision-state]")
                ? { owner, text: draft }
                : undefined;
    };

    const restoreDraft = () => {
        const message = messageField(root);
        if (!message) {
            return;
        }
        const focused = document.activeElement === message;
        message.value = draft;
        if (focused) {
            message.setSelectionRange(
                Math.min(selectionStart, draft.length),
                Math.min(selectionEnd, draft.length),
            );
        }
    };

    const onKeyDown = (event: KeyboardEvent) => {
        if (event.isComposing) {
            return;
        }
        if (event.key !== "Enter" || !(event.metaKey || event.ctrlKey)) {
            return;
        }
        if (!(event.target instanceof HTMLTextAreaElement)) {
            return;
        }
        event.preventDefault();
        const submitter = root.querySelector<HTMLButtonElement>(
            'button[type="submit"][name="mode"][value="quick"], button[type="submit"]',
        );
        if (!submitter || submitter.disabled || submitter.form !== root) {
            return;
        }
        root.requestSubmit(submitter);
    };

    const message = messageField(root);
    const submitLabel = root.querySelector<HTMLElement>(
        "[data-composer-submit-label]",
    );
    const submitButton = root.querySelector<HTMLButtonElement>(
        "[data-composer-submit]",
    );
    const commandStatus = root.querySelector<HTMLElement>(
        "[data-composer-command-status]",
    );
    const initialLabel = submitLabel?.textContent?.trim() ?? "Send";

    const syncCommandMode = () => {
        // A retained composer can switch between Send and Queue after a patch.
        // Its server-authored defaults, not the previous label, own that state.
        const defaultLabel =
            submitButton?.dataset.defaultLabel?.trim() ?? initialLabel;
        const defaultAriaLabel =
            submitButton?.dataset.defaultAriaLabel?.trim() ?? defaultLabel;
        const mode = classifyComposerInput(messageField(root)?.value ?? "");
        if (mode.kind !== "command" || !message || message.disabled) {
            if (submitLabel) submitLabel.textContent = defaultLabel;
            submitButton?.setAttribute("aria-label", defaultAriaLabel);
            if (commandStatus) {
                commandStatus.hidden = true;
                commandStatus.textContent = "";
            }
            return;
        }
        const label = mode.excluded ? "Run without context" : "Run command";
        if (submitLabel) submitLabel.textContent = label;
        submitButton?.setAttribute("aria-label", label);
        if (commandStatus) {
            const context = mode.excluded
                ? "Not added to automatic model context"
                : "Added to model context";
            commandStatus.textContent =
                `Direct command \u00b7 ${commandLocationLabel(root)} \u00b7 ` +
                `${context} \u00b7 No provider connection is needed.`;
            commandStatus.hidden = false;
        }
    };

    const insertPrefix = (button: HTMLElement) => {
        const insert = button.dataset.composerInsert ?? "";
        if (insert === "") return;
        const field = messageField(root);
        if (!field || field.disabled) return;
        let caret: number;
        if (insert === "/") {
            const command = /^\s*\/\S*/.exec(field.value);
            if (command) {
                caret = command[0].length;
            } else {
                // Commands occupy the leading token. Keep the draft as arguments,
                // even when the user selects text before this action.
                field.value = field.value ? `/ ${field.value}` : "/";
                caret = 1;
            }
        } else {
            const start = field.selectionStart ?? field.value.length;
            const end = field.selectionEnd ?? start;
            const before = field.value.slice(0, start);
            const space = before.length > 0 && !/\s$/.test(before) ? " " : "";
            const text = `${space}${insert}`;
            field.value = before + text + field.value.slice(end);
            caret = start + text.length;
        }
        field.focus();
        field.setSelectionRange(caret, caret);
        field.dispatchEvent(new Event("input", { bubbles: true }));
        syncCommandMode();
    };

    const onInsertClick = (event: MouseEvent) => {
        if (!(event.target instanceof Element)) return;
        const button = event.target.closest<HTMLElement>(
            "[data-composer-insert]",
        );
        if (!button) return;
        event.preventDefault();
        insertPrefix(button);
    };

    root.addEventListener("input", syncCommandMode, { signal });
    root.addEventListener("click", onInsertClick, { signal });
    syncCommandMode();
    captureDraft();

    root.addEventListener("input", captureDraft, { signal });
    root.addEventListener("focusin", captureDraft, { signal });
    root.addEventListener("keyup", captureDraft, { signal });
    root.addEventListener("mouseup", captureDraft, { signal });
    root.addEventListener("keydown", onKeyDown, { signal });

    return {
        reconcile(context) {
            syncCommandMode();
            if (context.cause === "location") {
                const nextOwner = conversationIdentity();
                if (owner && nextOwner === owner) restoreDraft();
                owner = nextOwner;
                captureDraft();
                return;
            }
            if (context.cause === "live-patch") {
                restoreDraft();
                return;
            }
            if (
                context.cause !== "patch" ||
                context.detail.outcome !== "applied-patch"
            ) {
                return;
            }
            if (
                context.detail.form === root &&
                (new URL(
                    context.detail.url,
                    window.location.href,
                ).pathname.match(/\/(directories|settings)\//) ||
                    root
                        .closest("#conversation-detail")
                        ?.querySelector('[data-conversation-state="new"]'))
            ) {
                restoreDraft();
                return;
            }
            if (context.detail.form !== root) {
                if (
                    context.detail.targetIds.includes("composer") ||
                    context.detail.targetIds.includes("conversation-detail")
                ) {
                    // Projections must not erase an unsent message.
                    restoreDraft();
                }
                return;
            }
            if (context.detail.targetIds.includes("composer")) {
                captureDraft();
            }
            if (context.detail.status !== 200) {
                restoreDraft();
                return;
            }
            if (
                !context.detail.targetIds.includes("transcript") &&
                !context.detail.targetIds.includes("conversation-detail")
            ) {
                return;
            }
            const message = messageField(root);
            if (!message) {
                return;
            }
            draft = "";
            rememberedDraft = undefined;
            message.value = "";
            message.focus();
        },
        destroy() {},
    };
}
