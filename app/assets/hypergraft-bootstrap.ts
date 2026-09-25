import {
    bindLiveFeedback,
    bindNavigationRecovery,
    bindReadFeedback,
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

export function startApp(): () => void {
    const stopDiagnostics = import.meta.env.DEV
        ? listenForDiagnostics((detail) => {
              console.error("Hypergraft diagnostic", detail.reason);
          })
        : () => {};
    const bound = bindTransportFeedback(document);
    const stopLiveFeedback = bindLiveFeedback(document);
    const stopReadFeedback = bindReadFeedback(document);
    const stopNavigationRecovery = bindNavigationRecovery(document);

    const stopRuntime = startHypergraft({
        feedback: bound.feedback,
        // Only catalogue containers opt in. The transcript owns its auto-scroll.
        scrollRestoration: true,
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

    return () => {
        stopLiveFeedback();
        stopRuntime();
        stopReadFeedback();
        stopNavigationRecovery();
        bound.destroy();
        stopDiagnostics();
    };
}
