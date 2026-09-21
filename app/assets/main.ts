import "@fontsource/ibm-plex-sans/latin-400.css";
import "@fontsource/ibm-plex-sans/latin-600.css";
import "@fontsource/ibm-plex-sans/latin-700.css";
import "@fontsource/ibm-plex-mono/latin-400.css";
import "@fontsource/ibm-plex-mono/latin-500.css";
import "./input.css";
import { startApp } from "./hypergraft-bootstrap";
import {
    commandBlockReason,
    listenForRequestSettled,
} from "hypergraft/browser";

type ComposerModel = {
    id: string;
    favourite: boolean;
    default_effort: string;
    efforts: { value: string; label: string }[];
};

function composerCatalogue(): Record<string, ComposerModel[]> {
    const source = document.querySelector<HTMLElement>(
        "[data-conversation-model-catalogue]",
    );
    try {
        return JSON.parse(source?.dataset.conversationModelCatalogue ?? "{}");
    } catch {
        return {};
    }
}

function composerControls() {
    const model = document.querySelector<HTMLInputElement>(
        "#conversation-model",
    );
    const provider = document.querySelector<HTMLInputElement>(
        "#conversation-provider",
    );
    const thinking = document.querySelector<HTMLSelectElement>(
        "#conversation-thinking",
    );
    const toggle = document.querySelector<HTMLButtonElement>(
        "#conversation-model-toggle",
    );
    const panel = document.querySelector<HTMLElement>(
        "#conversation-model-options",
    );
    return model && provider && thinking && toggle && panel
        ? { model, provider, thinking, toggle, panel }
        : null;
}

function renderComposerModels() {
    const controls = composerControls();
    const results = document.querySelector<HTMLElement>(
        "#conversation-model-results",
    );
    const search = document.querySelector<HTMLInputElement>(
        "#conversation-model-search",
    );
    const filter = document.querySelector<HTMLSelectElement>(
        "#conversation-model-provider-filter",
    );
    if (!controls || !results || !search || !filter) return;
    const clear = document.getElementById("conversation-model-search-clear");
    if (clear) clear.hidden = search.value.length === 0;
    const query = search.value.trim().toLocaleLowerCase();
    const favouritesOnly =
        document.getElementById("conversation-model-favourites-filter")
            ?.ariaPressed === "true";
    const matches = Object.entries(composerCatalogue())
        .flatMap(([provider, models]) => {
            const label =
                Array.from(filter.options).find(
                    (option) => option.value === provider,
                )?.text ?? provider;
            return models
                .filter(
                    (model) =>
                        (!filter.value || filter.value === provider) &&
                        (!favouritesOnly || model.favourite) &&
                        `${model.id} ${label}`
                            .toLocaleLowerCase()
                            .includes(query),
                )
                .map((model) => ({ ...model, provider, label }));
        })
        .sort(
            (a, b) =>
                Number(b.favourite) - Number(a.favourite) ||
                a.id.localeCompare(b.id) ||
                a.provider.localeCompare(b.provider),
        );
    const nodes: HTMLElement[] = [];
    let group = "";
    for (const item of matches) {
        const heading = item.favourite ? "Favourites" : "All models";
        if (heading !== group) {
            const label = document.createElement("p");
            label.className = "px-2 pt-3 pb-1 text-xs font-semibold text-quiet";
            label.textContent = heading;
            nodes.push(label);
            group = heading;
        }
        const row = document.createElement("div");
        row.className = "flex min-w-0 items-center gap-1";
        const button = document.createElement("button");
        button.type = "button";
        button.dataset.composerModel = item.id;
        button.dataset.provider = item.provider;
        button.className =
            "btn btn-ghost h-auto min-h-11 min-w-0 flex-1 justify-start gap-1 px-2 py-2 text-left font-normal aria-pressed:bg-base-200";
        button.ariaPressed = String(
            controls.provider.value === item.provider &&
                controls.model.value === item.id,
        );
        const label = document.createElement("span");
        label.className = "flex min-w-0 flex-1 flex-col";
        const name = document.createElement("span");
        name.className = "break-all text-sm";
        name.textContent = item.id;
        const provider = document.createElement("span");
        provider.className = "text-xs text-quiet";
        provider.textContent = item.label;
        label.append(name, provider);
        button.append(label);
        if (button.ariaPressed === "true") button.append(modelIcon("check"));
        const favourite = document.createElement("button");
        favourite.type = "button";
        favourite.dataset.modelFavourite = item.id;
        favourite.dataset.provider = item.provider;
        favourite.className =
            "btn btn-ghost btn-square min-h-11 h-11 w-11 shrink-0";
        favourite.ariaPressed = String(item.favourite);
        favourite.ariaLabel = `${item.favourite ? "Remove" : "Add"} ${item.id} (${item.label}) ${item.favourite ? "from" : "to"} favourites`;
        favourite.title = item.favourite ? "Remove favourite" : "Add favourite";
        favourite.append(modelIcon("star", item.favourite));
        row.append(button, favourite);
        nodes.push(row);
    }
    results.replaceChildren(...nodes);
    const status = document.getElementById("conversation-model-search-status");
    if (status)
        status.textContent = matches.length
            ? `${matches.length} ${matches.length === 1 ? "model" : "models"}`
            : favouritesOnly
              ? "No favourites match. Turn off Favourites to see all models."
              : "No models match. Clear the search or change the provider filter.";
}

