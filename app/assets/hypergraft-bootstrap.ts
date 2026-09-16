import {
    bindTransportFeedback,
    listenForDiagnostics,
    startHypergraft,
} from "hypergraft/browser";
import { initAppContext } from "./app-context";
import { initComposer, initShortcutHint } from "./composer";
import { initConnectErrors } from "./connect-errors";
import { initConnectPlan } from "./connect-plan";
import { initDeskSettings } from "./desk-settings";
import { initConversation } from "./conversation";
import { initObserve } from "./observe";
import { initThemeSelector } from "./theme";
import { initThinkingVisibility } from "./thinking-visibility";
import { initTranscript } from "./transcript";
import { initWorkflowEditor } from "./workflow-editor";
import { initWorkspace } from "./workspace";

export function startApp(onMessageAdded: (message: HTMLElement) => void): void {
    const bound = bindTransportFeedback(document);

    if (import.meta.env.DEV) {
        listenForDiagnostics((detail) => {
            console.error("Hypergraft diagnostic", detail.reason);
        });
    }

    startHypergraft({
        feedback: bound.feedback,
        islands: {
            "app-context": initAppContext,
            composer: initComposer,
            "connect-errors": initConnectErrors,
            "connect-plan": initConnectPlan,
            "desk-settings": initDeskSettings,
            conversation: initConversation,
            observe: initObserve,
            "shortcut-hint": initShortcutHint,
            "theme-selector": initThemeSelector,
            "thinking-visibility": initThinkingVisibility,
            transcript: (root) => initTranscript(root, onMessageAdded),
            "workflow-editor": initWorkflowEditor,
            workspace: initWorkspace,
        },
    });
}
