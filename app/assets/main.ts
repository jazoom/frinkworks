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
    deprecated: boolean;
    image_input: boolean;
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
    const imagesOnly =
        document.getElementById("conversation-model-images-filter")
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
                        !model.deprecated &&
                        (!filter.value || filter.value === provider) &&
                        (!favouritesOnly || model.favourite) &&
                        (!imagesOnly || model.image_input) &&
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
        const meta = document.createElement("span");
        meta.className = "flex flex-wrap items-center gap-2 text-xs text-quiet";
        const provider = document.createElement("span");
        provider.textContent = item.label;
        meta.append(provider);
        if (item.image_input) {
            const images = document.createElement("span");
            images.className = "badge badge-ghost badge-xs";
            images.textContent = "Images";
            meta.append(images);
        }
        label.append(name, meta);
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
            : imagesOnly
              ? "No image-capable models match. Turn off Images to see all models."
              : favouritesOnly
                ? "No favourites match. Turn off Favourites to see all models."
                : filter.value &&
                    !composerCatalogue()[filter.value]?.some(
                        (model) => !model.deprecated,
                    )
                  ? "This provider has no models for new conversations. Choose another provider or refresh the catalogue in Settings."
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
    const retirement = document.querySelector<HTMLElement>(
        "[data-model-retirement]",
    );
    if (retirement) retirement.hidden = !selected?.deprecated;
    const notice = document.querySelector<HTMLElement>(
        "[data-model-default-notice]",
    );
    if (notice) notice.hidden = true;
    syncThinkingChoice();
    syncImageCompatibility();
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

// Staged images need a model with known image-input support. The catalogue
// carries that flag; a missing entry stays unknown and is not treated as
// supported. The server repeats the check before dispatch.
const IMAGE_UNSUPPORTED_NOTE =
    "The selected model does not accept images. Choose a model with image input.";
const IMAGE_UNKNOWN_NOTE =
    "Frinkworks cannot confirm image input for the selected model. Choose a model with image input.";