function modelIcon(name: string, filled = false): SVGSVGElement {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.classList.add("workspace-icon", "shrink-0");
    svg.setAttribute("aria-hidden", "true");
    if (filled) svg.classList.add("model-favourite-filled");
    const use = document.createElementNS(svg.namespaceURI, "use");
    use.setAttribute("href", `/static/images/workspace-icons.svg#${name}`);
    svg.append(use);
    return svg;
}

function composerEfforts(saved: string) {
    const controls = composerControls();
    if (!controls) return;
    const selected = composerCatalogue()[controls.provider.value]?.find(
        (item) => item.id === controls.model.value,
    );
    const efforts = selected?.efforts ?? [];
    controls.thinking.replaceChildren(
        ...(efforts.length
            ? efforts.map((effort) => new Option(effort.label, effort.value))
            : [new Option("Not available", "")]),
    );
    controls.thinking.value = efforts.some((effort) => effort.value === saved)
        ? saved
        : (selected?.default_effort ?? "");
    controls.thinking.disabled =
        controls.toggle.disabled || efforts.length === 0;
    const label = document.getElementById("conversation-model-value");
    if (label) label.textContent = controls.model.value || "Choose a model";
    syncThinkingChoice();
}

function syncThinkingChoice() {
    const select = document.querySelector<HTMLSelectElement>(
        "#conversation-thinking",
    );
    const toggle = document.querySelector<HTMLButtonElement>(
        "#conversation-thinking-toggle",
    );
    const label = document.getElementById("conversation-thinking-value");
    const results = document.getElementById("conversation-thinking-results");
    if (!select || !toggle || !label || !results) return;
    toggle.disabled = select.disabled;
    label.textContent =
        select.selectedOptions[0]?.text.trim() || "Not available";
    results.replaceChildren(
        ...Array.from(select.options, (option) => {
            const button = document.createElement("button");
            button.type = "button";
            button.dataset.thinkingValue = option.value;
            button.className =
                "btn btn-ghost h-auto min-h-11 w-full justify-between gap-6 px-3 py-3 text-left text-sm font-normal aria-pressed:bg-base-200";
            button.ariaPressed = String(option.selected);
            button.disabled = select.disabled || option.disabled;
            const text = document.createElement("span");
            text.textContent = option.text.trim();
            button.append(text);
            if (option.selected) button.append(modelIcon("check"));
            return button;
        }),
    );
}

let pendingModel:
    | {
          provider: string;
          model: string;
          thinking: string;
          focus: string;
          message: string;
      }
    | undefined;
let pendingFavourite:
    { provider: string; model: string; scroll: number } | undefined;
let modelLocks: {
    control: HTMLButtonElement | HTMLSelectElement;
    disabled: boolean;
}[] = [];

function saveComposerModel(focus: string) {
    const controls = composerControls();
    if (!controls) return;
    controls.model.dispatchEvent(new Event("input", { bubbles: true }));
    if (controls.model.form?.id !== "conversation-model-form") return;
    const settings = document.querySelector<HTMLFormElement>(
        "#conversation-settings-form",
    );
    const saved = (name: string) =>
        (settings?.elements.namedItem(name) as HTMLInputElement | null)
            ?.value ?? "";
    pendingModel = {
        provider: saved("provider"),
        model: saved("model"),
        thinking: saved("thinking"),
        focus,
        message:
            document.querySelector<HTMLTextAreaElement>("#composer-message")
                ?.value ?? "",
    };
    controls.model.form.requestSubmit();
}

