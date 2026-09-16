import type { IslandInstance, IslandMountContext } from "hypergraft/browser";

export function initConversation(
    root: HTMLElement,
    { signal }: IslandMountContext,
): IslandInstance {
    function modelForm() {
        return (
            root.querySelector<HTMLFormElement>(
                "#conversation-settings-form",
            ) ?? root.querySelector<HTMLFormElement>("#conversation-composer")
        );
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
            if (
                !draftSettings &&
                ["provider", "model", "thinking"].includes(name)
            )
                continue;
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
                syncConversation();
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
                    if (
                        name === "revision" &&
                        context.cause === "patch" &&
                        context.detail.form.id === "conversation-model-form"
                    )
                        continue;
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
                if (label && draftSettings)
                    label.textContent =
                        unsavedSettings.get("model")?.value ?? "";
                if (
                    context.cause === "patch" &&
                    context.detail.form.id === "conversation-model-form"
                )
                    retainSettings();
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
