import { commandBlockReason, type IslandInstance } from "hypergraft/browser";

export const DEFAULT_THEME = "system";

export const THEMES = [
    "system",
    "springfield",
    "evergreen-terrace",
    "leftorium",
    "stonecutters",
    "sector-7-g",
] as const;

export type Theme = (typeof THEMES)[number];

function isTheme(value: string | null | undefined): value is Theme {
    return THEMES.some((theme) => theme === value);
}

function activeTheme(root: HTMLElement): Theme {
    const marker = root.querySelector<HTMLElement>("[data-active-theme]");
    return isTheme(marker?.dataset.activeTheme)
        ? marker.dataset.activeTheme
        : DEFAULT_THEME;
}

function applyTheme(page: HTMLElement, theme: Theme): void {
    if ((page.dataset.theme ?? DEFAULT_THEME) === theme) {
        return;
    }
    page.classList.add("theme-switching");
    if (theme === "system") {
        delete page.dataset.theme;
    } else {
        page.dataset.theme = theme;
    }
    void page.offsetWidth;
    page.classList.remove("theme-switching");
}

export function initThemeSelector(root: HTMLElement): IslandInstance {
    if (!(root instanceof HTMLFormElement)) {
        return { destroy() {} };
    }

    const page = document.documentElement;
    const applyAuthoritativeTheme = () => {
        const theme = activeTheme(root);
        applyTheme(page, theme);
        root.querySelectorAll<HTMLInputElement>("[data-theme-choice]").forEach(
            (choice) => {
                choice.checked = choice.value === theme;
            },
        );
    };
    applyAuthoritativeTheme();

    const onChange = (event: Event) => {
        const choice = event.target;
        if (
            !(choice instanceof HTMLInputElement) ||
            !choice.matches("[data-theme-choice]") ||
            !choice.checked
        ) {
            return;
        }
        if (!isTheme(choice.value) || commandBlockReason()) {
            applyAuthoritativeTheme();
            return;
        }
        applyTheme(page, choice.value);
        root.requestSubmit();
    };

    root.addEventListener("change", onChange);

    return {
        reconcile() {
            applyAuthoritativeTheme();
        },
        destroy() {
            root.removeEventListener("change", onChange);
        },
    };
}