document.addEventListener("submit", (event) => {
    if (
        event.target instanceof HTMLFormElement &&
        event.target.hasAttribute("data-queue-return")
    ) {
        const editor = event.target.querySelector<HTMLInputElement>(
            "[data-queue-editor]",
        );
        const message =
            document.querySelector<HTMLTextAreaElement>("#composer-message");
        const text = event.target.dataset.queueText;
        const replace =
            event.target.querySelector<HTMLInputElement>(
                'input[name="replace"]',
            )?.value === "1";
        if (!editor || !message || text === undefined) {
            event.preventDefault();
            return;
        }
        if (!message.value.trim() || replace) {
            message.value = text;
            message.dispatchEvent(new Event("input", { bubbles: true }));
            message.focus();
        }
        editor.value = message.value;
        return;
    }
    if (
        !(event.target instanceof HTMLFormElement) ||
        event.target.id !== "conversation-model-form"
    )
        return;
    const controls = composerControls();
    const send = document.querySelector<HTMLButtonElement>(
        '#conversation-composer button[name="action"]',
    );
    if (!controls) return;
    const thinkingToggle = document.querySelector<HTMLButtonElement>(
        "#conversation-thinking-toggle",
    );
    // Lock the message command until the server returns the model's new revision.
    modelLocks = [
        controls.toggle,
        controls.thinking,
        ...(thinkingToggle ? [thinkingToggle] : []),
        ...(send ? [send] : []),
    ].map((control) => ({ control, disabled: control.disabled }));
    // Hypergraft captures the form before these controls become disabled.
    queueMicrotask(() =>
        modelLocks.forEach(({ control }) => {
            control.disabled = true;
        }),
    );
});

document.addEventListener(
    "beforetoggle",
    (event) => {
        if (
            !(event instanceof ToggleEvent) ||
            !(event.target instanceof HTMLElement)
        )
            return;
        if (event.target.id === "conversation-thinking-options") {
            const toggle = document.querySelector<HTMLButtonElement>(
                "#conversation-thinking-toggle",
            );
            if (!toggle) return;
            toggle.ariaExpanded = String(event.newState === "open");
            if (event.newState === "open") {
                syncThinkingChoice();
                const panel = event.target;
                requestAnimationFrame(() => {
                    if (panel.matches(":popover-open"))
                        panel
                            .querySelector<HTMLElement>('[aria-pressed="true"]')
                            ?.focus();
                });
            }
            return;
        }
        if (event.target.id !== "conversation-model-options") return;
        const controls = composerControls();
        if (!controls) return;
        controls.toggle.ariaExpanded = String(event.newState === "open");
        if (event.newState === "open") {
            const search = document.querySelector<HTMLInputElement>(
                "#conversation-model-search",
            );
            if (search) search.value = "";
            renderComposerModels();
        }
    },
    true,
);

