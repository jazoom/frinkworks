import {
    commandBlockReason,
    type IslandInstance,
    type IslandMountContext,
} from "hypergraft/browser";

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
        "tool_edit",
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
    const executionNames = [
        "environment",
        "location",
        "host_approval",
        "directory_access",
        "network",
        "network_domains",
    ];
    const ordinaryNames = [
        "instructions",
        "tool_list",
        "tool_read",
        "tool_edit",
        "tool_write",
        "tool_run",
    ];
    let unsavedSettings:
        | Map<string, { value: string; checked: boolean; disabled: boolean }>
        | undefined;
    let submittedSettings: typeof unsavedSettings;
    let ordinarySavePending = false;
    let ordinarySaveQueued = false;
    let ordinaryFailure = "";
    function ordinaryValues() {
        const form = modelForm();
        return JSON.stringify(
            ordinaryNames.map((name) => {
                const field = form?.elements.namedItem(name);
                if (field instanceof HTMLInputElement)
                    return [name, field.checked];
                if (field instanceof HTMLTextAreaElement)
                    return [name, field.value];
                return [name, null];
            }),
        );
    }
    let savedOrdinary = ordinaryValues();
    function syncSettingsStatus() {
        const status = root.querySelector<HTMLElement>(
            "[data-instructions-status]",
        );
        if (!status) return;
        const invalid =
            root.querySelector<HTMLTextAreaElement>(
                "#conversation-instructions",
            )?.validity.valid === false;
        const text = draftSettings
            ? invalid
                ? "Draft retained. The instructions need correction."
                : "Draft · saved with your first message."
            : ordinaryFailure ||
              (ordinarySavePending
                  ? "Save in progress"
                  : invalid || ordinaryValues() !== savedOrdinary
                    ? "Unsaved changes"
                    : "Saved");
        if (status.textContent !== text) status.textContent = text;
    }
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

    function applyField(
        form: HTMLFormElement,
        name: string,
        saved: { value: string; checked: boolean; disabled: boolean },
    ) {
        const field = form.elements.namedItem(name);
        if (field instanceof RadioNodeList) {
            for (const radio of field) {
                if (radio instanceof HTMLInputElement)
                    radio.checked = radio.value === saved.value;
            }
            return;
        }
        if (!(
            field instanceof HTMLInputElement ||
            field instanceof HTMLSelectElement ||
            field instanceof HTMLTextAreaElement
        ))
            return;
        if (
            field instanceof HTMLSelectElement &&
            !Array.from(field.options).some(
                (option) => option.value === saved.value,
            )
        )
            field.add(new Option(saved.value, saved.value));
        field.value = saved.value;
        if (field instanceof HTMLInputElement) field.checked = saved.checked;
        if (
            name === "thinking" &&
            !root.querySelector<HTMLSelectElement>('[name="provider"]')
                ?.disabled
        )
            field.disabled = saved.disabled;
    }

    function applyDefaults(form: HTMLFormElement, names: string[]) {
        for (const name of names) {
            const field = form.elements.namedItem(name);
            if (field instanceof RadioNodeList) {
                for (const radio of field) {
                    if (radio instanceof HTMLInputElement)
                        radio.checked = radio.defaultChecked;
                }
                continue;
            }
            if (
                field instanceof HTMLInputElement &&
                (field.type === "checkbox" || field.type === "radio")
            )
                field.checked = field.defaultChecked;
            else if (field instanceof HTMLSelectElement) {
                for (const option of field.options)
                    option.selected = option.defaultSelected;
            } else if (
                field instanceof HTMLInputElement ||
                field instanceof HTMLTextAreaElement
            )
                field.value = field.defaultValue;
        }
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

    function queueOrdinarySave() {
        if (draftSettings) return;
        const form = root.querySelector<HTMLFormElement>(
            "#conversation-settings-form",
        );
        if (!form || typeof form.requestSubmit !== "function") return;
        if (
            !ordinaryFailure &&
            ordinaryValues() === savedOrdinary &&
            !ordinarySavePending
        )
            return;
        if (ordinarySavePending || commandBlockReason()) {
            ordinarySaveQueued = true;
            return;
        }
        retainSettings();
        // Pin execution fields before validation and submission. Requested
        // execution settings cannot enter an ordinary save.
        applyDefaults(form, executionNames);
        if (form.checkValidity()) {
            submittedSettings = new Map(unsavedSettings);
            ordinaryFailure = "";
            ordinarySavePending = true;
            form.requestSubmit();
        }
        if (unsavedSettings) {
            for (const name of executionNames) {
                const saved = unsavedSettings.get(name);
                if (saved) applyField(form, name, saved);
            }
        }
        syncSettingsStatus();
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
        const networkSummaries = root.querySelectorAll(
            "[data-conversation-network-summary]",
        );
        if (networkSummaries.length) {
            const network = form.elements.namedItem("network");
            const value =
                network instanceof RadioNodeList
                    ? network.value
                    : network instanceof HTMLSelectElement
                      ? network.value
                      : "none";
            for (const summary of networkSummaries)
                summary.textContent =
                    value === "restricted"
                        ? "Restricted domains"
                        : value === "public"
                          ? "Public internet"
                          : "Off";
        }
        const environment = field("environment") as HTMLSelectElement | null;
        if (environment)
            for (const summary of root.querySelectorAll(
                "[data-conversation-environment-summary]",
            ))
                summary.textContent =
                    environment.selectedOptions[0]?.dataset.environmentName ||
                    "Choose environment";
    }

    root.addEventListener(
        "input",
        (event) => {
            const field = event.target;
            if (
                (field instanceof HTMLInputElement ||
                    field instanceof HTMLSelectElement ||
                    field instanceof HTMLTextAreaElement) &&
                settingsNames.includes(field.name)
            ) {
                retainSettings();
                syncSettingsStatus();
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
                retainSettings();
                queueOrdinarySave();
                return;
            }
            if (
                event.target instanceof HTMLInputElement &&
                event.target.matches("[data-tool-field]")
            )
                syncEnableTools();
            const field = event.target;
            if (
                (field instanceof HTMLInputElement ||
                    field instanceof HTMLSelectElement) &&
                ordinaryNames.includes(field.name)
            ) {
                if (field instanceof HTMLSelectElement) syncConversation();
                if (field.form === modelForm()) {
                    retainSettings();
                    queueOrdinarySave();
                }
            } else if (field instanceof HTMLSelectElement) {
                syncConversation();
                if (field.form === modelForm()) retainSettings();
            }
        },
        { signal },
    );

    root.addEventListener(
        "submit",
        (event) => {
            if (draftSettings && event.target === modelForm()) {
                retainSettings();
                submittedSettings = new Map(unsavedSettings);
            }
        },
        { signal, capture: true },
    );

    root.addEventListener(
        "focusout",
        (event) => {
            const field = event.target;
            if (
                field instanceof HTMLTextAreaElement &&
                ordinaryNames.includes(field.name) &&
                field.form === modelForm()
            ) {
                retainSettings();
                queueOrdinarySave();
            }
        },
        { signal },
    );

    syncConversation();
    syncEnableTools();
    syncSettingsStatus();
    return {
        reconcile(context) {
            if (
                context.cause === "patch" &&
                context.detail.outcome !== "applied-patch"
            ) {
                if (context.detail.form.id === "conversation-settings-form") {
                    ordinarySavePending = false;
                    ordinarySaveQueued = false;
                    ordinaryFailure =
                        "Save result unknown. Further changes are blocked.";
                    syncSettingsStatus();
                }
                return;
            }
            if (
                context.cause !== "location" &&
                (!("targetIds" in context.detail) ||
                    !context.detail.targetIds.some(
                        (id) => id === root.id || id === "chat-main",
                    ))
            )
                return;
            const ordinaryResponse =
                context.cause === "patch" &&
                context.detail.form.id === "conversation-settings-form";
            if (ordinaryResponse || context.cause === "location") {
                ordinarySavePending = false;
                ordinaryFailure =
                    ordinaryResponse && context.detail.status !== 200
                        ? "Changes are not saved. Resolve the error before another change."
                        : "";
                if (ordinaryFailure) ordinarySaveQueued = false;
                else savedOrdinary = ordinaryValues();
            }
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
            if (context.cause === "location") {
                unsavedSettings = undefined;
                submittedSettings = undefined;
                ordinarySaveQueued = false;
            } else if (
                context.cause === "patch" &&
                context.detail.form.id === "conversation-settings-form" &&
                unsavedSettings
            ) {
                for (const name of [...unsavedSettings.keys()]) {
                    const saved = unsavedSettings.get(name);
                    const sent = submittedSettings?.get(name);
                    const changedAfterSubmit =
                        ordinaryNames.includes(name) &&
                        sent &&
                        (saved?.value !== sent.value ||
                            saved?.checked !== sent.checked);
                    if (
                        !executionNames.includes(name) &&
                        !changedAfterSubmit &&
                        !(ordinaryFailure && ordinaryNames.includes(name))
                    )
                        unsavedSettings.delete(name);
                }
                if (unsavedSettings.size === 0) unsavedSettings = undefined;
            } else if (
                context.cause === "patch" &&
                /\/presets\/(preview|save|apply)$/.test(context.detail.url) &&
                (!draftSettings || context.detail.status !== 200) &&
                !(
                    context.detail.url.endsWith("/presets/apply") &&
                    context.detail.status === 200
                )
            ) {
                // Preview and save cannot discard requested setup. A rejected
                // replacement must also retain edits without an automatic retry.
                ordinarySaveQueued = false;
            } else if (settingsResponse) {
                const laterDraftEdits =
                    draftSettings &&
                    submittedSettings &&
                    context.cause === "patch" &&
                    !context.detail.url.endsWith("/presets/apply");
                unsavedSettings = laterDraftEdits
                    ? new Map(
                          [...(unsavedSettings ?? [])].filter(
                              ([name, saved]) => {
                                  const sent = submittedSettings?.get(name);
                                  return (
                                      ordinaryNames.includes(name) &&
                                      sent &&
                                      (saved.value !== sent.value ||
                                          saved.checked !== sent.checked)
                                  );
                              },
                          ),
                      )
                    : undefined;
                if (
                    context.cause === "patch" &&
                    context.detail.status === 200
                ) {
                    savedOrdinary = ordinaryValues();
                    ordinaryFailure = "";
                }
            }
            if (unsavedSettings) {
                const form = modelForm();
                for (const [name, saved] of unsavedSettings) {
                    if (
                        name === "revision" &&
                        context.cause === "patch" &&
                        [
                            "conversation-model-form",
                            "command-directory-form",
                        ].includes(context.detail.form.id)
                    )
                        continue;
                    if (form) applyField(form, name, saved);
                }
                const label = root.querySelector("#conversation-model-value");
                const model = unsavedSettings.get("model");
                if (label && draftSettings && model)
                    label.textContent = model.value;
                if (
                    context.cause === "patch" &&
                    [
                        "conversation-model-form",
                        "command-directory-form",
                    ].includes(context.detail.form.id)
                )
                    retainSettings();
            }
            const access = root.querySelector<HTMLInputElement>(
                "#execution-directory-access",
            )?.value;
            if (access) {
                try {
                    const values = new Map<string, string>(JSON.parse(access));
                    root.querySelectorAll<HTMLInputElement>(
                        "[data-execution-directory]",
                    ).forEach((radio) => {
                        const value = values.get(
                            radio.dataset.executionDirectory ?? "",
                        );
                        if (value) radio.checked = radio.value === value;
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
            const networkSelect = root.querySelector<HTMLInputElement>(
                "[data-network-select]:checked",
            );
            const domains = root.querySelector<HTMLElement>(
                "[data-network-domains]",
            );
            if (networkSelect && domains)
                domains.hidden = networkSelect.value !== "restricted";
            if (settingsResponse) submittedSettings = undefined;
            if (ordinarySaveQueued && !ordinarySavePending) {
                ordinarySaveQueued = false;
                queueOrdinarySave();
            }
            syncSettingsStatus();
        },
        destroy() {},
    };
}
