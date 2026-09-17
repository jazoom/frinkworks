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

export function startApp(): void {
    const bound = bindTransportFeedback(document);

    if (import.meta.env.DEV) {
        listenForDiagnostics((detail) => {
            console.error("Hypergraft diagnostic", detail.reason);
        });
    }

    startHypergraft({
        feedback: bound.feedback,
        enterEffects: {
            message: {
                keyframes: [
                    { opacity: 0.2, transform: "translateY(10px)" },
                    { opacity: 1, transform: "none" },
                ],
                timing: {
                    duration: 240,
                    easing: "cubic-bezier(0.16, 1, 0.3, 1)",
                },
                reducedMotion: {
                    keyframes: [{ opacity: 0.65 }, { opacity: 1 }],
                    timing: {
                        duration: 170,
                        easing: "cubic-bezier(0.16, 1, 0.3, 1)",
                    },
                },
            },
        },
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
            transcript: initTranscript,
            "workflow-editor": initWorkflowEditor,
            workspace: initWorkspace,
        },
    });
}