document.addEventListener("input", (event) => {
    if (
        event.target instanceof HTMLTextAreaElement &&
        event.target.id === "composer-message" &&
        pendingModel
    )
        pendingModel.message = event.target.value;
    if (
        event.target instanceof HTMLInputElement &&
        event.target.id === "conversation-model-search"
    )
        renderComposerModels();
});
document.addEventListener("change", (event) => {
    if (!(event.target instanceof HTMLSelectElement)) return;
    if (event.target.id === "conversation-model-provider-filter")
        renderComposerModels();
    if (event.target.id === "conversation-thinking") {
        syncThinkingChoice();
        saveComposerModel("conversation-thinking-toggle");
    }
});
document.addEventListener("click", (event) => {
    if (!(event.target instanceof Element)) return;
    const controls = composerControls();
    if (!controls || controls.toggle.disabled) return;
    const effort = event.target.closest<HTMLButtonElement>(
        "[data-thinking-value]",
    );
    if (effort) {
        const value = effort.dataset.thinkingValue;
        if (
            commandBlockReason() ||
            controls.thinking.disabled ||
            value === undefined ||
            !Array.from(controls.thinking.options).some(
                (option) => option.value === value && !option.disabled,
            )
        )
            return;
        document.getElementById("conversation-thinking-options")?.hidePopover();
        document.getElementById("conversation-thinking-toggle")?.focus();
        if (controls.thinking.value !== value) {
            controls.thinking.value = value;
            controls.thinking.dispatchEvent(
                new Event("change", { bubbles: true }),
            );
        }
        return;
    }
    const clear = event.target.closest<HTMLButtonElement>(
        "#conversation-model-search-clear",
    );
    if (clear) {
        const search = document.querySelector<HTMLInputElement>(
            "#conversation-model-search",
        );
        if (search) {
            search.value = "";
            renderComposerModels();
            search.focus();
        }
        return;
    }
    const filter = event.target.closest<HTMLButtonElement>(
        "#conversation-model-favourites-filter",
    );
    if (filter) {
        filter.ariaPressed = String(filter.ariaPressed !== "true");
        renderComposerModels();
        return;
    }
    const favourite = event.target.closest<HTMLButtonElement>(
        "[data-model-favourite]",
    );
    if (favourite) {
        if (pendingFavourite || commandBlockReason()) return;
        const form = document.querySelector<HTMLFormElement>(
            "#conversation-favourite-form",
        );
        if (!form) return;
        const provider = favourite.dataset.provider ?? "";
        const model = favourite.dataset.modelFavourite ?? "";
        (form.elements.namedItem("provider") as HTMLInputElement).value =
            provider;
        (form.elements.namedItem("model") as HTMLInputElement).value = model;
        pendingFavourite = {
            provider,
            model,
            scroll:
                document.getElementById("conversation-model-results")
                    ?.scrollTop ?? 0,
        };
        form.requestSubmit();
        return;
    }
    const option = event.target.closest<HTMLButtonElement>(
        "[data-composer-model]",
    );
    if (!option || commandBlockReason()) return;
    const provider = option.dataset.provider ?? "";
    const model = option.dataset.composerModel ?? "";
    const selected = composerCatalogue()[provider]?.find(
        (item) => item.id === model,
    );
    if (!selected) return;
    controls.panel.hidePopover();
    controls.toggle.focus();
    if (provider === controls.provider.value && model === controls.model.value)
        return;
    const thinking = controls.thinking.value;
    controls.provider.value = provider;
    controls.model.value = model;
    composerEfforts(thinking);
    saveComposerModel("conversation-model-toggle");
});
document.addEventListener("keydown", (event) => {
    if (!(event.target instanceof HTMLElement)) return;
    const controls = composerControls();
    if (!controls) return;
    const thinkingToggle = document.querySelector<HTMLButtonElement>(
        "#conversation-thinking-toggle",
    );
    const thinkingPanel = document.getElementById(
        "conversation-thinking-options",
    );
    if (
        event.target === thinkingToggle &&
        thinkingToggle &&
        !thinkingToggle.disabled &&
        ["ArrowDown", "ArrowUp"].includes(event.key)
    ) {
        event.preventDefault();
        thinkingPanel?.showPopover();
        return;
    }
    if (thinkingPanel?.contains(event.target)) {
        if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            thinkingPanel.hidePopover();
            thinkingToggle?.focus();
            return;
        }
        const options = Array.from(
            thinkingPanel.querySelectorAll<HTMLButtonElement>(
                "[data-thinking-value]:not(:disabled)",
            ),
        );
        const index = options.indexOf(event.target as HTMLButtonElement);
        if (index < 0) return;
        let next: HTMLButtonElement | undefined;
        if (event.key === "ArrowDown")
            next = options[(index + 1) % options.length];
        else if (event.key === "ArrowUp")
            next = options[(index + options.length - 1) % options.length];
        else if (event.key === "Home") next = options[0];
        else if (event.key === "End") next = options.at(-1);
        else return;
        event.preventDefault();
        next?.focus();
        return;
    }
    if (
        event.target === controls.toggle &&
        ["ArrowDown", "ArrowUp"].includes(event.key)
    ) {
        event.preventDefault();
        controls.panel.showPopover();
        document.getElementById("conversation-model-search")?.focus();
        return;
    }
    if (!controls.panel.contains(event.target)) return;
    if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        controls.panel.hidePopover();
        controls.toggle.focus();
        return;
    }
    const search = document.querySelector<HTMLInputElement>(
        "#conversation-model-search",
    );
    const options = Array.from(
        controls.panel.querySelectorAll<HTMLButtonElement>(
            "[data-composer-model]",
        ),
    );
    if (event.target === search && event.key === "Enter") {
        event.preventDefault();
        const exact = options.filter(
            (option) =>
                option.dataset.composerModel?.toLocaleLowerCase() ===
                search.value.trim().toLocaleLowerCase(),
        );
        (exact.length === 1
            ? exact[0]
            : options.length === 1
              ? options[0]
              : undefined
        )?.click();
        return;
    }
    const index = options.indexOf(event.target as HTMLButtonElement);
    if (event.target !== search && index < 0) return;
    let next: HTMLElement | undefined;
    if (event.key === "ArrowDown") next = options[(index + 1) % options.length];
    else if (event.key === "ArrowUp")
        next =
            index === 0
                ? (search ?? undefined)
                : options.at(index < 0 ? -1 : index - 1);
    else if (index >= 0 && event.key === "Home") next = options[0];
    else if (index >= 0 && event.key === "End") next = options.at(-1);
    else return;
    event.preventDefault();
    next?.focus();
});

