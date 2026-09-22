import "@fontsource/ibm-plex-sans/latin-400.css";
import "@fontsource/ibm-plex-sans/latin-600.css";
import "@fontsource/ibm-plex-sans/latin-700.css";
import "@fontsource/ibm-plex-mono/latin-400.css";
import "@fontsource/ibm-plex-mono/latin-500.css";
import "./input.css";
import { startApp } from "./hypergraft-bootstrap";
import {
    commandBlockReason,
    listenForLocationChanges,
    listenForLivePatches,
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

// The message body is replaced during live updates, so the copy control is
// delegated at the document and carries no per-element listener.
document.addEventListener("click", (event) => {
    if (!(event.target instanceof Element)) return;
    const button = event.target.closest<HTMLButtonElement>(
        "[data-copy-response]",
    );
    if (!button) return;
    const wrapper = button.closest("[data-response-copy]");
    const source = wrapper?.querySelector<HTMLTemplateElement>(
        "template[data-copy-source]",
    );
    const status =
        wrapper?.querySelector<HTMLElement>("[data-copy-status]") ?? null;
    void copyResponseText(status, source?.content.textContent ?? "");
});

async function copyResponseText(
    status: HTMLElement | null,
    text: string,
): Promise<void> {
    if (status) status.textContent = "";
    if (text === "") {
        if (status) status.textContent = "Nothing to copy";
        return;
    }
    if (!navigator.clipboard?.writeText) {
        if (status) status.textContent = "Clipboard unavailable";
        return;
    }
    try {
        await navigator.clipboard.writeText(text);
        if (status) status.textContent = "Copied";
    } catch {
        if (status) status.textContent = "Copy failed";
    }
}
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

// A revision draft pre-fills the composer with an earlier user prompt. The
// binding lives in hidden fields, so an unrelated patch that re-renders the
// composer must not drop it. The client keeps the last captured target and
// re-applies it after such a patch.
type RevisionTarget = {
    owner: string;
    source: string;
    parent: string;
    activeLeaf: string;
    revision: string;
    text: string;
    cancelHref: string;
};

let revisionTarget: RevisionTarget | null = null;
let cancellingRevision = false;
let ordinaryDraft = "";

function conversationUrl(): string {
    return (
        document.querySelector<HTMLElement>("[data-conversation-url]")?.dataset
            .conversationUrl ?? ""
    );
}

function readRevisionTarget(): RevisionTarget | null {
    const state = document.querySelector<HTMLElement>("[data-revision-state]");
    if (!state) return null;
    const cancel = state.querySelector<HTMLAnchorElement>(
        "[data-cancel-revision]",
    );
    return {
        owner: conversationUrl(),
        source: state.dataset.revisionSource ?? "",
        parent: state.dataset.revisionParent ?? "",
        activeLeaf: state.dataset.revisionActiveLeaf ?? "",
        revision: state.dataset.revisionRevision ?? "",
        text: state.dataset.revisionText ?? "",
        cancelHref: cancel?.getAttribute("href") ?? "",
    };
}

function revisionBanner(target: RevisionTarget): HTMLElement {
    const banner = document.createElement("div");
    banner.className =
        "col-span-2 flex flex-wrap items-center justify-between gap-2 rounded-box border border-base-300 bg-base-200 px-3 py-2";
    banner.dataset.revisionState = "";
    banner.dataset.revisionSource = target.source;
    banner.dataset.revisionParent = target.parent;
    banner.dataset.revisionActiveLeaf = target.activeLeaf;
    banner.dataset.revisionRevision = target.revision;
    banner.dataset.revisionText = target.text;
    banner.setAttribute("role", "status");
    const text = document.createElement("p");
    text.className = "text-sm";
    text.textContent =
        "Revising an earlier prompt. Send creates a new branch. The original prompt and its descendants stay in the tree.";
    const cancel = document.createElement("a");
    cancel.className = "btn btn-ghost btn-xs min-h-11";
    cancel.setAttribute("href", target.cancelHref);
    cancel.dataset.graft = "";
    cancel.dataset.cancelRevision = "";
    cancel.textContent = "Cancel revision";
    banner.append(text, cancel);
    return banner;
}

function clearComposerDraft() {
    const message =
        document.querySelector<HTMLTextAreaElement>("#composer-message");
    if (!message || message.value === "") return;
    message.value = "";
    message.dispatchEvent(new Event("input", { bubbles: true }));
}

function reconcileRevision() {
    const owner = conversationUrl();
    const rendered = readRevisionTarget();
    const initialise =
        rendered !== null &&
        (revisionTarget?.source !== rendered.source ||
            revisionTarget.owner !== rendered.owner);
    if (rendered) {
        revisionTarget = rendered;
    } else if (cancellingRevision || !revisionTarget) {
        if (cancellingRevision) {
            const message =
                document.querySelector<HTMLTextAreaElement>(
                    "#composer-message",
                );
            if (message) {
                message.value = ordinaryDraft;
                message.dispatchEvent(new Event("input", { bubbles: true }));
            }
            ordinaryDraft = "";
        }
        cancellingRevision = false;
        revisionTarget = null;
        return;
    } else if (owner !== revisionTarget.owner) {
        // A different conversation never inherits another draft's target.
        revisionTarget = null;
        cancellingRevision = false;
        return;
    }
    const form = document.querySelector<HTMLFormElement>(
        "#conversation-composer",
    );
    if (!form || !revisionTarget) return;
    form.action = `${revisionTarget.owner}/messages`;
    for (const [name, value] of [
        ["revision", revisionTarget.revision],
        ["revise_source", revisionTarget.source],
        ["revise_parent", revisionTarget.parent],
        ["revise_active_leaf", revisionTarget.activeLeaf],
    ] as const) {
        const existing = form.elements.namedItem(name);
        const input =
            existing instanceof HTMLInputElement
                ? existing
                : document.createElement("input");
        if (!(existing instanceof HTMLInputElement)) {
            input.type = "hidden";
            input.name = name;
            form.append(input);
        }
        input.value = value;
    }
    if (!form.querySelector("[data-revision-state]")) {
        form.prepend(revisionBanner(revisionTarget));
    }
    const message =
        form.querySelector<HTMLTextAreaElement>("#composer-message");
    if (initialise && message && message.value === "") {
        message.value = revisionTarget.text;
        message.dispatchEvent(new Event("input", { bubbles: true }));
    }
}

// Explicit consent prevents a link from silently replacing unrelated input.
document.addEventListener(
    "click",
    (event) => {
        if (!(event.target instanceof Element)) return;
        if (
            event instanceof MouseEvent &&
            (event.button !== 0 ||
                event.ctrlKey ||
                event.metaKey ||
                event.shiftKey ||
                event.altKey)
        )
            return;
        const revise = event.target.closest("[data-revise-prompt]");
        if (revise) {
            const message =
                document.querySelector<HTMLTextAreaElement>(
                    "#composer-message",
                );
            const draft = message?.value ?? "";
            const current = readRevisionTarget();
            const href = revise.getAttribute("href") ?? "";
            if (
                draft.trim() !== "" &&
                !(current && href.includes(current.source))
            ) {
                const confirm = (
                    globalThis as {
                        confirm?: (message: string) => boolean;
                    }
                ).confirm;
                // An unavailable confirmation blocks the navigation instead of
                // discarding the unsent draft silently.
                if (
                    typeof confirm !== "function" ||
                    !confirm(
                        "Revise the earlier prompt? Cancel revision restores the unsent message.",
                    )
                ) {
                    event.preventDefault();
                    event.stopImmediatePropagation();
                    return;
                }
            }
            if (!current) ordinaryDraft = draft;
            revisionTarget = null;
            clearComposerDraft();
            cancellingRevision = false;
            return;
        }
        const cancel = event.target.closest("[data-cancel-revision]");
        if (cancel) {
            cancellingRevision = true;
            clearComposerDraft();
        }
    },
    true,
);

listenForRequestSettled((detail) => {
    if (detail.outcome !== "applied-patch") return;
    if (
        !detail.targetIds.includes("conversation-detail") &&
        !detail.targetIds.includes("composer") &&
        !detail.targetIds.includes("chat-main")
    )
        return;
    if (
        detail.status === 200 &&
        revisionTarget &&
        new URL(detail.url, window.location.href).pathname ===
            `${revisionTarget.owner}/messages`
    ) {
        revisionTarget = null;
        cancellingRevision = false;
        ordinaryDraft = "";
        return;
    }
    reconcileRevision();
});

// Use the multipart form to retain Hypergraft's unsafe-command guard.
const SUPPORTED_IMAGE_TYPES = new Set([
    "image/png",
    "image/jpeg",
    "image/webp",
]);

let pendingAttachmentUpload:
    { form: HTMLFormElement; action: string; warning: string } | undefined;

function attachmentForm(): HTMLFormElement | null {
    return document.querySelector<HTMLFormElement>("[data-attachment-upload]");
}

function attachmentControls(): HTMLElement | null {
    return document.querySelector<HTMLElement>("[data-attachments]");
}

function setAttachmentError(message: string) {
    const error = attachmentControls()?.querySelector<HTMLElement>(
        "[data-attachment-error]",
    );
    if (!error) return;
    error.textContent = message;
    error.hidden = message === "";
}

function setAttachmentPending(pending: boolean) {
    const controls = attachmentControls();
    controls
        ?.querySelectorAll<HTMLElement>("[data-attachment-pending]")
        .forEach((node) => {
            node.hidden = !pending;
        });
    const form = attachmentForm();
    if (form) {
        if (pending) form.setAttribute("aria-busy", "true");
        else form.removeAttribute("aria-busy");
    }
    // A disabled file input is omitted from the multipart body, so the
    // pending state never disables it.
    const input = form?.querySelector<HTMLInputElement>(
        "[data-attachment-input]",
    );
    if (input) {
        if (pending) input.setAttribute("aria-disabled", "true");
        else input.removeAttribute("aria-disabled");
    }
}

function supportedImage(file: File): boolean {
    return SUPPORTED_IMAGE_TYPES.has(file.type.toLocaleLowerCase());
}

function distinctFiles(files: File[]): File[] {
    // Equal metadata does not imply equal image bytes.
    return [...new Set(files)];
}

function insertAtCaret(field: HTMLTextAreaElement, text: string) {
    const start = field.selectionStart ?? field.value.length;
    const end = field.selectionEnd ?? field.value.length;
    field.value = field.value.slice(0, start) + text + field.value.slice(end);
    field.selectionStart = start + text.length;
    field.selectionEnd = start + text.length;
    field.dispatchEvent(new Event("input", { bubbles: true }));
}

function stageAttachmentFiles(files: File[]): boolean {
    const form = attachmentForm();
    const input = form?.querySelector<HTMLInputElement>(
        "[data-attachment-input]",
    );
    if (!form || !input || input.disabled) {
        setAttachmentError(
            "Power Plant cannot attach another image to this message.",
        );
        return false;
    }
    if (typeof DataTransfer === "undefined") {
        setAttachmentError(
            "This browser cannot stage images from the clipboard.",
        );
        return false;
    }
    if (commandBlockReason()) {
        setAttachmentError(
            "Wait for the current command to finish before adding images.",
        );
        return false;
    }
    const transfer = new DataTransfer();
    for (const file of files) transfer.items.add(file);
    input.files = transfer.files;
    input.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
}

function clipboardImages(clipboard: DataTransfer): File[] {
    return distinctFiles(Array.from(clipboard.files ?? []));
}

function stageTransferredFiles(files: File[]) {
    const supported = files.filter(supportedImage);
    const unsupported = files.some((file) => !supportedImage(file));
    const staged = supported.length > 0 && stageAttachmentFiles(supported);
    if (!unsupported) return;
    const warning = "Power Plant can attach PNG, JPEG and WebP images only.";
    if (staged && pendingAttachmentUpload) {
        pendingAttachmentUpload.warning = warning;
    }
    const error = attachmentControls()?.querySelector<HTMLElement>(
        "[data-attachment-error]",
    );
    setAttachmentError(
        [error?.textContent?.trim(), warning].filter(Boolean).join(" "),
    );
}

document.addEventListener("paste", (event) => {
    const field = event.target;
    if (!(field instanceof HTMLTextAreaElement)) return;
    if (field.id !== "composer-message" || field.disabled) return;
    const clipboard = event.clipboardData;
    if (!clipboard) return;
    const files = clipboardImages(clipboard);
    // A text-only paste keeps the browser's ordinary insertion behaviour.
    if (files.length === 0) return;
    event.preventDefault();
    const text = clipboard.getData("text/plain");
    if (text !== "") insertAtCaret(field, text);
    stageTransferredFiles(files);
});

function dropZone(target: EventTarget | null): HTMLElement | null {
    return target instanceof Element
        ? target.closest<HTMLElement>("[data-attachment-drop-zone]")
        : null;
}

function fileDrag(data: DataTransfer | null): boolean {
    if (!data) return false;
    return (
        Array.from(data.types).includes("Files") ||
        (data.files?.length ?? 0) > 0
    );
}

function setDropHint(zone: HTMLElement | null, active: boolean) {
    zone?.querySelector<HTMLElement>(
        "[data-attachment-drop-hint]",
    )?.classList.toggle("hidden", !active);
}

let activeDropZone: HTMLElement | null = null;

document.addEventListener("dragover", (event) => {
    if (!fileDrag(event.dataTransfer)) return;
    const zone = dropZone(event.target);
    if (!zone) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
    if (activeDropZone !== zone) {
        setDropHint(activeDropZone, false);
        activeDropZone = zone;
    }
    setDropHint(zone, true);
});

document.addEventListener("dragleave", (event) => {
    const zone = activeDropZone;
    if (!zone) return;
    const next = event.relatedTarget;
    if (next instanceof Node && zone.contains(next)) return;
    setDropHint(zone, false);
    activeDropZone = null;
});

document.addEventListener("drop", (event) => {
    const zone = dropZone(event.target);
    if (!zone || !fileDrag(event.dataTransfer)) return;
    event.preventDefault();
    setDropHint(activeDropZone ?? zone, false);
    activeDropZone = null;
    const files = distinctFiles(Array.from(event.dataTransfer?.files ?? []));
    stageTransferredFiles(files);
});

document.addEventListener("change", (event) => {
    const input = (event.target as Element | null)?.closest<HTMLInputElement>(
        "[data-attachment-input]",
    );
    if (!input || (input.files?.length ?? 0) === 0) return;
    const form = input.closest("form");
    if (!form) return;
    if (commandBlockReason()) {
        setAttachmentError(
            "Wait for the current command to finish before adding images.",
        );
        return;
    }
    pendingAttachmentUpload = { form, action: form.action, warning: "" };
    setAttachmentError("");
    setAttachmentPending(true);
    form.requestSubmit();
});

// A completion belongs to the draft that started it. A settlement from an
// earlier scope or a superseded form is ignored. A failure never retries.
listenForRequestSettled((detail) => {
    if (!detail.form.matches("[data-attachment-upload]")) return;
    const initiated = pendingAttachmentUpload;
    if (!initiated || initiated.form !== detail.form) return;
    const form = attachmentForm();
    if (!form || form.action !== initiated.action) {
        pendingAttachmentUpload = undefined;
        return;
    }
    pendingAttachmentUpload = undefined;
    setAttachmentPending(false);
    const input = form.querySelector<HTMLInputElement>(
        "[data-attachment-input]",
    );
    if (detail.outcome === "applied-patch" && detail.status === 200) {
        if (initiated.warning) {
            const error = attachmentControls()?.querySelector<HTMLElement>(
                "[data-attachment-error]",
            );
            setAttachmentError(
                [error?.textContent?.trim(), initiated.warning]
                    .filter(Boolean)
                    .join(" "),
            );
        }
        if (input) input.value = "";
        return;
    }
    setAttachmentError(
        "Power Plant could not confirm the upload. Reload this page before another attempt.",
    );
});

// At-sign file search. A suggestion contains a scope label and a
// model-visible path. Selection is distinct from Send, and a response for an
// older caret or conversation is discarded.
type FileSuggestion = {
    path: string;
    scope: string;
    directory: boolean;
};

type FileToken = {
    start: number;
    end: number;
    text: string;
};

let fileLookupSequence = 0;
let fileSuggestions: FileSuggestion[] = [];
let fileSelection = -1;
let fileRequest: AbortController | null = null;
let fileDebounce: number | undefined;
let fileSuggestionState: string | null = null;

function fileState(): string | null {
    const field = fileField();
    const token = field ? fileTokenAt(field) : null;
    if (!field || !token) return null;
    return JSON.stringify([
        field.value,
        token.start,
        token.end,
        fileLookupUrl(token),
        document.querySelector<HTMLInputElement>(
            '#conversation-composer input[name="revision"]',
        )?.value,
    ]);
}

function fileField(): HTMLTextAreaElement | null {
    return document.querySelector<HTMLTextAreaElement>("#composer-message");
}

function fileContainer(): HTMLElement | null {
    return document.querySelector<HTMLElement>("[data-file-suggestions]");
}

function fileTokenAt(field: HTMLTextAreaElement): FileToken | null {
    if (field.disabled) return null;
    const caret = field.selectionStart ?? 0;
    if (field.selectionEnd !== caret) return null;
    const value = field.value;
    let index = -1;
    for (let position = caret - 1; position >= 0; position -= 1) {
        const character = value[position];
        if (character === "@") {
            index = position;
            break;
        }
        if (/\s/.test(character)) return null;
    }
    if (index < 0) return null;
    if (index > 0 && !/\s/.test(value[index - 1])) return null;
    return { start: index, end: caret, text: value.slice(index + 1, caret) };
}

function fileInsert(path: string, directory: boolean): string {
    const withSeparator = directory ? `${path}/` : path;
    const quoted = /[\s"\\]/.test(path)
        ? JSON.stringify(withSeparator)
        : withSeparator;
    return `${quoted} `;
}

function closeFileSuggestions() {
    ++fileLookupSequence;
    if (fileDebounce !== undefined) window.clearTimeout(fileDebounce);
    fileDebounce = undefined;
    fileSuggestionState = null;
    fileSuggestions = [];
    fileSelection = -1;
    fileRequest?.abort();
    fileRequest = null;
    const container = fileContainer();
    if (container) {
        container.hidden = true;
        container.replaceChildren();
    }
    const field = fileField();
    field?.removeAttribute("aria-expanded");
    field?.removeAttribute("aria-controls");
}

function renderFileSuggestions(payload: {
    suggestions?: FileSuggestion[];
    message?: string;
}) {
    const container = fileContainer();
    if (!container) return;
    fileSuggestions = payload.suggestions ?? [];
    fileSelection = fileSuggestions.length > 0 ? 0 : -1;
    const nodes: Node[] = [];
    if (payload.message) {
        const status = document.createElement("p");
        status.className = "text-quiet px-3 py-2 text-xs";
        status.setAttribute("role", "status");
        status.textContent = payload.message;
        nodes.push(status);
    }
    if (fileSuggestions.length === 0) {
        if (!payload.message) {
            const status = document.createElement("p");
            status.className = "text-quiet px-3 py-2 text-xs";
            status.setAttribute("role", "status");
            status.textContent = "No matching files.";
            nodes.push(status);
        }
    } else {
        const list = document.createElement("ul");
        list.id = "conversation-file-options";
        list.className =
            "menu menu-sm max-h-[min(18rem,40dvh)] w-full flex-nowrap overflow-y-auto";
        list.setAttribute("role", "listbox");
        list.setAttribute("aria-label", "File suggestions");
        fileSuggestions.forEach((suggestion, index) => {
            const item = document.createElement("li");
            item.id = `conversation-file-option-${index}`;
            item.setAttribute("role", "option");
            item.setAttribute(
                "aria-selected",
                index === fileSelection ? "true" : "false",
            );
            const button = document.createElement("button");
            button.type = "button";
            button.className =
                "flex w-full min-w-0 items-center justify-between gap-3 text-left";
            button.dataset.fileSuggestion = "";
            button.dataset.filePath = suggestion.path;
            button.dataset.fileDirectory = String(suggestion.directory);
            const label = document.createElement("span");
            label.className = "truncate";
            label.textContent = suggestion.directory
                ? `${suggestion.path}/`
                : suggestion.path;
            const scope = document.createElement("span");
            scope.className = "text-quiet shrink-0 text-xs";
            scope.textContent = suggestion.scope;
            button.append(label, scope);
            item.append(button);
            list.append(item);
        });
        nodes.push(list);
    }
    container.replaceChildren(...nodes);
    container.hidden = false;
    const field = fileField();
    field?.setAttribute("aria-expanded", "true");
    field?.setAttribute("aria-controls", "conversation-file-options");
}

function updateFileSelection(delta: number) {
    if (fileSuggestions.length === 0) return;
    fileSelection =
        (fileSelection + delta + fileSuggestions.length) %
        fileSuggestions.length;
    fileContainer()
        ?.querySelectorAll<HTMLElement>('[role="option"]')
        .forEach((option, index) => {
            option.setAttribute(
                "aria-selected",
                index === fileSelection ? "true" : "false",
            );
            if (index === fileSelection)
                option.scrollIntoView({ block: "nearest" });
        });
}

function chooseFileSuggestion(path: string, directory: boolean) {
    const field = fileField();
    const token = field ? fileTokenAt(field) : null;
    if (!field || !token || fileState() !== fileSuggestionState) {
        closeFileSuggestions();
        return;
    }
    const insert = fileInsert(path, directory);
    field.value =
        field.value.slice(0, token.start) +
        insert +
        field.value.slice(token.end);
    const caret = token.start + insert.length;
    field.focus();
    field.setSelectionRange(caret, caret);
    field.dispatchEvent(new Event("input", { bubbles: true }));
    closeFileSuggestions();
}

function fileLookupUrl(token: FileToken): string | null {
    const owner =
        document.querySelector<HTMLElement>("[data-conversation-url]")?.dataset
            .conversationUrl ?? "";
    const params = new URLSearchParams();
    params.set("q", token.text);
    if (owner !== "") return `${owner}/files?${params.toString()}`;
    const state = document.querySelector<HTMLElement>(
        "[data-conversation-state]",
    )?.dataset.conversationState;
    if (state !== "new") return null;
    const form = document.querySelector<HTMLFormElement>(
        "#conversation-composer",
    );
    if (!form) return null;
    for (const name of [
        "draft_nonce",
        "consent_reference",
        "location",
        "directory_0",
        "directory_1",
        "directory_2",
        "directory_3",
        "directory_4",
        "directory_5",
        "directory_6",
        "directory_7",
    ]) {
        const value = form.elements.namedItem(name);
        if (value instanceof HTMLInputElement && value.value !== "") {
            params.set(name, value.value);
        }
    }
    return `/conversations/new/files?${params.toString()}`;
}

async function requestFileSuggestions(token: FileToken) {
    const field = fileField();
    const current = field ? fileTokenAt(field) : null;
    if (!current || JSON.stringify(current) !== JSON.stringify(token)) return;
    const url = fileLookupUrl(token);
    if (!url) return;
    const state = fileState();
    const sequence = ++fileLookupSequence;
    fileRequest?.abort();
    const controller = new AbortController();
    fileRequest = controller;
    try {
        const response = await fetch(url, {
            headers: { Accept: "application/json" },
            credentials: "same-origin",
            signal: controller.signal,
        });
        const payload = await response.json();
        if (sequence !== fileLookupSequence || state !== fileState()) return;
        fileSuggestionState = state;
        renderFileSuggestions(payload);
    } catch (error) {
        if ((error as { name?: string } | null)?.name === "AbortError") return;
        if (sequence !== fileLookupSequence) return;
        closeFileSuggestions();
    }
}

function scheduleFileSuggestions() {
    closeFileSuggestions();
    const field = fileField();
    const token = field ? fileTokenAt(field) : null;
    if (!token) {
        closeFileSuggestions();
        return;
    }
    if (fileDebounce !== undefined) window.clearTimeout(fileDebounce);
    fileDebounce = window.setTimeout(() => {
        fileDebounce = undefined;
        void requestFileSuggestions(token);
    }, 120);
}

document.addEventListener("input", (event) => {
    if (!(event.target instanceof HTMLTextAreaElement)) return;
    if (event.target.id !== "composer-message") return;
    scheduleFileSuggestions();
});

document.addEventListener(
    "keydown",
    (event) => {
        if (!(event.target instanceof HTMLTextAreaElement)) return;
        if (event.target.id !== "composer-message") return;
        if (event.isComposing || event.ctrlKey || event.metaKey || event.altKey)
            return;
        if (fileSuggestionState !== fileState()) closeFileSuggestions();
        if (fileSuggestions.length === 0) {
            if (event.key === "Escape") closeFileSuggestions();
            return;
        }
        if (event.key === "ArrowDown") {
            updateFileSelection(1);
        } else if (event.key === "ArrowUp") {
            updateFileSelection(-1);
        } else if (event.key === "Enter") {
            const selected = fileSuggestions[fileSelection];
            if (selected) {
                chooseFileSuggestion(selected.path, selected.directory);
            }
        } else if (event.key === "Escape") {
            closeFileSuggestions();
        } else {
            return;
        }
        // Selection never reaches Send or ordinary focus traversal.
        event.preventDefault();
        event.stopImmediatePropagation();
    },
    true,
);

document.addEventListener("click", (event) => {
    if (!(event.target instanceof Element)) return;
    const button = event.target.closest<HTMLElement>("[data-file-suggestion]");
    if (!button) return;
    const path = button.dataset.filePath ?? "";
    if (path === "") return;
    event.preventDefault();
    chooseFileSuggestion(path, button.dataset.fileDirectory === "true");
});

document.addEventListener("pointerdown", (event) => {
    if (!(event.target instanceof Element)) return;
    if (event.target.closest("[data-file-suggestions]")) return;
    if (
        event.target instanceof HTMLTextAreaElement &&
        event.target.id === "composer-message"
    )
        return;
    closeFileSuggestions();
});

document.addEventListener("selectionchange", () => {
    if (fileSuggestionState !== null && fileSuggestionState !== fileState()) {
        closeFileSuggestions();
    }
});
listenForLivePatches(() => {
    if (fileSuggestionState !== fileState()) closeFileSuggestions();
});
listenForLocationChanges(() => closeFileSuggestions());

startApp();
reconcileRevision();
listenForLivePatches(() => reconcileRevision());

// Link navigation emits no settlement, so the revision target is reconciled
// from the location change. This listener runs after startApp, so the
// composer island has already restored its own draft.
listenForLocationChanges(() => {
    reconcileRevision();
});

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
