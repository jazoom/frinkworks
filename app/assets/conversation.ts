import type { IslandInstance, IslandMountContext } from "hypergraft/browser";

export function initConversation(
    root: HTMLElement,
    { signal }: IslandMountContext,
): IslandInstance {
    function modelForm() {
        return root.querySelector<HTMLInputElement>("#conversation-model")
            ?.form;
    }

    let draftSettings = !!root.querySelector('[data-conversation-state="new"]');
    const settingsNames = [
        "provider",
        "model",
        "thinking",
        "instructions",
        "tool_list",
        "tool_read",
        "tool_write",
        "tool_run",
        "environment",
        "location",
        "host_approval",
        "directory_access",
        "network",
        "network_domains",
        "preset",
    ];
    let unsavedSettings:
        | Map<string, { value: string; checked: boolean; disabled: boolean }>
        | undefined;
    function syncEnableTools() {
        const toggle = root.querySelector<HTMLInputElement>(
            "[data-enable-tools]",
        );
        const tools = Array.from(
            root.querySelectorAll<HTMLInputElement>("[data-tool-field]"),
        );
        if (!toggle || tools.length === 0) return;
        const checked = tools.filter((tool) => tool.checked).length;
        toggle.checked = checked === tools.length;
        toggle.indeterminate = checked > 0 && checked < tools.length;
    }

    function cancelSettings(trigger: HTMLElement) {
        const panel = trigger.closest<HTMLElement>("#conversation-settings");
        if (!panel) return;
        if (trigger instanceof HTMLAnchorElement) {
            if (panel.matches(":popover-open")) panel.hidePopover();
            return;
        }
        panel
            .querySelectorAll<
                HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement
            >("input, select, textarea")
            .forEach((control) => {
                if (
                    control instanceof HTMLInputElement &&
                    (control.type === "checkbox" || control.type === "radio")
                )
                    control.checked = control.defaultChecked;
                else if (control instanceof HTMLSelectElement) {
                    for (const option of control.options)
                        option.selected = option.defaultSelected;
                } else control.value = control.defaultValue;
            });
        unsavedSettings = undefined;
        const model = panel.querySelector<HTMLInputElement>(
            "#conversation-model",
        );
        const label = panel.querySelector("#conversation-model-value");
        if (model && label)
            label.textContent = model.value || "No models available";
        const search = panel.querySelector<HTMLInputElement>(
            "#conversation-model-search",
        );
        if (search) search.value = "";
        panel.querySelector("#conversation-model-results")?.replaceChildren();
        setModelExpanded(false);
        syncEnableTools();
        syncConversation();
        const host =
            panel.querySelector<HTMLInputElement>(
                'input[name="location"]:checked',
            )?.value === "host";
        panel
            .querySelectorAll<HTMLElement>("[data-execution-sandbox-settings]")
            .forEach((section) => {
                section.hidden = host;
            });
        panel
            .querySelectorAll<HTMLElement>("[data-execution-host-policy]")
            .forEach((section) => {
                section.hidden = !host;
            });
        panel
            .querySelectorAll<HTMLElement>(".segmented label")
            .forEach((label) => {
                const input = label.querySelector<HTMLInputElement>(
                    'input[name="location"]',
                );
                if (input) label.classList.toggle("selected", input.checked);
            });
        const networkSelect = panel.querySelector<HTMLSelectElement>(
            "[data-network-select]",
        );
        const domains = panel.querySelector<HTMLElement>(
            "[data-network-domains]",
        );
        if (networkSelect && domains)
            domains.hidden = networkSelect.value !== "restricted";
        const environment = panel.querySelector<HTMLSelectElement>(
            "#conversation-environment",
        );
        if (environment)
            panel
                .querySelectorAll<HTMLElement>("[data-environment-problem]")
                .forEach((problem) => {
                    problem.hidden =
                        problem.dataset.environmentProblem !==
                        environment.value;
                });
        if (panel.matches(":popover-open")) panel.hidePopover();
    }

    function retainSettings() {
        const form = modelForm();
        if (!form) return;
        unsavedSettings = new Map();
        for (const name of [...settingsNames, "revision"]) {
            const field = form.elements.namedItem(name);
            if (field instanceof RadioNodeList) {
                unsavedSettings.set(name, {
                    value: field.value,
                    checked: false,
                    disabled: false,
                });
                continue;
            }
            if (
                field instanceof HTMLInputElement ||
                field instanceof HTMLSelectElement ||
                field instanceof HTMLTextAreaElement
            )
                unsavedSettings.set(name, {
                    value: field.value,
                    checked: field instanceof HTMLInputElement && field.checked,
                    disabled: field.disabled,
                });
        }
    }
    const choices = new Map<string, { model: string; thinking: string }>();
    function rememberModel() {
        const source = modelForm();
        if (!source) return;
        const provider = source.elements.namedItem(
            "provider",
        ) as HTMLSelectElement;
        choices.set(provider.value, {
            model: (source.elements.namedItem("model") as HTMLInputElement)
                .value,
            thinking: (
                source.elements.namedItem("thinking") as HTMLSelectElement
            ).value,
        });
    }

    function syncConversation() {
        // Saved summaries describe the authoritative configuration, not unsubmitted choices.
        if (!root.querySelector('[data-conversation-state="new"]')) return;
        const form = root.querySelector<HTMLFormElement>(
            "#conversation-composer",
        );
        if (!form) return;
        const field = (name: string) =>
            form.elements.namedItem(name) as
                HTMLInputElement | HTMLSelectElement | null;
        const model = root.querySelector("[data-conversation-model-summary]");
        if (model) {
            model.textContent = field("model")?.value || "Choose a model";
        }
        const networkSummary = root.querySelector(
            "[data-conversation-network-summary]",
        );
        if (networkSummary) {
            const network = form.elements.namedItem("network");
            const value =
                network instanceof RadioNodeList
                    ? network.value
                    : network instanceof HTMLSelectElement
                      ? network.value
                      : "none";
            networkSummary.textContent =
                value === "restricted"
                    ? "Restricted domains"
                    : value === "public"
                      ? "Public internet"
                      : "Off";
        }
        const environment = field("environment") as HTMLSelectElement | null;
        const environmentSummary = root.querySelector(
            "[data-conversation-environment-summary]",
        );
        if (environmentSummary && environment) {
            environmentSummary.textContent =
                environment.selectedOptions[0]?.dataset.environmentName ||
                "Choose environment";
        }
        const project = field("project") as HTMLSelectElement | null;
        const context = root.querySelector<HTMLElement>(
            "[data-conversation-project-summary]",
        );
        if (context && project) {
            context.hidden = !project.value;
            context.textContent = project.value
                ? `${project.selectedOptions[0].text} · No file access`
                : "";
        }
    }

    function updateModelOptions(changed: HTMLSelectElement | HTMLInputElement) {
        const form = modelForm();
        const source = root.querySelector<HTMLElement>(
            "[data-conversation-model-catalogue]",
        );
        if (!form || !source) return;
        type Model = {
            id: string;
            default_effort: string;
            efforts: { value: string; label: string }[];
        };
        const catalogue: Record<string, Model[]> = JSON.parse(
            source.dataset.conversationModelCatalogue ?? "{}",
        );
        const provider = form.elements.namedItem(
            "provider",
        ) as HTMLSelectElement;
        const model = form.elements.namedItem("model") as HTMLInputElement;
        const thinking = form.elements.namedItem(
            "thinking",
        ) as HTMLSelectElement;
        if (provider.disabled) return;
        const models = catalogue[provider.value] ?? [];
        const search = root.querySelector<HTMLInputElement>(
            "#conversation-model-search",
        );
        const status = root.querySelector("#conversation-model-search-status");
        if (changed.name === "provider") {
            if (search) search.value = "";
            setModelExpanded(false);
            const preferred =
                choices.get(provider.value)?.model ??
                provider.selectedOptions[0]?.dataset.defaultModel;
            model.value =
                models.find((item) => item.id === preferred)?.id ??
                models[0]?.id ??
                "";
        }
        const selected = models.find((item) => item.id === model.value);
        const query = search?.value.trim().toLocaleLowerCase() ?? "";
        const matches = models.filter((item) =>
            item.id.toLocaleLowerCase().includes(query),
        );
        root.querySelector("#conversation-model-results")?.replaceChildren(
            ...matches.map((item) => {
                const button = document.createElement("button");
                button.type = "button";
                button.dataset.conversationModelValue = item.id;
                button.textContent = item.id;
                button.className =
                    "btn btn-ghost h-auto min-h-11 w-full min-w-0 justify-start text-left break-all font-normal aria-pressed:bg-base-300";
                button.ariaPressed = String(item === selected);
                return button;
            }),
        );
        const toggle = root.querySelector<HTMLButtonElement>(
            "#conversation-model-toggle",
        );
        if (toggle) toggle.disabled = models.length === 0;
        const label = root.querySelector("#conversation-model-value");
        if (label) label.textContent = model.value || "No models available";
        if (status) {
            status.textContent = matches.length
                ? `${matches.length} ${matches.length === 1 ? "model" : "models"}.`
                : "No matching models. Clear the search to see all models.";
        }
        // Search changes only the results, never the submitted model or effort.
        if (changed === search) return;
        const efforts = selected?.efforts ?? [];
        thinking.replaceChildren(
            ...(efforts.length
                ? efforts.map(
                      (effort) => new Option(effort.label, effort.value),
                  )
                : [new Option("Not available", "")]),
        );
        thinking.disabled = efforts.length === 0;
        const remembered = choices.get(provider.value);
        thinking.value =
            remembered?.model === model.value &&
            efforts.some((effort) => effort.value === remembered.thinking)
                ? remembered.thinking
                : (selected?.default_effort ?? "");
    }

    function setModelExpanded(expanded: boolean, restoreFocus = false) {
        const toggle = root.querySelector<HTMLButtonElement>(
            "#conversation-model-toggle",
        );
        const options = root.querySelector<HTMLElement>(
            "#conversation-model-options",
        );
        const search = root.querySelector<HTMLInputElement>(
            "#conversation-model-search",
        );
        if (!toggle || !options || !search || (expanded && toggle.disabled))
            return;
        toggle.ariaExpanded = String(expanded);
        options.hidden = !expanded;
        if (expanded) {
            search.value = "";
            updateModelOptions(search);
            search.focus({ preventScroll: true });
        } else if (restoreFocus) {
            toggle.focus();
        }
    }

    root.addEventListener(
        "click",
        (event) => {
            if (!(event.target instanceof Element)) return;
            const cancel = event.target.closest<HTMLElement>(
                "[data-settings-cancel]",
            );
            if (cancel) {
                cancelSettings(cancel);
                return;
            }
            const saveToggle = event.target.closest<HTMLButtonElement>(
                "[data-preset-save-toggle]",
            );
            if (saveToggle) {
                const panel = root.querySelector<HTMLElement>(
                    "#conversation-preset-save",
                );
                if (panel) {
                    panel.hidden = false;
                    saveToggle.setAttribute("aria-expanded", "true");
                    panel
                        .querySelector<HTMLElement>("#conversation-preset-name")
                        ?.focus();
                }
                return;
            }
            const saveCancel = event.target.closest<HTMLElement>(
                "[data-preset-save-cancel]",
            );
            if (saveCancel) {
                const panel = root.querySelector<HTMLElement>(
                    "#conversation-preset-save",
                );
                if (panel) panel.hidden = true;
                const toggle = root.querySelector<HTMLButtonElement>(
                    "[data-preset-save-toggle]",
                );
                toggle?.setAttribute("aria-expanded", "false");
                toggle?.focus();
                return;
            }
            const previewCancel = event.target.closest<HTMLElement>(
                "[data-preset-cancel]",
            );
            if (previewCancel) {
                // Preview grants no authority, so Cancel only hides the
                // replacement description and keeps the effective setup.
                previewCancel.closest("[data-preset-preview]")?.remove();
                const fallback =
                    root.querySelector<HTMLElement>(
                        "#settings-presets .choice-row",
                    ) ??
                    root.querySelector<HTMLElement>(
                        "[data-preset-save-toggle]",
                    );
                fallback?.focus();
                return;
            }
            const toggle = event.target.closest<HTMLButtonElement>(
                "#conversation-model-toggle",
            );
            if (toggle) {
                setModelExpanded(toggle.ariaExpanded !== "true");
                return;
            }
            const option = event.target.closest<HTMLButtonElement>(
                "[data-conversation-model-value]",
            );
            const model = root.querySelector<HTMLInputElement>(
                "#conversation-model",
            );
            if (
                option &&
                model &&
                !root.querySelector<HTMLSelectElement>('[name="provider"]')
                    ?.disabled
            ) {
                if (model.value !== option.dataset.conversationModelValue) {
                    model.value = option.dataset.conversationModelValue ?? "";
                    updateModelOptions(model);
                    syncConversation();
                    rememberModel();
                    retainSettings();
                }
                setModelExpanded(false, true);
            } else if (!event.target.closest("#conversation-model-picker")) {
                setModelExpanded(false);
            }
        },
        { signal },
    );

    root.addEventListener(
        "focusin",
        (event) => {
            if (
                event.target instanceof Element &&
                !event.target.closest("#conversation-model-picker")
            ) {
                setModelExpanded(false);
            }
        },
        { signal },
    );

    root.addEventListener(
        "keydown",
        (event) => {
            if (
                !(event.target instanceof Element) ||
                !event.target.closest("#conversation-model-picker")
            )
                return;
            const search = root.querySelector<HTMLInputElement>(
                "#conversation-model-search",
            );
            const toggle = root.querySelector<HTMLButtonElement>(
                "#conversation-model-toggle",
            );
            if (event.key === "Escape" && toggle?.ariaExpanded === "true") {
                event.preventDefault();
                event.stopPropagation();
                setModelExpanded(false, true);
                return;
            }
            if (
                event.target === toggle &&
                ["ArrowDown", "ArrowUp"].includes(event.key)
            ) {
                event.preventDefault();
                setModelExpanded(true);
                return;
            }
            const options = Array.from(
                root.querySelectorAll<HTMLButtonElement>(
                    "[data-conversation-model-value]",
                ),
            );
            if (event.target === search && event.key === "Enter") {
                event.preventDefault();
                const query = search.value.trim().toLocaleLowerCase();
                const exact = options.find(
                    (option) =>
                        option.dataset.conversationModelValue?.toLocaleLowerCase() ===
                        query,
                );
                (
                    exact ?? (options.length === 1 ? options[0] : undefined)
                )?.click();
                return;
            }
            const index = options.indexOf(event.target as HTMLButtonElement);
            if (event.target !== search && index < 0) return;
            let next: HTMLElement | undefined;
            if (event.key === "ArrowDown")
                next = options[(index + 1) % options.length];
            else if (event.key === "ArrowUp")
                next =
                    index === 0
                        ? (search ?? undefined)
                        : options.at(index < 0 ? -1 : index - 1);
            else if (event.target !== search && event.key === "Home")
                next = options[0];
            else if (event.target !== search && event.key === "End")
                next = options.at(-1);
            else return;
            event.preventDefault();
            next?.focus();
        },
        { signal },
    );

    root.addEventListener(
        "input",
        (event) => {
            const field = event.target;
            if (
                (field instanceof HTMLInputElement ||
                    field instanceof HTMLSelectElement ||
                    field instanceof HTMLTextAreaElement) &&
                settingsNames.includes(field.name)
            )
                retainSettings();
            if (
                event.target instanceof HTMLInputElement &&
                event.target.id === "conversation-model-search"
            ) {
                updateModelOptions(event.target);
                return;
            }
            if (
                event.target instanceof HTMLInputElement &&
                event.target.form?.id === "conversation-composer"
            ) {
                syncConversation();
            }
        },
        { signal },
    );

    root.addEventListener(
        "change",
        (event) => {
            if (
                event.target instanceof HTMLInputElement &&
                event.target.matches("[data-enable-tools]")
            ) {
                const tools = Array.from(
                    root.querySelectorAll<HTMLInputElement>(
                        "[data-tool-field]",
                    ),
                );
                for (const tool of tools) {
                    if (tool.checked !== event.target.checked) {
                        tool.checked = event.target.checked;
                        tool.dispatchEvent(
                            new Event("input", { bubbles: true }),
                        );
                    }
                }
                event.target.indeterminate = false;
                return;
            }
            if (event.target instanceof HTMLSelectElement) {
                if (
                    event.target.form === modelForm() &&
                    event.target.name === "provider"
                )
                    updateModelOptions(event.target);
                syncConversation();
                if (
                    event.target.form === modelForm() &&
                    ["provider", "thinking"].includes(event.target.name)
                )
                    rememberModel();
                if (event.target.form === modelForm()) retainSettings();
            }
        },
        { signal },
    );

    syncConversation();
    syncEnableTools();
    return {
        reconcile(context) {
            if (
                context.cause !== "location" &&
                (!("targetIds" in context.detail) ||
                    !context.detail.targetIds.some(
                        (id) => id === root.id || id === "chat-main",
                    ))
            )
                return;
            const settingsResponse =
                (context.cause === "patch" &&
                    [
                        "conversation-settings-form",
                        "conversation-preset-form",
                        "conversation-preset-save-form",
                        "conversation-preset-apply-form",
                        "conversation-execution-switch-form",
                    ].includes(context.detail.form.id)) ||
                (context.cause === "patch" &&
                    context.detail.form.id === "conversation-composer" &&
                    draftSettings);
            if (context.cause === "location" || settingsResponse) {
                unsavedSettings = undefined;
            } else if (unsavedSettings) {
                const form = modelForm();
                for (const [name, saved] of unsavedSettings) {
                    const field = form?.elements.namedItem(name);
                    if (field instanceof RadioNodeList) {
                        for (const radio of field) {
                            if (radio instanceof HTMLInputElement)
                                radio.checked = radio.value === saved.value;
                        }
                        continue;
                    }
                    if (!(
                        field instanceof HTMLInputElement ||
                        field instanceof HTMLSelectElement ||
                        field instanceof HTMLTextAreaElement
                    ))
                        continue;
                    if (
                        field instanceof HTMLSelectElement &&
                        !Array.from(field.options).some(
                            (option) => option.value === saved.value,
                        )
                    )
                        field.add(new Option(saved.value, saved.value));
                    field.value = saved.value;
                    if (field instanceof HTMLInputElement)
                        field.checked = saved.checked;
                    if (
                        name === "thinking" &&
                        !root.querySelector<HTMLSelectElement>(
                            '[name="provider"]',
                        )?.disabled
                    )
                        field.disabled = saved.disabled;
                }
                const label = root.querySelector("#conversation-model-value");
                if (label)
                    label.textContent =
                        unsavedSettings.get("model")?.value ?? "";
            }
            const access = root.querySelector<HTMLInputElement>(
                "#execution-directory-access",
            )?.value;
            if (access) {
                try {
                    const values = new Map<string, string>(JSON.parse(access));
                    root.querySelectorAll<HTMLSelectElement>(
                        "[data-execution-directory]",
                    ).forEach((select) => {
                        const value = values.get(
                            select.dataset.executionDirectory ?? "",
                        );
                        if (value) select.value = value;
                    });
                } catch {
                    // The server rejects malformed strategy values. The editor keeps its current rows.
                }
            }
            draftSettings = !!root.querySelector(
                '[data-conversation-state="new"]',
            );
            choices.clear();
            setModelExpanded(false);
            const search = root.querySelector<HTMLInputElement>(
                "#conversation-model-search",
            );
            if (search) search.value = "";
            root.querySelector(
                "#conversation-model-results",
            )?.replaceChildren();
            syncConversation();
            syncEnableTools();
            const host =
                root.querySelector<HTMLInputElement>(
                    'input[name="location"]:checked',
                )?.value === "host";
            root.querySelectorAll<HTMLElement>(
                "[data-execution-sandbox-settings]",
            ).forEach((section) => {
                section.hidden = host;
            });
            root.querySelectorAll<HTMLElement>(
                "[data-execution-host-policy]",
            ).forEach((section) => {
                section.hidden = !host;
            });
            root.querySelectorAll<HTMLElement>(".segmented label").forEach(
                (label) => {
                    const input = label.querySelector<HTMLInputElement>(
                        'input[name="location"]',
                    );
                    if (input)
                        label.classList.toggle("selected", input.checked);
                },
            );
            const networkSelect = root.querySelector<HTMLSelectElement>(
                "[data-network-select]",
            );
            const domains = root.querySelector<HTMLElement>(
                "[data-network-domains]",
            );
            if (networkSelect && domains)
                domains.hidden = networkSelect.value !== "restricted";
        },
        destroy() {},
    };
}