listenForRequestSettled((detail) => {
    if (
        detail.outcome === "applied-patch" &&
        detail.targetIds.includes("conversation-detail")
    )
        syncThinkingChoice();
    if (pendingFavourite && detail.form.id === "conversation-favourite-form") {
        const pending = pendingFavourite;
        pendingFavourite = undefined;
        renderComposerModels();
        const results = document.getElementById("conversation-model-results");
        if (results) results.scrollTop = pending.scroll;
        const button = Array.from(
            document.querySelectorAll<HTMLButtonElement>(
                "[data-model-favourite]",
            ),
        ).find(
            (item) =>
                item.dataset.provider === pending.provider &&
                item.dataset.modelFavourite === pending.model,
        );
        (button ?? document.getElementById("conversation-model-search"))?.focus(
            { preventScroll: true },
        );
        if (detail.outcome !== "applied-patch") {
            const status = document.querySelector(
                "[data-model-favourite-status]",
            );
            if (status)
                status.textContent =
                    "Power Plant could not save the favourite. Try again.";
        }
    }
    if (pendingModel && detail.form.id === "conversation-model-form") {
        const pending = pendingModel;
        pendingModel = undefined;
        modelLocks.forEach(({ control, disabled }) => {
            if (control.isConnected) control.disabled = disabled;
        });
        modelLocks = [];
        if (detail.outcome !== "applied-patch") {
            const controls = composerControls();
            if (controls) {
                controls.provider.value = pending.provider;
                controls.model.value = pending.model;
                composerEfforts(pending.thinking);
            }
        }
        const currentForm = document.querySelector<HTMLFormElement>(
            "#conversation-model-form",
        );
        if (currentForm?.action === detail.form.action) {
            const message =
                document.querySelector<HTMLTextAreaElement>(
                    "#composer-message",
                );
            if (message) {
                message.value = pending.message;
                message.dispatchEvent(new Event("input", { bubbles: true }));
            }
            document
                .getElementById(pending.focus)
                ?.focus({ preventScroll: true });
        }
    }
});

type ReviewCatalogueModel = {
    id: string;
    default_effort: string;
    efforts: { value: string; label: string }[];
};

// Independent review pages offer the conversation model catalogue through
// constrained selects. Provider and model changes refresh the dependent
// options from the embedded catalogue. The server still validates every
// choice, so native submission works without this enhancement.
function reviewCatalogue(
    section: HTMLElement,
): Record<string, ReviewCatalogueModel[]> {
    const source = section.parentElement?.querySelector<HTMLElement>(
        "[data-review-model-catalogue]",
    );
    const raw = source?.dataset.reviewModelCatalogue ?? "{}";
    try {
        const parsed: unknown = JSON.parse(raw);
        if (parsed && typeof parsed === "object" && !Array.isArray(parsed))
            return parsed as Record<string, ReviewCatalogueModel[]>;
    } catch {
        // A malformed catalogue keeps the server-rendered options.
    }
    return {};
}

function syncReviewEfforts(
    section: HTMLElement,
    catalogue: Record<string, ReviewCatalogueModel[]>,
) {
    const provider = section.querySelector<HTMLSelectElement>(
        "[data-review-provider]",
    );
    const model = section.querySelector<HTMLSelectElement>(
        "[data-review-model]",
    );
    const thinking = section.querySelector<HTMLSelectElement>(
        "[data-review-thinking]",
    );
    if (!provider || !model || !thinking) return;
    const models = catalogue[provider.value] ?? [];
    const selected = models.find((item) => item.id === model.value);
    const efforts = selected?.efforts ?? [];
    const previous = thinking.value;
    thinking.replaceChildren(
        ...(efforts.length
            ? efforts.map((effort) => new Option(effort.label, effort.value))
            : [new Option("Not available", "")]),
    );
    thinking.value = efforts.some((effort) => effort.value === previous)
        ? previous
        : (selected?.default_effort ?? "");
}

function syncReviewModels(section: HTMLElement, resetModel: boolean) {
    const provider = section.querySelector<HTMLSelectElement>(
        "[data-review-provider]",
    );
    const model = section.querySelector<HTMLSelectElement>(
        "[data-review-model]",
    );
    if (!provider || !model) return;
    const catalogue = reviewCatalogue(section);
    const models = catalogue[provider.value] ?? [];
    let selected = model.value;
    if (resetModel && !models.some((item) => item.id === selected))
        selected = models[0]?.id ?? "";
    // A provider change replaces the model list. The server-rendered
    // Unavailable entry never survives a move to another provider.
    if (models.length === 0) {
        if (selected) {
            model.replaceChildren(
                new Option(`Unavailable · ${selected}`, selected),
            );
            model.value = selected;
        } else {
            const empty = new Option("No models available", "");
            empty.disabled = true;
            model.replaceChildren(empty);
            model.value = "";
        }
    } else {
        model.replaceChildren(
            ...models.map((item) => new Option(item.id, item.id)),
        );
        model.value = models.some((item) => item.id === selected)
            ? selected
            : (models[0]?.id ?? "");
    }
    syncReviewEfforts(section, catalogue);
}

