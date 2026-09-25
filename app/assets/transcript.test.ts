// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import {
    PIN_THRESHOLD_PX,
    initTranscript,
    isPinned,
    leftoverPx,
    revealBatchSize,
} from "./transcript";

let animationFrames: Map<number, FrameRequestCallback>;
let nextFrameId: number;
let reducedMotion: boolean;
let motionListeners: Set<() => void>;

function transcript(text: string): HTMLElement {
    document.body.innerHTML = `
        <section id="transcript">
            <article id="turn-1" class="chat-turn">
                <div class="chat-turn-body" data-streaming="true">
                    <section data-streaming-content>
                        <div data-streaming-text>${text}</div>
                    </section>
                </div>
            </article>
        </section>`;
    return document.querySelector<HTMLElement>("#transcript")!;
}

function streamedText(root: HTMLElement): string {
    return root.querySelector<HTMLElement>("[data-streaming-text]")!
        .textContent!;
}

function runAnimationFrame(): void {
    const callbacks = Array.from(animationFrames.values());
    animationFrames.clear();
    for (const callback of callbacks) {
        callback(performance.now());
    }
}

async function mutationsSettled(): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
    animationFrames = new Map();
    nextFrameId = 1;
    reducedMotion = false;
    motionListeners = new Set();
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
        const id = nextFrameId++;
        animationFrames.set(id, callback);
        return id;
    });
    vi.stubGlobal("cancelAnimationFrame", (id: number) => {
        animationFrames.delete(id);
    });
    vi.stubGlobal("matchMedia", () => ({
        get matches() {
            return reducedMotion;
        },
        media: "(prefers-reduced-motion: reduce)",
        onchange: null,
        addEventListener: (_type: string, listener: () => void) => {
            motionListeners.add(listener);
        },
        removeEventListener: (_type: string, listener: () => void) => {
            motionListeners.delete(listener);
        },
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
    }));
    HTMLElement.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
});

test("leftover is the distance from the viewport bottom to the document end", () => {
    expect(
        leftoverPx({
            scrollHeight: 1000,
            scrollY: 200,
            viewportHeight: 700,
        }),
    ).toBe(100);
});

test("a leftover at the threshold stays pinned", () => {
    expect(isPinned(0)).toBe(true);
    expect(isPinned(PIN_THRESHOLD_PX)).toBe(true);
});

test("a leftover above the threshold is unpinned", () => {
    expect(isPinned(PIN_THRESHOLD_PX + 1)).toBe(false);
});

test("automatic scroll follows only a pinned leftover", () => {
    const beforeAppend = leftoverPx({
        scrollHeight: 800,
        scrollY: 680,
        viewportHeight: 100,
    });
    expect(isPinned(beforeAppend)).toBe(true);

    const afterLargeAppend = leftoverPx({
        scrollHeight: 2000,
        scrollY: 680,
        viewportHeight: 100,
    });
    expect(isPinned(afterLargeAppend)).toBe(false);
    expect(isPinned(beforeAppend)).toBe(true);
});

test("a user who scrolls upward is unpinned", () => {
    const afterScrollUp = leftoverPx({
        scrollHeight: 2000,
        scrollY: 200,
        viewportHeight: 100,
    });
    expect(isPinned(afterScrollUp)).toBe(false);
});

test("a short buffer reveals one character in each frame", () => {
    const root = transcript("Fresh");
    const destroy = initTranscript(root);

    expect(streamedText(root)).toBe("");
    runAnimationFrame();
    expect(streamedText(root)).toBe("F");
    runAnimationFrame();
    expect(streamedText(root)).toBe("Fr");

    destroy();
});

test("a large backlog uses a larger catch-up batch", () => {
    expect(revealBatchSize(24)).toBe(1);
    expect(revealBatchSize(120)).toBeGreaterThan(1);
    expect(revealBatchSize(10_000)).toBe(32);
});

test("replacement markup keeps the revealed prefix and adopts new text", async () => {
    const root = transcript("Fresh");
    const destroy = initTranscript(root);
    runAnimationFrame();
    expect(streamedText(root)).toBe("F");

    root.querySelector(".chat-turn-body")!.outerHTML = `
        <div class="chat-turn-body" data-streaming="true">
            <section data-streaming-content>
                <div data-streaming-text><strong>Frink</strong>works</div>
            </section>
        </div>`;
    await mutationsSettled();

    expect(streamedText(root)).toBe("F");
    runAnimationFrame();
    expect(streamedText(root)).toBe("Fr");
    destroy();
    expect(streamedText(root)).toBe("Frinkworks");
});

test("retained markup adopts authoritative text updates", async () => {
    const root = transcript(
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
    );
    const destroy = initTranscript(root);
    runAnimationFrame();
    const revealed = streamedText(root);
    expect(revealed.length).toBeGreaterThan(0);
    expect(revealed.length).toBeLessThan(52);

    const node = root.querySelector("[data-streaming-text]")!.firstChild!;
    node.nodeValue =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz plus the final response";
    await mutationsSettled();

    expect(streamedText(root)).toBe(revealed);
    root.querySelector("[data-streaming]")!.removeAttribute("data-streaming");
    await mutationsSettled();
    expect(streamedText(root)).toBe(
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz plus the final response",
    );
    destroy();
});

test("stream completion flushes a buffer without a final replacement", async () => {
    const root = transcript("Frinkworks");
    const destroy = initTranscript(root);
    runAnimationFrame();
    expect(streamedText(root)).toBe("F");

    root.querySelector("[data-streaming]")!.removeAttribute("data-streaming");
    await mutationsSettled();

    expect(streamedText(root)).toBe("Frinkworks");
    destroy();
});

test("reduced motion leaves streamed text complete", () => {
    reducedMotion = true;
    const root = transcript("Frinkworks");
    const destroy = initTranscript(root);

    expect(streamedText(root)).toBe("Frinkworks");
    expect(animationFrames.size).toBe(1);

    destroy();
});

test("a motion preference change flushes the buffer", () => {
    const root = transcript("Frinkworks");
    const destroy = initTranscript(root);
    runAnimationFrame();
    expect(streamedText(root)).toBe("F");

    reducedMotion = true;
    for (const listener of motionListeners) {
        listener();
    }

    expect(streamedText(root)).toBe("Frinkworks");
    destroy();
});

test("cleanup cancels frames, restores text, and removes listeners", () => {
    const root = transcript("Frinkworks");
    const destroy = initTranscript(root);
    runAnimationFrame();
    expect(streamedText(root)).toBe("F");

    destroy();

    expect(streamedText(root)).toBe("Frinkworks");
    expect(animationFrames.size).toBe(0);
    expect(motionListeners.size).toBe(0);
});
