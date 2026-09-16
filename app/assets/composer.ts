import type { IslandInstance, IslandMountContext } from "hypergraft/browser";

function usesCommandKey(): boolean {
    const nav = navigator as Navigator & {
        userAgentData?: { platform?: string };
    };
    const platform = nav.userAgentData?.platform ?? nav.platform;
    return /Mac|iPhone|iPad|iPod/.test(platform);
}

function sendShortcutHint(): string {
    const key = usesCommandKey() ? "⌘" : "Ctrl";
    return `${key} + Enter to send.`;
}

export function initShortcutHint(root: HTMLElement): IslandInstance {
    const sync = () => {
        root.textContent = sendShortcutHint();
    };
    sync();
    return { reconcile: sync, destroy() {} };
}

function messageField(root: HTMLElement): HTMLTextAreaElement | null {
    return root.querySelector("#composer-message");
}

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

    root.addEventListener("input", captureDraft, { signal });
    root.addEventListener("focusin", captureDraft, { signal });
    root.addEventListener("keyup", captureDraft, { signal });
    root.addEventListener("mouseup", captureDraft, { signal });
    root.addEventListener("keydown", onKeyDown, { signal });

    return {
        reconcile(context) {
            if (context.cause === "location") {
                const nextOwner = conversationIdentity();
                if (owner && nextOwner === owner) restoreDraft();
                else captureDraft();
                owner = nextOwner;
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
            message.value = "";
            message.focus();
        },
        destroy() {},
    };
}