function syncExecutionModeFields() {
    const location = document.querySelector<HTMLInputElement>(
        'input[form="conversation-settings-form"][name="location"]:checked',
    );
    if (!location) return;
    const host = location.value === "host";
    document
        .querySelectorAll<HTMLElement>("[data-execution-sandbox-settings]")
        .forEach((section) => {
            section.hidden = host;
        });
    document
        .querySelectorAll<HTMLElement>("[data-execution-host-policy]")
        .forEach((section) => {
            section.hidden = !host;
        });
    document
        .querySelectorAll<HTMLElement>(".segmented label")
        .forEach((label) => {
            const input = label.querySelector<HTMLInputElement>(
                'input[name="location"]',
            );
            if (input) label.classList.toggle("selected", input.checked);
        });
}

function syncNetworkDomains() {
    const select = document.querySelector<HTMLSelectElement>(
        "[data-network-select]",
    );
    const domains = document.querySelector<HTMLElement>(
        "[data-network-domains]",
    );
    if (!select || !domains) return;
    domains.hidden = select.value !== "restricted";
}

function selectConversationSettingsSection(panel: HTMLElement, id: string) {
    panel
        .querySelectorAll<HTMLElement>("[data-settings-panel]")
        .forEach((section) => {
            section.hidden = section.id !== id;
        });
    panel
        .querySelectorAll<HTMLElement>("[data-settings-tab]")
        .forEach((tab) => {
            const selected = tab.dataset.settingsTab === id;
            tab.setAttribute("aria-pressed", String(selected));
            tab.classList.toggle("selected", selected);
        });
    const actions = panel.querySelector<HTMLElement>(
        "[data-execution-actions]",
    );
    if (actions)
        actions.hidden =
            id !== "settings-execution" &&
            !(
                id === "settings-directories" &&
                panel.querySelector("[data-execution-directory]")
            );
}

function revealConversationSetting(target: HTMLElement, focus = true) {
    const panel = target.closest<HTMLElement>("#conversation-settings");
    const scroll = panel?.querySelector<HTMLElement>("[data-settings-scroll]");
    if (!panel || !scroll) return;
    const section = target.closest<HTMLElement>("[data-settings-panel]");
    if (target.closest("[data-execution-actions]"))
        selectConversationSettingsSection(panel, "settings-execution");
    else if (section || target.id === "conversation-settings-heading")
        selectConversationSettingsSection(
            panel,
            section?.id ?? "settings-directories",
        );
    let parent = target.parentElement;
    while (parent && parent !== panel) {
        if (parent instanceof HTMLDetailsElement) parent.open = true;
        parent = parent.parentElement;
    }
    if (!panel.matches(":popover-open")) panel.showPopover();
    if (focus) target.focus({ preventScroll: true });
    scroll.scrollTop =
        scroll.contains(target) && !target.classList.contains("sr-only")
            ? scroll.scrollTop +
              target.getBoundingClientRect().top -
              scroll.getBoundingClientRect().top -
              16
            : 0;
}

document.addEventListener("click", (event) => {
    const shortcut =
        event.target instanceof Element
            ? event.target.closest<HTMLElement>("[data-settings-section]")
            : null;
    const tab =
        event.target instanceof Element
            ? event.target.closest<HTMLElement>("[data-settings-tab]")
            : null;
    if (tab) {
        const panel = tab.closest<HTMLElement>("#conversation-settings");
        if (panel && tab.dataset.settingsTab) {
            selectConversationSettingsSection(panel, tab.dataset.settingsTab);
            panel
                .querySelector<HTMLElement>("[data-settings-scroll]")
                ?.scrollTo({ top: 0 });
        }
        return;
    }
    if (!shortcut) return;
    let target = document.getElementById(
        shortcut.dataset.settingsSection ?? "",
    );
    if (target?.closest("[data-execution-sandbox-settings][hidden]"))
        target = document.getElementById("conversation-execution-heading");
    if (!target) return;
    const panel = target.closest<HTMLElement>("#conversation-settings");
    if (!panel) return;
    if (
        shortcut.getAttribute("popovertargetaction") === "toggle" &&
        panel.matches(":popover-open")
    )
        return;
    selectConversationSettingsSection(
        panel,
        target.closest("[data-settings-panel]")?.id ?? "settings-directories",
    );
    // Native activation retains the trigger for Escape focus restoration.
    requestAnimationFrame(() => revealConversationSetting(target));
});

let settingsScroll: number | undefined;
let settingsSection: string | undefined;
document.addEventListener(
    "submit",
    () => {
        const panel = document.querySelector<HTMLElement>(
            "#conversation-settings:popover-open",
        );
        settingsScroll = panel?.querySelector<HTMLElement>(
            "[data-settings-scroll]",
        )?.scrollTop;
        settingsSection = panel?.querySelector<HTMLElement>(
            '[data-settings-tab][aria-pressed="true"]',
        )?.dataset.settingsTab;
    },
    true,
);

