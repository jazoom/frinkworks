export const PIN_THRESHOLD_PX = 96;
const CATCH_UP_THRESHOLD = 24;
const CATCH_UP_FRAMES = 12;
const MAX_CHARACTERS_PER_FRAME = 32;

interface TextSegment {
    node: Text;
    characters: string[];
    start: number;
}

interface StreamState {
    element: HTMLElement;
    revealed: number;
    total: number;
    segments: TextSegment[];
}

export function leftoverPx(input: {
    scrollHeight: number;
    scrollY: number;
    viewportHeight: number;
}): number {
    return input.scrollHeight - input.scrollY - input.viewportHeight;
}

export function isPinned(
    leftover: number,
    threshold = PIN_THRESHOLD_PX,
): boolean {
    return leftover <= threshold;
}

export function revealBatchSize(backlog: number): number {
    if (backlog <= 0) {
        return 0;
    }
    if (backlog <= CATCH_UP_THRESHOLD) {
        return 1;
    }
    return Math.min(
        MAX_CHARACTERS_PER_FRAME,
        Math.ceil((backlog - CATCH_UP_THRESHOLD) / CATCH_UP_FRAMES) + 1,
    );
}

function streamId(element: HTMLElement): string | null {
    return element.closest<HTMLElement>(".chat-turn")?.id ?? null;
}

function textSegments(element: HTMLElement): {
    segments: TextSegment[];
    total: number;
} {
    const segments: TextSegment[] = [];
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    let total = 0;
    let node = walker.nextNode();
    while (node) {
        const text = node as Text;
        const characters = Array.from(text.data);
        segments.push({
            node: text,
            characters,
            start: total,
        });
        total += characters.length;
        node = walker.nextNode();
    }
    return { segments, total };
}

function renderCount(state: StreamState, count: number): void {
    state.revealed = Math.min(count, state.total);
    for (const segment of state.segments) {
        const visible = Math.max(
            0,
            Math.min(segment.characters.length, state.revealed - segment.start),
        );
        const value = segment.characters.slice(0, visible).join("");
        if (segment.node.data !== value) {
            segment.node.data = value;
        }
    }
}

export function initTranscript(root: HTMLElement): () => void {
    let pinned = true;
    let frame: number | null = null;
    let destroyed = false;
    const streams = new Map<string, StreamState>();
    const motionSkipped = new Set<string>();
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

    const jump = root
        .closest(".chat-sheet")
        ?.querySelector<HTMLButtonElement>("[data-jump-latest]");
    const measure = () =>
        leftoverPx({
            scrollHeight: root.scrollHeight,
            scrollY: root.scrollTop,
            viewportHeight: root.clientHeight,
        });
    const updateJump = () => {
        if (jump) {
            jump.hidden =
                !root.querySelector(".chat-turn") || isPinned(measure());
        }
    };

    const onScroll = () => {
        pinned = isPinned(measure());
        updateJump();
    };

    const stick = () => {
        if (pinned && root.querySelector(".chat-turn")) {
            // Scroll the transcript, not its ancestors or the composer.
            root.scrollTop = root.scrollHeight;
        }
        updateJump();
    };

    const jumpToLatest = () => {
        pinned = true;
        stick();
        root.focus({ preventScroll: true });
    };

    const observeOptions: MutationObserverInit = {
        attributeFilter: ["data-streaming"],
        attributes: true,
        childList: true,
        characterData: true,
        subtree: true,
    };

    let observer: MutationObserver;

    const clearObservedMutations = () => {
        observer.takeRecords();
    };

    const animate = () => {
        frame = null;
        if (destroyed) {
            return;
        }

        let pending = false;
        for (const state of streams.values()) {
            const batch = revealBatchSize(state.total - state.revealed);
            if (batch > 0) {
                renderCount(state, state.revealed + batch);
                pending ||= state.revealed < state.total;
            }
        }
        clearObservedMutations();
        stick();
        if (pending) {
            frame = requestAnimationFrame(animate);
        }
    };

    const schedule = () => {
        if (frame === null && streams.size > 0 && !reducedMotion.matches) {
            frame = requestAnimationFrame(animate);
        }
    };

    const reconcile = (mutations: MutationRecord[] = []) => {
        const changedElements = new Set<HTMLElement>();
        for (const mutation of mutations) {
            const target =
                mutation.target instanceof HTMLElement
                    ? mutation.target
                    : mutation.target.parentElement;
            const element = target?.closest<HTMLElement>(
                "[data-streaming-text]",
            );
            if (element) {
                changedElements.add(element);
            }
        }

        const present = new Set<string>();
        const elements = root.querySelectorAll<HTMLElement>(
            "[data-streaming-text]",
        );

        for (const element of elements) {
            const id = streamId(element);
            if (!id) {
                continue;
            }
            present.add(id);
            let previous = streams.get(id);
            const retainedAndChanged =
                previous?.element === element && changedElements.has(element);
            if (previous && retainedAndChanged) {
                const source = textSegments(element);
                previous = {
                    element,
                    revealed: Math.min(previous.revealed, source.total),
                    total: source.total,
                    segments: source.segments,
                };
                streams.set(id, previous);
            }

            const active =
                element.closest<HTMLElement>("[data-streaming='true']") !==
                null;
            if (!active) {
                if (previous?.element === element) {
                    renderCount(previous, previous.total);
                }
                streams.delete(id);
                motionSkipped.delete(id);
                continue;
            }
            if (reducedMotion.matches || motionSkipped.has(id)) {
                if (reducedMotion.matches) {
                    motionSkipped.add(id);
                }
                streams.delete(id);
                continue;
            }

            if (previous?.element === element) {
                if (retainedAndChanged) {
                    renderCount(previous, previous.revealed);
                }
                continue;
            }
            const source = textSegments(element);
            const state: StreamState = {
                element,
                revealed: Math.min(previous?.revealed ?? 0, source.total),
                total: source.total,
                segments: source.segments,
            };
            streams.set(id, state);
            renderCount(state, state.revealed);
        }

        for (const [id] of streams) {
            if (!present.has(id)) {
                streams.delete(id);
            }
        }
        clearObservedMutations();
        schedule();
    };

    const onMutations = (mutations: MutationRecord[]) => {
        stick();
        reconcile(mutations);
    };

    const onMotionChange = () => {
        if (!reducedMotion.matches) {
            return;
        }
        if (frame !== null) {
            cancelAnimationFrame(frame);
            frame = null;
        }
        for (const [id, state] of streams) {
            renderCount(state, state.total);
            motionSkipped.add(id);
        }
        streams.clear();
        clearObservedMutations();
        stick();
    };

    observer = new MutationObserver(onMutations);
    observer.observe(root, observeOptions);
    root.addEventListener("scroll", onScroll, { passive: true });
    reducedMotion.addEventListener("change", onMotionChange);
    jump?.addEventListener("click", jumpToLatest);
    const resize = new ResizeObserver(stick);
    resize.observe(root);
    root.addEventListener("toggle", stick, true);
    reconcile();
    const initialScroll = requestAnimationFrame(stick);

    return () => {
        destroyed = true;
        cancelAnimationFrame(initialScroll);
        if (frame !== null) {
            cancelAnimationFrame(frame);
        }
        for (const state of streams.values()) {
            renderCount(state, state.total);
        }
        streams.clear();
        observer.disconnect();
        resize.disconnect();
        jump?.removeEventListener("click", jumpToLatest);
        root.removeEventListener("toggle", stick, true);
        root.removeEventListener("scroll", onScroll);
        reducedMotion.removeEventListener("change", onMotionChange);
    };
}
