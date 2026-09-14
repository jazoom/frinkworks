// @vitest-environment happy-dom
import { beforeEach, expect, test, vi } from "vitest";
import { DEFAULT_THEME, initThemeSelector } from "./theme";

beforeEach(() => {
    delete document.documentElement.dataset.theme;
    document.body.replaceChildren();
});

function themeForm(theme: string): HTMLFormElement {
    const form = document.createElement("form");
    form.innerHTML = `
        <div data-active-theme="${theme}"></div>
        <select data-theme-select name="theme">
            <option value="system">System preference</option>
            <option value="springfield">Springfield</option>
            <option value="evergreen-terrace">Evergreen Terrace</option>
            <option value="leftorium">Leftorium</option>
            <option value="stonecutters">Stonecutters</option>
            <option value="sector-7-g">Sector 7-G</option>
        </select>
    `;
    document.body.append(form);
    return form;
}

test("the selector uses the server-rendered theme", () => {
    const form = themeForm("evergreen-terrace");

    initThemeSelector(form);

    expect(document.documentElement.dataset.theme).toBe("evergreen-terrace");
    expect(form.querySelector<HTMLSelectElement>("select")!.value).toBe(
        "evergreen-terrace",
    );
});

test("an invalid server-rendered theme defaults to the system preference", () => {
    const form = themeForm("unknown");

    initThemeSelector(form);

    expect(document.documentElement.dataset.theme).toBeUndefined();
    expect(form.querySelector<HTMLSelectElement>("select")!.value).toBe(
        DEFAULT_THEME,
    );
});

test("a change applies immediately and submits the preference", () => {
    const form = themeForm("springfield");
    const requestSubmit = vi
        .spyOn(form, "requestSubmit")
        .mockImplementation(() => {});
    initThemeSelector(form);
    const select = form.querySelector<HTMLSelectElement>("select")!;

    select.value = "sector-7-g";
    select.dispatchEvent(new Event("change", { bubbles: true }));

    expect(document.documentElement.dataset.theme).toBe("sector-7-g");
    expect(requestSubmit).toHaveBeenCalledOnce();
});

test("reconciliation restores the server-rendered selection", () => {
    const form = themeForm("springfield");
    const instance = initThemeSelector(form);
    document.documentElement.dataset.theme = "evergreen-terrace";
    form.innerHTML = themeForm("springfield").innerHTML;

    instance.reconcile?.({} as never);

    expect(document.documentElement.dataset.theme).toBe("springfield");
    expect(form.querySelector<HTMLSelectElement>("select")!.value).toBe(
        "springfield",
    );
});