listenForRequestSettled((detail) => {
    if (
        detail.outcome === "applied-patch" &&
        !detail.targetIds.includes("conversation-detail")
    ) {
        return;
    }
    const previousScroll = settingsScroll;
    const previousSection = settingsSection;
    settingsScroll = undefined;
    settingsSection = undefined;
    syncExecutionModeFields();
    syncNetworkDomains();
    // Retained external submitters can retain transport-only ARIA state after a patch.
    document
        .querySelectorAll<HTMLButtonElement>(
            "#conversation-settings button[form]",
        )
        .forEach((button) => {
            if (
                !button.disabled &&
                !button.hasAttribute("data-graft-submitter-pending")
            )
                button.removeAttribute("aria-disabled");
        });
    const requestedPanel = document.querySelector<HTMLElement>(
        '#conversation-settings[data-settings-open="true"]',
    );
    // A top-layer panel must not conceal command or transport errors.
    document
        .querySelectorAll<HTMLElement>(".conversation-panel:popover-open")
        .forEach((panel) => {
            if (detail.outcome !== "applied-patch" || panel !== requestedPanel)
                panel.hidePopover();
        });
    if (detail.outcome === "applied-patch" && requestedPanel) {
        if (previousSection)
            selectConversationSettingsSection(requestedPanel, previousSection);
        if (!requestedPanel.matches(":popover-open"))
            requestedPanel.showPopover();
        const destination =
            requestedPanel.querySelector<HTMLElement>(
                "[data-execution-switch]:not([hidden]), [data-preset-preview], #conversation-directory-consent",
            ) ??
            (requestedPanel.dataset.directoriesOpen === "true"
                ? document.getElementById("conversation-directory-heading")
                : null);
        const scroll = requestedPanel.querySelector<HTMLElement>(
            "[data-settings-scroll]",
        );
        if (destination) revealConversationSetting(destination);
        else if (scroll && previousScroll !== undefined)
            scroll.scrollTop = previousScroll;
    }
    if (detail.outcome === "applied-patch" && detail.status !== 200) {
        (
            requestedPanel?.querySelector<HTMLElement>('[role="alert"]') ??
            document.querySelector<HTMLElement>("#conversation-error") ??
            requestedPanel
        )?.focus();
    }
});

document.addEventListener("change", (event) => {
    const field = event.target;
    if (
        field instanceof HTMLSelectElement &&
        field.matches("[data-review-provider]")
    ) {
        const section = field.closest<HTMLElement>(
            "[data-review-model-picker]",
        );
        if (section) syncReviewModels(section, true);
    }
    if (
        field instanceof HTMLSelectElement &&
        field.matches("[data-review-model]")
    ) {
        const section = field.closest<HTMLElement>(
            "[data-review-model-picker]",
        );
        if (section) syncReviewEfforts(section, reviewCatalogue(section));
    }
    if (
        field instanceof HTMLSelectElement &&
        field.matches("[data-draft-directory-access]")
    ) {
        const submitter = document.getElementById(
            field.dataset.draftDirectoryAccess ?? "",
        );
        if (submitter instanceof HTMLButtonElement && submitter.form) {
            submitter.value = field.value;
            submitter.form.requestSubmit(submitter);
        }
    }
    if (
        field instanceof HTMLSelectElement &&
        field.id === "conversation-environment"
    ) {
        document
            .querySelectorAll<HTMLElement>("[data-environment-problem]")
            .forEach((problem) => {
                problem.hidden =
                    problem.dataset.environmentProblem !== field.value;
            });
        const status = document.querySelector<HTMLElement>(
            "[data-conversation-environment-status]",
        );
        if (
            status &&
            document.querySelector('[data-conversation-state="new"]')
        ) {
            const label = field.selectedOptions[0]?.dataset.environmentStatus;
            status.textContent = label ?? "";
            status.setAttribute("aria-label", `Status: ${label ?? ""}`);
            status.hidden = !label;
        }
    }
    if (
        field instanceof HTMLInputElement &&
        field.matches("[data-workflow-location]")
    ) {
        const preview = field
            .closest("fieldset")
            ?.querySelector<HTMLButtonElement>(
                "[data-workflow-location-preview]",
            );
        if (preview && field.form) field.form.requestSubmit(preview);
    }
    if (
        field instanceof HTMLInputElement &&
        field.form?.id === "conversation-composer" &&
        (field.name === "location" ||
            field.name === "tool_run" ||
            field.name === "host_approval")
    ) {
        const preview = document.querySelector<HTMLButtonElement>(
            "[data-location-preview]",
        );
        if (preview?.form === field.form) field.form.requestSubmit(preview);
    }
    if (
        field instanceof HTMLSelectElement &&
        field.matches("[data-network-select]")
    ) {
        syncNetworkDomains();
    }
    if (
        field instanceof HTMLInputElement &&
        field.name === "location" &&
        field.form?.id === "conversation-settings-form"
    ) {
        syncExecutionModeFields();
    }
    if (
        field instanceof HTMLSelectElement &&
        field.matches("[data-execution-directory]")
    ) {
        const value = document.querySelector<HTMLInputElement>(
            "#execution-directory-access",
        );
        if (value) {
            value.value = JSON.stringify(
                Array.from(
                    document.querySelectorAll<HTMLSelectElement>(
                        "[data-execution-directory]",
                    ),
                    (select) => [
                        select.dataset.executionDirectory,
                        select.value,
                    ],
                ),
            );
            value.dispatchEvent(new Event("input", { bubbles: true }));
        }
    }
    if (
        (event.target instanceof HTMLSelectElement &&
            (event.target.id === "conversation-environment" ||
                event.target.matches("[data-execution-directory]") ||
                event.target.matches("[data-network-select]") ||
                event.target.name === "host_approval")) ||
        (event.target instanceof HTMLInputElement &&
            (event.target.name === "location" ||
                event.target.name === "host_approval"))
    ) {
        document
            .querySelector<HTMLElement>("[data-execution-switch]")
            ?.setAttribute("hidden", "");
    }
});

