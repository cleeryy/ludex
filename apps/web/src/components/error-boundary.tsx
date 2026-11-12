import { Component, type ErrorInfo, type ReactNode } from "react";
import { t } from "@/lib/i18n";
import { isNative, native } from "@/lib/native";

/**
 * Last-resort error boundary (#171).
 *
 * A render crash is recorded locally on the desktop (redacted in Rust) and
 * shown as a readable page instead of a blank webview. Recording never uploads
 * anything: the user still has to open Settings → Help and about, look at the
 * preview and press Send.
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    void native.diagnosticsRecordError(
      "web",
      `${error.name}: ${error.message}`,
      info.componentStack ?? error.stack ?? null,
    );
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="grid min-h-dvh place-items-center bg-bg px-6 py-10">
        <div className="w-full max-w-[520px]">
          <h1 className="text-[28px] font-semibold leading-tight tracking-[-0.03em] text-ink">
            {t("Something went wrong on this screen")}
          </h1>
          <p className="mt-3 text-[13.5px] leading-relaxed text-ink-2">
            {t("The page hit an unexpected error. Your library and saves are untouched.")}
          </p>
          <pre className="mt-4 max-h-40 overflow-auto whitespace-pre-wrap rounded-[3px] border border-line bg-surface p-3 font-mono text-[11px] text-ink-3">
            {error.message}
          </pre>
          {isNative() && (
            <p className="mt-3 text-[12px] leading-relaxed text-ink-3">
              {t("A redacted report was saved locally. Review and send it from Settings → Help and about if you want to help fix this.")}
            </p>
          )}
          <div className="mt-6 flex flex-wrap gap-2">
            <button
              type="button"
              className="h-9 rounded-[2px] bg-signal px-4 text-[12.5px] font-medium text-bg"
              onClick={() => window.location.reload()}
            >
              {t("Reload")}
            </button>
            <button
              type="button"
              className="h-9 rounded-[2px] border border-line px-4 text-[12.5px] text-ink"
              onClick={() => {
                window.location.hash = "#/settings?section=help";
                window.location.reload();
              }}
            >
              {t("Open diagnostics settings")}
            </button>
          </div>
        </div>
      </div>
    );
  }
}

/**
 * Captures unhandled rejections and window errors that never reach React
 * (#171). Local only; the same consent gate applies before anything is sent.
 */
export function installGlobalErrorCapture(): void {
  if (typeof window === "undefined") return;
  window.addEventListener("unhandledrejection", (event) => {
    const reason = event.reason;
    const message = reason instanceof Error ? reason.message : String(reason ?? "Unhandled rejection");
    void native.diagnosticsRecordError(
      "web",
      `unhandled rejection: ${message}`,
      reason instanceof Error ? reason.stack ?? null : null,
    );
  });
  window.addEventListener("error", (event) => {
    if (!event.message) return;
    void native.diagnosticsRecordError("web", `window error: ${event.message}`, event.error?.stack ?? null);
  });
}