function syncImageCompatibility() {
    const note = document.querySelector<HTMLElement>(
        "[data-image-compatibility]",
    );
    const text = note?.querySelector<HTMLElement>(
        "[data-image-compatibility-text]",
    );
    if (!note || !text) return;
    const controls = composerControls();
    const attached = document.querySelectorAll("[data-attachment]").length;
    let message = "";
    if (controls && attached > 0 && controls.model.value) {
        const selected = composerCatalogue()[controls.provider.value]?.find(
            (item) => item.id === controls.model.value,
        );
        message = !selected
            ? IMAGE_UNKNOWN_NOTE
            : selected.image_input
              ? ""
              : IMAGE_UNSUPPORTED_NOTE;
    }
    text.textContent = message;
    note.hidden = message === "";
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
        const editText = event.target.dataset.queueEditText ?? text;
        if (!message.value.trim() || replace) {
            message.value = editText;
            message.dispatchEvent(new Event("input", { bubbles: true }));
            message.focus();
        }
        // The hidden field keeps the unescaped text so the server identity
        // comparison still matches the queued item.
        editor.value = text;
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
    const imagesFilter = event.target.closest<HTMLButtonElement>(
        "#conversation-model-images-filter",
    );
    if (imagesFilter) {
        imagesFilter.ariaPressed = String(imagesFilter.ariaPressed !== "true");
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
    ) {
        syncThinkingChoice();
        syncImageCompatibility();
    }
    if (
        detail.outcome === "applied-patch" &&
        detail.targetIds.includes("conversation-attachment-controls")
    )
        syncImageCompatibility();
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
                    "Frinkworks could not save the favourite. Try again.";
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
    deprecated: boolean;
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
    const models = (catalogue[provider.value] ?? []).filter(
        (model) => !model.deprecated,
    );
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

function syncEnvironmentStatus() {
    const field = document.querySelector<HTMLSelectElement>(
        "#conversation-environment",
    );
    if (!field) return;
    document
        .querySelectorAll<HTMLElement>("[data-environment-problem]")
        .forEach((problem) => {
            problem.hidden = problem.dataset.environmentProblem !== field.value;
        });
}

function syncExecutionModeFields() {
    syncEnvironmentStatus();
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

function syncInstructionsFields() {
    const field = document.querySelector<HTMLTextAreaElement>(
        "#conversation-instructions",
    );
    if (!field) return;
    const limit = Number(field.dataset.instructionLimit);
    const tooLong = new TextEncoder().encode(field.value).length > limit;
    const controls = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/u.test(
        field.value,
    );
    const message = tooLong
        ? "The instructions exceed 32 KiB. Remove some text."
        : controls
          ? "The instructions contain unsupported control characters. Remove those characters."
          : "";
    field.setCustomValidity(message);
    field.setAttribute("aria-invalid", String(!!message));
    const error = document.getElementById("conversation-instructions-error");
    if (error) {
        error.textContent = message;
        error.hidden = !message;
    }
    const saved = !!document.getElementById("conversation-settings-form");
    const host = Array.from(
        document.querySelectorAll<HTMLInputElement>('input[name="location"]'),
    ).some(
        (input) =>
            input.value === "host" &&
            (saved ? input.defaultChecked : input.checked),
    );
    const noTools = !document.querySelector("[data-tool-field]:checked");
    for (const [selector, hidden] of [
        ["[data-instructions-host]", !host || noTools],
        ["[data-instructions-sandbox]", host || noTools],
        ["[data-instructions-no-tools]", !noTools],
    ] as const) {
        const note = document.querySelector<HTMLElement>(selector);
        if (note) note.hidden = hidden;
    }
}

document.addEventListener("input", syncInstructionsFields, true);
document.addEventListener("change", syncInstructionsFields);

function syncNetworkDomains() {
    const select = document.querySelector<HTMLInputElement>(
        "[data-network-select]:checked",
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
    const presetName = panel.querySelector<HTMLInputElement>(
        "#conversation-preset-name",
    );
    if (presetName) {
        presetName.disabled =
            id !== "settings-presets" ||
            !!panel.querySelector<HTMLElement>("#conversation-preset-save")
                ?.hidden;
        validatePresetName();
    }
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
            if (settingsSection !== undefined)
                settingsSection = tab.dataset.settingsTab;
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

function syncExecutionConsent() {
    const form = document.querySelector<HTMLFormElement>(
        "#conversation-settings-form",
    );
    const consent = document.querySelector<HTMLButtonElement>(
        "#conversation-host-consent-preview",
    );
    const note = document.querySelector<HTMLElement>(
        "[data-consent-review-note]",
    );
    if (!form || !consent) return;
    const fields = Array.from(form.elements).filter(
        (field) =>
            field instanceof HTMLInputElement ||
            field instanceof HTMLSelectElement ||
            field instanceof HTMLTextAreaElement,
    );
    const dirty = fields.some((field) => {
        if (
            ![
                "location",
                "host_approval",
                "environment",
                "network",
                "network_domains",
                "directory_access",
            ].includes(field.name)
        )
            return false;
        if (field instanceof HTMLSelectElement)
            return Array.from(field.options).some(
                (option) => option.selected !== option.defaultSelected,
            );
        if (field instanceof HTMLInputElement && field.type === "radio")
            return field.checked !== field.defaultChecked;
        return field.value !== field.defaultValue;
    });
    consent.disabled = dirty;
    if (note) note.hidden = !dirty;
}

document.addEventListener("input", syncExecutionConsent);
document.addEventListener("change", syncExecutionConsent);

function syncExecutionReview() {
    const panel = document.getElementById("conversation-settings");
    if (!panel) return;
    const review = panel.querySelector<HTMLElement>(
        "[data-execution-switch]:not([hidden])",
    );
    panel.classList.toggle("execution-review-open", !!review);
    const title = panel.querySelector<HTMLElement>("[data-setup-title]");
    const reviewTitle = panel.querySelector<HTMLElement>(
        "[data-execution-review-title]",
    );
    if (title) title.hidden = !!review;
    if (reviewTitle) reviewTitle.hidden = !review;
}

function closeHostConsent(policy = false) {
    const dialog = document.querySelector<HTMLDialogElement>(
        "#conversation-host-consent",
    );
    dialog?.close();
    dialog?.remove();
    const request = document.querySelector<HTMLInputElement>(
        '[name="host_consent_request"]',
    );
    if (request) request.value = "";
    const target =
        document.getElementById(
            policy
                ? "conversation-host-approval"
                : "conversation-host-consent-preview",
        ) ?? document.getElementById("conversation-execution-heading");
    if (target) revealConversationSetting(target);
}

document.addEventListener(
    "cancel",
    (event) => {
        if (
            event.target instanceof HTMLDialogElement &&
            event.target.id === "conversation-host-consent"
        ) {
            event.preventDefault();
            closeHostConsent();
        }
    },
    true,
);

document.addEventListener("click", (event) => {
    if (!(event.target instanceof Element)) return;
    const close = event.target.closest<HTMLElement>(
        "[data-host-consent-close]",
    );
    if (close) closeHostConsent(close.dataset.hostConsentClose === "policy");
    if (!event.target.closest("[data-execution-cancel]")) return;
    document.querySelector("[data-execution-switch]")?.remove();
    for (const name of [
        "location",
        "host_approval",
        "environment",
        "directory_access",
        "network",
        "network_domains",
    ]) {
        document
            .querySelectorAll<
                HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement
            >(`[form="conversation-settings-form"][name="${name}"]`)
            .forEach((field) => {
                if (field instanceof HTMLSelectElement) {
                    for (const option of field.options)
                        option.selected = option.defaultSelected;
                } else if (
                    field instanceof HTMLInputElement &&
                    field.type === "radio"
                )
                    field.checked = field.defaultChecked;
                else field.value = field.defaultValue;
                field.dispatchEvent(new Event("input", { bubbles: true }));
            });
    }
    document
        .querySelectorAll<HTMLInputElement>("[data-execution-directory]")
        .forEach((field) => {
            field.checked = field.defaultChecked;
        });
    syncExecutionModeFields();
    syncNetworkDomains();
    syncExecutionReview();
    const heading = document.getElementById("conversation-execution-heading");
    if (heading) revealConversationSetting(heading);
});

function syncPresetSummary() {
    if (!document.querySelector('[data-conversation-state="new"]')) return;
    const form = document.querySelector<HTMLFormElement>(
        "#conversation-composer",
    );
    if (!form) return;
    const data = new FormData(form);
    const value = (name: string) => String(data.get(name) ?? "");
    const provider = document.querySelector<HTMLSelectElement>(
        "#conversation-model-provider-filter",
    );
    const environment = form.elements.namedItem("environment");
    const directories = document.querySelector<HTMLElement>(
        "[data-preset-directories]",
    );
    const values: Record<string, string> = {
        provider:
            Array.from(provider?.options ?? []).find(
                (option) => option.value === value("provider"),
            )?.textContent ?? value("provider"),
        model: value("model"),
        thinking: value("thinking") || "Default / not available",
        tools: ["list", "read", "edit", "write", "run"]
            .filter((tool) => value(`tool_${tool}`))
            .map((tool) => tool[0]!.toUpperCase() + tool.slice(1))
            .join(", "),
        location: value("location") === "host" ? "This computer" : "Sandbox",
        host_approval:
            value("host_approval") === "automatic"
                ? "Automatic (YOLO)"
                : "Ask each time",
        environment:
            environment instanceof HTMLSelectElement
                ? (environment.selectedOptions[0]?.dataset.environmentName ??
                  "None")
                : "None",
        network:
            value("network") === "restricted"
                ? `Restricted: ${value("network_domains")
                      .split(/\r?\n/)
                      .map((domain) => domain.trim())
                      .filter(Boolean)
                      .join(", ")}`
                : value("network") === "public"
                  ? "Public internet"
                  : "Off",
        directories: Array.from(
            directories?.querySelectorAll<HTMLElement>("[data-path]") ?? [],
        )
            .map(
                (directory, index) =>
                    `${index === 0 ? "Start: " : ""}${directory.dataset.path}\n${directory.dataset.access} requested${directory.dataset.available === "false" ? " · Unavailable" : ""}`,
            )
            .join("\n\n"),
        instructions: value("instructions"),
    };
    for (const [key, text] of Object.entries(values)) {
        for (const target of document.querySelectorAll<HTMLElement>(
            `[data-preset-summary="${key}"], [data-preset-current="${key}"]`,
        ))
            target.textContent = text || "None";
        const row = document.querySelector<HTMLElement>(
            `[data-preset-row="${key}"]`,
        );
        if (row) {
            const changed =
                (key === "environment" ? value("environment") : text) !==
                row.dataset.presetRequestedIdentity;
            row.classList.toggle("preset-changed", changed);
            const marker = row.querySelector<HTMLElement>(
                "[data-preset-changed]",
            );
            if (marker) marker.hidden = !changed;
        }
    }
}

function validatePresetName() {
    const field = document.querySelector<HTMLInputElement>(
        "#conversation-preset-name",
    );
    if (!field) return;
    const name = field.value.trim();
    const invalid =
        !field.disabled &&
        (new TextEncoder().encode(name).length > 80 || /[\p{Cc}]/u.test(name));
    const message = invalid
        ? "Use at most 80 UTF-8 bytes without control characters."
        : "";
    field.setCustomValidity(message);
    field.setAttribute("aria-invalid", String(invalid));
    const error = document.getElementById("preset-name-error");
    if (error) {
        error.textContent = message;
        error.hidden = !invalid;
    }
}

function showPresetSave(open: boolean) {
    const panel = document.getElementById("conversation-preset-save");
    const list = document.querySelector<HTMLElement>("[data-preset-list]");
    const field = document.querySelector<HTMLInputElement>(
        "#conversation-preset-name",
    );
    if (panel) panel.hidden = !open;
    if (list)
        list.hidden = open || !!document.querySelector("[data-preset-preview]");
    if (field) field.disabled = !open;
    document
        .querySelector("[data-preset-save-toggle]")
        ?.setAttribute("aria-expanded", String(open));
    validatePresetName();
    syncPresetSummary();
}

let retainedPresetName: string | undefined;
let retainedPresetSave = false;
let submittedPresetName: string | undefined;
document.addEventListener("input", (event) => {
    if (
        event.target instanceof HTMLInputElement &&
        event.target.id === "conversation-preset-name"
    ) {
        retainedPresetName = event.target.value;
        validatePresetName();
    }
});
document.addEventListener("click", (event) => {
    if (!(event.target instanceof Element)) return;
    if (event.target.closest("[data-preset-save-toggle]")) {
        retainedPresetSave = true;
        showPresetSave(true);
        const field = document.getElementById("conversation-preset-name");
        if (field) revealConversationSetting(field);
    } else if (
        event.target.closest("[data-preset-save-cancel], [data-preset-cancel]")
    ) {
        // Cancellation grants no authority and sends no command.
        document.querySelector("[data-preset-preview]")?.remove();
        retainedPresetSave = false;
        showPresetSave(false);
        const heading = document.getElementById("conversation-preset-heading");
        if (heading) revealConversationSetting(heading);
    }
    if (event.target.closest('[data-settings-tab="settings-presets"]'))
        syncPresetSummary();
});
listenForLocationChanges(() => {
    retainedPresetName = undefined;
    retainedPresetSave = false;
    submittedPresetName = undefined;
});

let settingsScroll: number | undefined;
let settingsSection: string | undefined;
document.addEventListener(
    "submit",
    () => {
        const name = document.querySelector<HTMLInputElement>(
            "#conversation-preset-name",
        );
        submittedPresetName = name?.value;
        retainedPresetName = name?.value;
        retainedPresetSave = !!document.querySelector(
            "#conversation-preset-save:not([hidden])",
        );
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
    document
        .querySelector<HTMLDialogElement>("#conversation-host-consent[open]")
        ?.close();
    const previousScroll = settingsScroll;
    const previousSection =
        settingsSection ??
        (detail.url.includes("/settings/host-consent")
            ? "settings-execution"
            : undefined);
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
    if (detail.outcome === "applied-patch") {
        const name = document.querySelector<HTMLInputElement>(
            "#conversation-preset-name",
        );
        const saved =
            detail.url.endsWith("/presets/save") && detail.status === 200;
        const laterName = retainedPresetName !== submittedPresetName;
        if (saved && !laterName) retainedPresetSave = false;
        if (name && retainedPresetName !== undefined)
            name.value = retainedPresetName;
        showPresetSave(
            retainedPresetSave ||
                (detail.status !== 200 && detail.url.endsWith("/presets/save")),
        );
    }
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
                "[data-execution-switch]:not([hidden]), [data-preset-preview], #conversation-preset-save:not([hidden]), #conversation-directory-consent",
            ) ??
            (requestedPanel.dataset.directoriesOpen === "true"
                ? document.getElementById("conversation-directory-heading")
                : detail.url.includes("/presets/")
                  ? document.getElementById("conversation-preset-heading")
                  : null);
        const scroll = requestedPanel.querySelector<HTMLElement>(
            "[data-settings-scroll]",
        );
        if (destination) revealConversationSetting(destination);
        else if (scroll && previousScroll !== undefined)
            scroll.scrollTop = previousScroll;
    }
    syncExecutionReview();
    if (detail.outcome === "applied-patch" && detail.status === 200) {
        const consent = document.querySelector<HTMLDialogElement>(
            "[data-host-consent]",
        );
        if (consent && !consent.open) consent.showModal();
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
        field instanceof HTMLInputElement &&
        field.checked &&
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
        field.matches("[data-command-directory]")
    ) {
        const submitter = document.getElementById("command-directory-submit");
        if (submitter instanceof HTMLButtonElement && submitter.form)
            submitter.form.requestSubmit(submitter);
    }
    if (
        field instanceof HTMLSelectElement &&
        field.id === "conversation-environment"
    ) {
        syncEnvironmentStatus();
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
            field.matches("[data-tool-field], [data-enable-tools]") ||
            field.name === "host_approval")
    ) {
        const preview = document.querySelector<HTMLButtonElement>(
            "[data-location-preview]",
        );
        if (preview?.form === field.form) field.form.requestSubmit(preview);
    }
    if (
        field instanceof HTMLInputElement &&
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
        field instanceof HTMLInputElement &&
        field.checked &&
        field.matches("[data-execution-directory]")
    ) {
        const value = document.querySelector<HTMLInputElement>(
            "#execution-directory-access",
        );
        if (value) {
            value.value = JSON.stringify(
                Array.from(
                    document.querySelectorAll<HTMLInputElement>(
                        "[data-execution-directory]:checked",
                    ),
                    (radio) => [radio.dataset.executionDirectory, radio.value],
                ),
            );
            value.dispatchEvent(new Event("input", { bubbles: true }));
        }
    }
    if (
        (event.target instanceof HTMLSelectElement &&
            (event.target.id === "conversation-environment" ||
                event.target.matches("[data-network-select]") ||
                event.target.name === "host_approval")) ||
        (event.target instanceof HTMLInputElement &&
            (event.target.name === "location" ||
                event.target.name === "host_approval" ||
                event.target.name === "network" ||
                event.target.matches("[data-execution-directory]")))
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

// Mirror only the server's active reply status. A historical window can contain
// a pending entry, but its content does not represent the live tail.
function syncReplyStrip() {
    const strip = document.querySelector<HTMLElement>("[data-reply-strip]");
    const text = strip?.querySelector<HTMLElement>("[data-reply-strip-text]");
    if (!strip || !text) return;
    const reply = document.querySelector("#conversation-history-status")
        ? null
        : document.querySelector<HTMLElement>(
              '#transcript > .conversation-turn > .chat-turn-meta > [data-reply-active="true"]',
          );
    const status = reply?.dataset.replyStatus ?? "";
    if (text.textContent !== status) text.textContent = status;
    strip.hidden = status === "";
    const work = document.querySelector<HTMLElement>(
        "[data-work-reply-status]",
    );
    if (work && status) {
        if (work.textContent !== status) work.textContent = status;
        const retry = document.querySelector<HTMLElement>(
            "[data-work-retry-message]",
        );
        if (retry) {
            const message = reply?.dataset.replyRetryMessage ?? "";
            if (retry.textContent !== message) retry.textContent = message;
            retry.hidden = message === "";
        }
    }
}

function syncComposerActions() {
    const form = document.querySelector<HTMLFormElement>(
        "#conversation-composer",
    );
    const field = form?.querySelector<HTMLTextAreaElement>("#composer-message");
    const send = form?.querySelector<HTMLButtonElement>(
        "[data-composer-submit]",
    );
    const prepared = form?.querySelector<HTMLInputElement>(
        'input[name="prepared_run"]',
    );
    if (field && send) {
        const empty =
            field.value.trim() === "" &&
            !document.querySelector("[data-attachment]") &&
            !prepared?.value;
        send.disabled = field.disabled || empty;
        send.setAttribute("aria-disabled", String(send.disabled));
    }
    const picker = form?.querySelector<HTMLButtonElement>(
        "[data-attachment-picker]",
    );
    if (picker) {
        picker.disabled =
            !document.querySelector("[data-attachment-input]") ||
            attachmentForm()?.getAttribute("aria-busy") === "true";
    }
}

document.addEventListener("input", (event) => {
    if (
        event.target instanceof HTMLTextAreaElement &&
        event.target.id === "composer-message"
    ) {
        syncComposerActions();
    }
});

document.addEventListener("click", (event) => {
    if (!(event.target instanceof Element)) return;
    if (event.target.closest("[data-attachment-picker]")) {
        if (commandBlockReason()) return;
        attachmentForm()
            ?.querySelector<HTMLInputElement>("[data-attachment-input]")
            ?.click();
    }
    if (event.target.closest("[data-composer-insert]")) {
        const help = document.getElementById("composer-help");
        if (help?.matches(":popover-open")) help.hidePopover();
    }
});

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
    syncComposerActions();
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
            "Frinkworks cannot attach another image to this message.",
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
    const warning = "Frinkworks can attach PNG, JPEG and WebP images only.";
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
        "Frinkworks could not confirm the upload. Reload this page before another attempt.",
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
        field.selectionStart,
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
    let index = 0;
    while (index < value.length) {
        while (index < value.length && /\s/.test(value[index])) index += 1;
        if (index >= value.length) break;
        const start = index;
        let quoted = false;
        let escaped = false;
        while (index < value.length) {
            const character = value[index];
            if (character === "\n" || character === "\r") break;
            if (escaped) {
                escaped = false;
                index += 1;
                continue;
            }
            if (character === "\\") {
                escaped = true;
                index += 1;
                continue;
            }
            if (character === '"') {
                quoted = !quoted;
                index += 1;
                continue;
            }
            if (!quoted && /\s/.test(character)) break;
            index += 1;
        }
        const end = index;
        if (caret >= start && caret <= end) {
            const raw = value.slice(start, end);
            if (!raw.startsWith("@")) return null;
            if (caret <= start) return null;
            return {
                start,
                end,
                text: decodeFileToken(value.slice(start + 1, caret)),
            };
        }
    }
    return null;
}

function decodeFileToken(raw: string): string {
    let text = raw;
    if (text.startsWith('"')) {
        text = text.slice(1);
        if (text.endsWith('"')) text = text.slice(0, -1);
    }
    let decoded = "";
    let escaped = false;
    for (const character of text) {
        if (escaped) {
            decoded += character;
            escaped = false;
        } else if (character === "\\") {
            escaped = true;
        } else {
            decoded += character;
        }
    }
    if (escaped) decoded += "\\";
    return decoded;
}

// Directory completion keeps the caret before the closing quote for the next segment.
function fileInsert(
    path: string,
    directory: boolean,
): { text: string; caret: number } {
    const base = directory && !path.endsWith("/") ? `${path}/` : path;
    if (!/[\s"\\]/.test(base)) {
        const text = directory ? `@${base}` : `${base} `;
        return { text, caret: text.length };
    }
    const quoted = JSON.stringify(base);
    if (directory) return { text: `@${quoted}`, caret: quoted.length };
    return { text: `${quoted} `, caret: quoted.length + 1 };
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
            label.textContent =
                suggestion.directory && !suggestion.path.endsWith("/")
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
        insert.text +
        field.value.slice(token.end);
    const caret = token.start + insert.caret;
    field.focus();
    field.setSelectionRange(caret, caret);
    field.dispatchEvent(new Event("input", { bubbles: true }));
    // The input event schedules the next segment after directory selection.
    if (!directory) closeFileSuggestions();
}

function fileLookupUrl(token: FileToken): string | null {
    const owner =
        document.querySelector<HTMLElement>("[data-conversation-url]")?.dataset
            .conversationUrl ?? "";
    const params = new URLSearchParams();
    params.set("q", token.text);
    if (token.text.includes("/")) params.set("mode", "complete");
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
        } else if (event.key === "Tab") {
            if (event.shiftKey) return;
            const selected = fileSuggestions[fileSelection];
            if (!selected) return;
            chooseFileSuggestion(selected.path, selected.directory);
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

// Slash skill commands. The leading token selects the command. A complete
// reference previews its source, and the frozen hash binds submission.
type CommandSuggestion = {
    command: string;
    name: string;
    scope: string;
    source_label?: string;
    description: string;
    kind?: string;
    hint?: string;
};

type CommandPreview = {
    binding: string;
    command: string;
    scope: string;
    source_label?: string;
    source: string;
    hash: string;
    base: string;
    expanded: string;
    kind?: string;
    hint?: string;
};

type CommandPlan = { mode: "suggest" | "preview"; query: string };

type CommandToken = { start: number; end: number; text: string };

let commandLookupSequence = 0;
let commandSuggestions: CommandSuggestion[] = [];
let commandSelection = -1;
let commandRequest: AbortController | null = null;
let commandDebounce: number | undefined;
let commandSuggestionState: string | null = null;
let commandBoundPreview: CommandPreview | null = null;
let commandBoundUrl: string | null = null;

function commandField(): HTMLTextAreaElement | null {
    return document.querySelector<HTMLTextAreaElement>("#composer-message");
}

function commandContainer(): HTMLElement | null {
    return document.querySelector<HTMLElement>("[data-command-suggestions]");
}

function commandSourceInput(): HTMLInputElement | null {
    return document.querySelector<HTMLInputElement>(
        'input[name="command_source"]',
    );
}

function commandHashInput(): HTMLInputElement | null {
    return document.querySelector<HTMLInputElement>(
        'input[name="command_hash"]',
    );
}

function commandLeadingToken(value: string): CommandToken | null {
    let index = 0;
    while (index < value.length && /\s/.test(value[index] ?? "")) index += 1;
    if (index >= value.length) return null;
    const start = index;
    while (index < value.length && !/\s/.test(value[index] ?? "")) index += 1;
    return { start, end: index, text: value.slice(start, index) };
}

function commandPlan(): CommandPlan | null {
    const field = commandField();
    if (!field || field.disabled) return null;
    const caret = field.selectionStart ?? 0;
    if (field.selectionEnd !== caret) return null;
    const token = commandLeadingToken(field.value);
    if (!token || !token.text.startsWith("/")) return null;
    if (caret <= token.end) return { mode: "suggest", query: token.text };
    return { mode: "preview", query: commandPreviewQuery(field.value, token) };
}

function commandPreviewQuery(value: string, token: CommandToken): string {
    return token.text.startsWith("/skill:") ? token.text : value;
}

function commandLookupUrl(plan: CommandPlan): string | null {
    const owner =
        document.querySelector<HTMLElement>("[data-conversation-url]")?.dataset
            .conversationUrl ?? "";
    const params = new URLSearchParams();
    params.set("q", plan.query);
    params.set("mode", plan.mode);
    if (owner !== "") return `${owner}/commands?${params.toString()}`;
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
    return `/conversations/new/commands?${params.toString()}`;
}

function commandState(plan: CommandPlan, url: string): string | null {
    const field = commandField();
    if (!field) return null;
    return JSON.stringify([
        field.value,
        field.selectionStart,
        plan.mode,
        plan.query,
        url,
    ]);
}

function setCommandPreview(preview: CommandPreview | null) {
    const source = commandSourceInput();
    const hash = commandHashInput();
    if (source) source.value = preview?.binding ?? "";
    if (hash) hash.value = preview?.hash ?? "";
}

function reconcileCommandPreview() {
    const value = commandField()?.value ?? "";
    const token = commandLeadingToken(value);
    const query = token ? commandPreviewQuery(value, token) : null;
    const url = query ? commandLookupUrl({ mode: "preview", query }) : null;
    if (
        !token ||
        query !== commandBoundPreview?.command ||
        url !== commandBoundUrl
    ) {
        commandBoundPreview = null;
        commandBoundUrl = null;
    }
    setCommandPreview(commandBoundPreview);
}

function closeCommandSuggestions() {
    ++commandLookupSequence;
    if (commandDebounce !== undefined) window.clearTimeout(commandDebounce);
    commandDebounce = undefined;
    commandSuggestionState = null;
    commandSuggestions = [];
    commandSelection = -1;
    commandRequest?.abort();
    commandRequest = null;
    const container = commandContainer();
    if (container) {
        container.hidden = true;
        container.replaceChildren();
    }
    const field = commandField();
    field?.removeAttribute("aria-expanded");
    field?.removeAttribute("aria-controls");
}

function renderCommandSuggestions(payload: {
    suggestions?: CommandSuggestion[];
    preview?: CommandPreview | null;
    message?: string;
}) {
    const container = commandContainer();
    if (!container) return;
    commandSuggestions = payload.suggestions ?? [];
    commandSelection = commandSuggestions.length > 0 ? 0 : -1;
    if (payload.preview) {
        commandBoundPreview = payload.preview;
        commandBoundUrl = commandLookupUrl({
            mode: "preview",
            query: payload.preview.command,
        });
        setCommandPreview(payload.preview);
    }
    const nodes: Node[] = [];
    if (payload.preview) {
        const prompt = payload.preview.kind === "prompt";
        const status = document.createElement("p");
        status.className = "text-quiet px-3 py-2 text-xs";
        status.setAttribute("role", "status");
        status.textContent = `${prompt ? "Prompt" : "Skill"} ready: ${payload.preview.source}`;
        nodes.push(status);
        if (payload.preview.hint) {
            const hint = document.createElement("p");
            hint.className = "text-quiet px-3 text-xs";
            hint.setAttribute("role", "status");
            hint.textContent = `Hint: ${payload.preview.hint}`;
            nodes.push(hint);
        }
        const details = document.createElement("details");
        details.className = "px-3 pb-2 text-xs";
        const summary = document.createElement("summary");
        summary.textContent = prompt ? "Prompt preview" : "Skill preview";
        const body = document.createElement("pre");
        body.className = "max-h-64 overflow-auto whitespace-pre-wrap";
        body.textContent = payload.preview.expanded;
        details.append(summary, body);
        nodes.push(details);
    } else if (payload.message) {
        const status = document.createElement("p");
        status.className = "text-quiet px-3 py-2 text-xs";
        status.setAttribute("role", "status");
        status.textContent = payload.message;
        nodes.push(status);
    }
    if (commandSuggestions.length === 0) {
        if (!payload.message && !payload.preview) {
            const status = document.createElement("p");
            status.className = "text-quiet px-3 py-2 text-xs";
            status.setAttribute("role", "status");
            status.textContent = "No matching commands.";
            nodes.push(status);
        }
    } else {
        const list = document.createElement("ul");
        list.id = "conversation-command-options";
        list.className =
            "menu menu-sm max-h-[min(18rem,40dvh)] w-full flex-nowrap overflow-y-auto";
        list.setAttribute("role", "listbox");
        list.setAttribute("aria-label", "Command suggestions");
        commandSuggestions.forEach((suggestion, index) => {
            const item = document.createElement("li");
            item.id = `conversation-command-option-${index}`;
            item.setAttribute("role", "option");
            item.setAttribute(
                "aria-selected",
                index === commandSelection ? "true" : "false",
            );
            const button = document.createElement("button");
            button.type = "button";
            button.className =
                "flex w-full min-w-0 flex-col items-start gap-1 text-left";
            button.dataset.commandSuggestion = "";
            button.dataset.command = suggestion.command;
            button.dataset.commandKind = suggestion.kind ?? "skill";
            const top = document.createElement("span");
            top.className = "flex w-full items-center justify-between gap-3";
            const label = document.createElement("span");
            label.className = "truncate font-medium";
            label.textContent = suggestion.command;
            const scope = document.createElement("span");
            scope.className = "text-quiet shrink-0 text-xs";
            scope.textContent = suggestion.source_label ?? suggestion.scope;
            top.append(label, scope);
            const description = document.createElement("span");
            description.className = "text-quiet line-clamp-2 text-xs";
            description.textContent = suggestion.description;
            button.append(top, description);
            if (suggestion.hint) {
                const hint = document.createElement("span");
                hint.className = "text-quiet text-xs";
                hint.textContent = `Hint: ${suggestion.hint}`;
                button.append(hint);
            }
            item.append(button);
            list.append(item);
        });
        nodes.push(list);
    }
    container.replaceChildren(...nodes);
    container.hidden = false;
    const field = commandField();
    field?.setAttribute("aria-expanded", "true");
    field?.setAttribute("aria-controls", "conversation-command-options");
}

function updateCommandSelection(delta: number) {
    if (commandSuggestions.length === 0) return;
    commandSelection =
        (commandSelection + delta + commandSuggestions.length) %
        commandSuggestions.length;
    commandContainer()
        ?.querySelectorAll<HTMLElement>('[role="option"]')
        .forEach((option, index) => {
            option.setAttribute(
                "aria-selected",
                index === commandSelection ? "true" : "false",
            );
            if (index === commandSelection)
                option.scrollIntoView({ block: "nearest" });
        });
}

function chooseCommandSuggestion(command: string) {
    commandBoundPreview = null;
    commandBoundUrl = null;
    const field = commandField();
    if (!field) return;
    const token = commandLeadingToken(field.value);
    if (!token || !token.text.startsWith("/")) return;
    field.value =
        field.value.slice(0, token.start) +
        `${command} ` +
        field.value.slice(token.end);
    const caret = token.start + command.length + 1;
    field.focus();
    field.setSelectionRange(caret, caret);
    field.dispatchEvent(new Event("input", { bubbles: true }));
}

async function requestCommandSuggestions(plan: CommandPlan) {
    const url = commandLookupUrl(plan);
    if (!url) return;
    const state = commandState(plan, url);
    if (!state) return;
    const sequence = ++commandLookupSequence;
    commandRequest?.abort();
    const controller = new AbortController();
    commandRequest = controller;
    try {
        const response = await fetch(url, {
            headers: { Accept: "application/json" },
            credentials: "same-origin",
            signal: controller.signal,
        });
        const payload = await response.json();
        if (sequence !== commandLookupSequence) return;
        const current = commandPlan();
        const currentUrl = current ? commandLookupUrl(current) : null;
        if (
            !current ||
            !currentUrl ||
            commandState(current, currentUrl) !== state
        )
            return;
        commandSuggestionState = state;
        renderCommandSuggestions(payload);
    } catch (error) {
        if ((error as { name?: string } | null)?.name === "AbortError") return;
        if (sequence !== commandLookupSequence) return;
        closeCommandSuggestions();
    }
}

function scheduleCommandSuggestions() {
    closeCommandSuggestions();
    reconcileCommandPreview();
    const plan = commandPlan();
    if (!plan) return;
    if (commandBoundPreview) {
        renderCommandSuggestions({ preview: commandBoundPreview });
        return;
    }
    if (commandDebounce !== undefined) window.clearTimeout(commandDebounce);
    commandDebounce = window.setTimeout(() => {
        commandDebounce = undefined;
        void requestCommandSuggestions(plan);
    }, 120);
}

document.addEventListener("input", (event) => {
    if (!(event.target instanceof HTMLTextAreaElement)) return;
    if (event.target.id !== "composer-message") return;
    scheduleCommandSuggestions();
});

document.addEventListener(
    "keydown",
    (event) => {
        if (!(event.target instanceof HTMLTextAreaElement)) return;
        if (event.target.id !== "composer-message") return;
        if (event.isComposing || event.ctrlKey || event.metaKey || event.altKey)
            return;
        if (commandSuggestions.length === 0) {
            if (event.key === "Escape") closeCommandSuggestions();
            return;
        }
        if (event.key === "ArrowDown") {
            updateCommandSelection(1);
        } else if (event.key === "ArrowUp") {
            updateCommandSelection(-1);
        } else if (event.key === "Enter") {
            const selected = commandSuggestions[commandSelection];
            if (selected) chooseCommandSuggestion(selected.command);
        } else if (event.key === "Tab") {
            if (event.shiftKey) return;
            const selected = commandSuggestions[commandSelection];
            if (!selected) return;
            chooseCommandSuggestion(selected.command);
        } else if (event.key === "Escape") {
            closeCommandSuggestions();
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
    const button = event.target.closest<HTMLElement>(
        "[data-command-suggestion]",
    );
    if (!button) return;
    const command = button.dataset.command ?? "";
    if (command === "") return;
    event.preventDefault();
    chooseCommandSuggestion(command);
});

document.addEventListener("pointerdown", (event) => {
    if (!(event.target instanceof Element)) return;
    if (event.target.closest("[data-command-suggestions]")) return;
    if (
        event.target instanceof HTMLTextAreaElement &&
        event.target.id === "composer-message"
    )
        return;
    closeCommandSuggestions();
});

document.addEventListener("selectionchange", () => {
    if (commandSuggestionState === null) return;
    const plan = commandPlan();
    const url = plan ? commandLookupUrl(plan) : null;
    if (!plan || !url || commandSuggestionState !== commandState(plan, url)) {
        closeCommandSuggestions();
    }
});
listenForLivePatches(() => {
    reconcileCommandPreview();
    const plan = commandPlan();
    const url = plan ? commandLookupUrl(plan) : null;
    if (
        commandSuggestionState !== null &&
        (!plan || !url || commandSuggestionState !== commandState(plan, url))
    ) {
        closeCommandSuggestions();
    }
});
listenForLocationChanges(() => {
    closeCommandSuggestions();
    commandBoundPreview = null;
    commandBoundUrl = null;
    setCommandPreview(null);
});

// A programmatic composer clear emits no input event. Drop a preview binding
// that no longer matches the submitted text before the form serialises it.
document.addEventListener("submit", (event) => {
    if (!(event.target instanceof HTMLFormElement)) return;
    if (event.target.id !== "conversation-composer") return;
    reconcileCommandPreview();
});

function focusWorkflowSelection() {
    const target = document.querySelector<HTMLElement>(
        "#workflow-destination-heading, #workflow-selection-error",
    );
    if (!target) return;
    // Navigation can retain the catalogue's inner scroll offset. The chooser
    // or rejection must not remain above the visible records.
    const content = target
        .closest(".workspace-catalogue")
        ?.querySelector(":scope > div");
    if (content) content.scrollTop = 0;
    target.focus({ preventScroll: true });
}

document
    .querySelector("[data-live-reload]")
    ?.addEventListener("click", () => location.reload());

const stopApp = startApp();
import.meta.hot?.dispose(stopApp);
focusWorkflowSelection();
listenForLocationChanges((detail) => {
    // History restoration owns the saved position, including chooser pages.
    if (detail.cause === "history-traversal") return;
    // The installed runtime focuses its patch target after this event.
    queueMicrotask(focusWorkflowSelection);
});
// Consent must reflect requested fields after the conversation island restores them.
listenForRequestSettled(syncExecutionConsent);
listenForLivePatches(syncExecutionConsent);
syncInstructionsFields();
listenForLocationChanges(syncInstructionsFields);
listenForRequestSettled(syncInstructionsFields);
listenForLivePatches(syncInstructionsFields);
reconcileRevision();
syncImageCompatibility();
syncReplyStrip();
listenForLivePatches(syncReplyStrip);
listenForLocationChanges(syncReplyStrip);
listenForRequestSettled(syncReplyStrip);
document.addEventListener("hypergraft:progress", syncReplyStrip);
syncComposerActions();
listenForLivePatches(syncComposerActions);
listenForLocationChanges(syncComposerActions);
listenForRequestSettled(syncComposerActions);
listenForLivePatches(() => reconcileRevision());

// Link navigation emits no settlement, so the revision target is reconciled
// from the location change. This listener runs after startApp, so the
// composer island has already restored its own draft.
listenForLocationChanges(() => {
    reconcileRevision();
});

const LIVE_RELOAD_EVENT_STREAM = "/_tower-livereload/event-stream";
const LIVE_RELOAD_CHANNEL = "frinkworks-live-reload";

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

function enableSupervisedDevelopment() {
    const endpoint = "/_dev/supervised";
    let phase = "ready";
    let message = "";
    let notice = "";
    let submitting = false;
    let reloading = false;
    let requestVersion = 0;
    const render = () => {
        const busy =
            submitting ||
            ["checking", "building", "restarting"].includes(phase);
        const text = notice || (phase === "ready" ? "" : message);
        document
            .querySelectorAll<HTMLElement>("[data-supervised-development]")
            .forEach((control) => {
                control.hidden = false;
                const button = control.querySelector<HTMLButtonElement>(
                    "[data-supervised-rebuild]",
                );
                const progress = control.querySelector<HTMLElement>(
                    "[data-supervised-progress]",
                );
                const status = control.querySelector<HTMLElement>(
                    "[data-supervised-status]",
                );
                if (button) button.disabled = busy;
                if (progress) progress.hidden = !busy;
                if (status && status.textContent !== text)
                    status.textContent = text;
            });
    };
    const receive = (data: {
        phase: string;
        revision: string;
        message: string;
    }) => {
        phase = data.phase;
        message = data.message;
        if (["checking", "building", "restarting"].includes(phase)) notice = "";
        render();
        if (
            phase === "ready" &&
            data.revision !== import.meta.env.VITE_SUPERVISED_REVISION &&
            !reloading
        ) {
            reloading = true;
            window.location.reload();
        }
    };
    render();
    listenForLocationChanges(render);
    document.addEventListener("click", async (event) => {
        const button =
            event.target instanceof Element
                ? event.target.closest<HTMLButtonElement>(
                      "[data-supervised-rebuild]",
                  )
                : null;
        if (!button || button.disabled || submitting) return;
        submitting = true;
        requestVersion++;
        phase = "checking";
        message = "Idle check in progress.";
        notice = "";
        render();
        try {
            const response = await fetch(endpoint, {
                method: "POST",
                headers: { "X-Frinkworks-Dev": "restart" },
                signal: AbortSignal.timeout(10000),
            });
            const data = await response.json();
            if (!response.ok) notice = data.message;
            receive(data);
        } catch {
            phase = "error";
            notice =
                "The supervisor did not answer. See the terminal output before another attempt.";
        } finally {
            submitting = false;
            render();
            const status = button
                .closest("[data-supervised-development]")
                ?.querySelector<HTMLElement>("[data-supervised-status]");
            if (status?.getClientRects().length)
                status.scrollIntoView({ block: "nearest" });
        }
    });
    window.addEventListener("pageshow", () => {
        const lifetime = new AbortController();
        let timer: number;
        const poll = async () => {
            const version = requestVersion;
            try {
                if (submitting) return;
                const response = await fetch(endpoint, {
                    cache: "no-store",
                    signal: AbortSignal.any([
                        lifetime.signal,
                        AbortSignal.timeout(5000),
                    ]),
                });
                if (!response.ok)
                    throw new Error("The supervisor request failed.");
                const data = await response.json();
                // A status request from before a click cannot overwrite that command's result.
                if (
                    !lifetime.signal.aborted &&
                    version === requestVersion &&
                    !submitting
                )
                    receive(data);
            } catch {
                if (
                    !lifetime.signal.aborted &&
                    version === requestVersion &&
                    !submitting
                ) {
                    phase = "error";
                    message =
                        "The supervisor is unavailable. See the terminal output.";
                    render();
                }
            } finally {
                if (!lifetime.signal.aborted && !reloading)
                    timer = window.setTimeout(poll, 1500);
            }
        };
        window.addEventListener(
            "pagehide",
            () => {
                lifetime.abort();
                window.clearTimeout(timer);
            },
            { once: true },
        );
        void poll();
    });
}

if (import.meta.env.MODE === "development") {
    enableLiveReload();
} else if (import.meta.env.MODE === "supervised") {
    enableSupervisedDevelopment();
}