document.addEventListener(
    "invalid",
    (event) => {
        if (!(event.target instanceof HTMLElement)) return;
        if (event.target.closest("#conversation-settings")) {
            revealConversationSetting(event.target);
            return;
        }
        if (!event.target.closest("#workflow-launch")) return;
        let parent = event.target.parentElement;
        while (parent && parent.id !== "workflow-launch") {
            parent.hidden = false;
            if (parent instanceof HTMLDetailsElement) parent.open = true;
            parent = parent.parentElement;
        }
    },
    true,
);

startApp();

const LIVE_RELOAD_EVENT_STREAM = "/_tower-livereload/event-stream";
const LIVE_RELOAD_CHANNEL = "powerplant-live-reload";

// One event stream per tab can exhaust the browser HTTP/1.1 connection pool.
// Keep the stream in the visible tab. Use BroadcastChannel to notify hidden tabs to reload.
function enableLiveReload() {
    // Retain the document revision across back/forward cache restores.
    let revision: string | null = null;
    window.addEventListener("pageshow", () => {
        let source: EventSource | null = null;
        let reloading = false;
        let checking = false;
        let checkPending = false;
        const lifetime = new AbortController();
        const channel = new BroadcastChannel(LIVE_RELOAD_CHANNEL);

        const closeStream = () => {
            if (!source) return;
            source.close();
            source = null;
        };

        const reload = (broadcast: boolean) => {
            if (reloading) return;
            reloading = true;
            closeStream();
            if (broadcast) channel.postMessage(null);
            channel.close();
            window.location.reload();
        };

        channel.addEventListener("message", () => reload(false));

        const checkRevision = async () => {
            if (reloading || lifetime.signal.aborted) return;
            if (checking) {
                // A reconnect must not rely on a request from before the disconnect.
                checkPending = true;
                return;
            }
            checking = true;
            try {
                const response = await fetch("/_tower-livereload/revision", {
                    cache: "no-store",
                    signal: AbortSignal.any([
                        lifetime.signal,
                        AbortSignal.timeout(5000),
                    ]),
                });
                if (!response.ok) return;
                const current = await response.text();
                if (
                    lifetime.signal.aborted ||
                    !/^[a-f0-9]{32}-\d+$/.test(current)
                )
                    return;
                if (revision !== null && revision !== current) {
                    reload(true);
                } else {
                    revision = current;
                }
            } catch {
                // A server restart can interrupt this request. Reconnect or visibility retries it.
            } finally {
                checking = false;
                if (checkPending) {
                    checkPending = false;
                    void checkRevision();
                }
            }
        };

        const openStream = () => {
            if (source || document.visibilityState !== "visible") return;
            const next = new EventSource(LIVE_RELOAD_EVENT_STREAM);
            source = next;

            next.addEventListener("reload", () => reload(true));

            next.addEventListener("init", () => void checkRevision());
        };

        const onVisibility = () => {
            if (document.visibilityState === "visible") {
                void checkRevision();
                openStream();
            } else {
                closeStream();
            }
        };

        document.addEventListener("visibilitychange", onVisibility);
        window.addEventListener(
            "pagehide",
            () => {
                lifetime.abort();
                document.removeEventListener("visibilitychange", onVisibility);
                closeStream();
                channel.close();
            },
            { once: true },
        );
        // Even a page that starts hidden needs a baseline before its first visible connection.
        void checkRevision();
        openStream();
    });
}

if (import.meta.env.MODE === "development") {
    enableLiveReload();
}
