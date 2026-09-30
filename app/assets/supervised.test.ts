// @vitest-environment happy-dom
import { afterEach, expect, test, vi } from "vitest";
import controls from "../src/shared_templates/layout/development.html?raw";

vi.mock("./hypergraft-bootstrap", () => ({ startApp: vi.fn() }));
vi.mock("hypergraft/browser", () => ({
    commandBlockReason: vi.fn(),
    listenForLocationChanges: vi.fn(),
    listenForLivePatches: vi.fn(),
    listenForRequestSettled: vi.fn(),
}));

afterEach(() => {
    window.dispatchEvent(new Event("pagehide"));
    document.body.replaceChildren();
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
});

function response(phase: string, status: number, canInterrupt = false) {
    return new Response(
        JSON.stringify({
            phase,
            revision: "current",
            message: phase,
            canInterrupt,
        }),
        { status },
    );
}

test("only an interruptible restart refusal exposes confirmation, without an automatic retry", async () => {
    vi.stubEnv("MODE", "supervised");
    vi.stubEnv("VITE_SUPERVISED_REVISION", "current");
    document.body.innerHTML = controls;
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    await import("./main");
    const confirmation = document.querySelector<HTMLElement>(
        "[data-supervised-interrupt]",
    )!;
    const normal = document.querySelector<HTMLButtonElement>(
        '[data-supervised-rebuild=""]',
    )!;
    const interrupt = document.querySelector<HTMLButtonElement>(
        '[data-supervised-rebuild="interrupt"]',
    )!;
    const cancel = document.querySelector<HTMLButtonElement>(
        "[data-supervised-cancel]",
    )!;

    expect(confirmation.hidden).toBe(true);
    expect(normal.hidden).toBe(false);
    interrupt.click();
    expect(fetch).not.toHaveBeenCalled();

    // An error without the server's explicit interruption offer grants no override.
    fetch.mockResolvedValueOnce(response("error", 409));
    normal.click();
    await vi.waitFor(() => expect(normal.disabled).toBe(false));
    expect(confirmation.hidden).toBe(true);
    expect(normal.hidden).toBe(false);
    expect(fetch).toHaveBeenCalledTimes(1);

    fetch.mockResolvedValueOnce(response("error", 409, true));
    normal.click();
    await vi.waitFor(() => expect(confirmation.hidden).toBe(false));
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(fetch.mock.calls[1][1].headers).toEqual({
        "X-Frinkworks-Dev": "restart",
    });
    expect(normal.hidden).toBe(true);
    expect(interrupt.disabled).toBe(false);

    cancel.click();
    expect(confirmation.hidden).toBe(true);
    expect(normal.hidden).toBe(false);
    expect(document.activeElement).toBe(normal);
    interrupt.click();
    expect(fetch).toHaveBeenCalledTimes(2);

    fetch.mockResolvedValueOnce(response("error", 503, true));
    normal.click();
    await vi.waitFor(() => expect(confirmation.hidden).toBe(false));
    let resolve!: (response: Response) => void;
    fetch.mockImplementationOnce(
        () =>
            new Promise<Response>((done) => {
                resolve = done;
            }),
    );
    interrupt.click();
    expect(fetch).toHaveBeenCalledTimes(4);
    expect(fetch.mock.calls[3][1].headers).toEqual({
        "X-Frinkworks-Dev": "interrupt-restart",
    });
    expect(confirmation.hidden).toBe(true);
    expect(normal.hidden).toBe(false);
    expect(normal.disabled).toBe(true);
    expect(interrupt.disabled).toBe(true);
    normal.click();
    interrupt.click();
    expect(fetch).toHaveBeenCalledTimes(4);
    resolve(response("building", 202));
    await vi.waitFor(() =>
        expect(
            document.querySelector("[data-supervised-status]")!.textContent,
        ).toBe("building"),
    );

    fetch.mockResolvedValue(response("error", 200));
    window.dispatchEvent(new Event("pageshow"));
    await vi.waitFor(() => expect(normal.disabled).toBe(false));
    expect(confirmation.hidden).toBe(true);
    expect(normal.hidden).toBe(false);
    expect(interrupt.disabled).toBe(true);
});
