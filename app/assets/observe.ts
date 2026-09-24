import type { IslandInstance } from "hypergraft/browser";

const RETRY_MS = 1000;

export function initObserve(root: HTMLElement): IslandInstance {
    let timer: ReturnType<typeof setTimeout> | undefined;

    const form = () =>
        root.querySelector<HTMLFormElement>("form[method='get']");

    const submit = () => {
        if (root.dataset.observeActive !== "true") {
            return;
        }
        form()?.requestSubmit();
    };

    const clearTimer = () => {
        if (timer === undefined) {
            return;
        }
        clearTimeout(timer);
        timer = undefined;
    };

    const schedule = (delay: number) => {
        clearTimer();
        timer = setTimeout(() => {
            timer = undefined;
            submit();
        }, delay);
    };

    // The initiating command is still pending when this root is first inserted.
    // A synchronous submit is dropped by the document unsafe guard.
    schedule(0);

    return {
        reconcile(context) {
            // Navigation cancels safe requests without a settlement event.
            // A retained observer must restart, but not for its own GET URL.
            if (context.cause === "location") {
                if (context.detail.cause !== "get-form-replacement")
                    schedule(0);
                return;
            }
            if (context.cause !== "patch") {
                return;
            }
            if (root.dataset.observeActive !== "true") {
                return;
            }
            // Any unsafe command can cancel observation, even outside this root.
            if (context.detail.form.method.toLowerCase() !== "get") {
                schedule(0);
                return;
            }
            // A later segment must start after this one settles. Morph can
            // keep this root, so mount will not run again.
            if (context.detail.outcome === "applied-patch") {
                const target = root.dataset.observeTarget ?? "";
                if (
                    !context.detail.targetIds.some(
                        (id) =>
                            id === target ||
                            document.getElementById(id)?.contains(root),
                    )
                ) {
                    return;
                }
                schedule(0);
                return;
            }
            if (context.detail.form !== form()) {
                return;
            }
            if (context.detail.outcome !== "safe-failure") {
                return;
            }
            schedule(RETRY_MS);
        },
        destroy() {
            clearTimer();
        },
    };
}
