import { useCallback, useEffect, useState } from "react";
import type { Window as TauriWindow } from "@tauri-apps/api/window";
import { Copy, Minus, Square, X } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { isNative } from "@/lib/native";
import { useCustomTitleBar } from "@/lib/window-chrome";

/**
 * Window controls for the custom title bar (#111).
 *
 * Minimize / maximize-restore / close mirror the native buttons, including the
 * keyboard order (Tab reaches minimize, then maximize, then close). Close goes
 * through `Window::close()`, so the Rust `CloseRequested` handler still decides
 * between hiding to the tray and quitting — the custom bar never bypasses it.
 */

let currentWindowPromise: Promise<TauriWindow> | null = null;

/** `@tauri-apps/api/window` is loaded once, on first use. */
function currentWindow(): Promise<TauriWindow> {
  currentWindowPromise ??= import("@tauri-apps/api/window").then((module) => module.getCurrentWindow());
  return currentWindowPromise;
}

export interface WindowChrome {
  /** Ludex draws the title bar: draggable areas and window controls. */
  custom: boolean;
  /** The window is fullscreen; the title bar and controls step aside. */
  fullscreen: boolean;
  maximized: boolean;
  minimize: () => void;
  toggleMaximize: () => void;
  close: () => void;
}

export function useWindowChrome(): WindowChrome {
  const custom = useCustomTitleBar();
  const nativeShell = isNative();
  const [maximized, setMaximized] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);

  useEffect(() => {
    if (!nativeShell) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let timer: number | undefined;

    const handle = currentWindow().catch(() => null);

    const sync = () => {
      void handle
        .then((appWindow) => (appWindow ? Promise.all([appWindow.isMaximized(), appWindow.isFullscreen()]) : null))
        .then((state) => {
          if (disposed || !state) return;
          setMaximized(state[0]);
          setFullscreen(state[1]);
        })
        .catch(() => {
          /* a plain browser or a locked-down webview: keep the static chrome */
        });
    };

    sync();
    void handle
      .then((appWindow) =>
        appWindow
          ? appWindow.onResized(() => {
              // Dragging a window fires dozens of resize events: read the state
              // once the gesture settles instead of on every frame.
              if (timer) window.clearTimeout(timer);
              timer = window.setTimeout(sync, 120);
            })
          : null,
      )
      .then((off) => {
        if (!off) return;
        if (disposed) off();
        else unlisten = off;
      })
      .catch(() => {});

    return () => {
      disposed = true;
      if (timer) window.clearTimeout(timer);
      unlisten?.();
    };
  }, [nativeShell]);

  const minimize = useCallback(() => {
    void currentWindow()
      .then((appWindow) => appWindow.minimize())
      .catch((error: unknown) => console.warn("Could not minimize the window", error));
  }, []);

  const toggleMaximize = useCallback(() => {
    void currentWindow()
      .then((appWindow) => appWindow.toggleMaximize())
      .catch((error: unknown) => console.warn("Could not toggle maximize", error));
  }, []);

  const close = useCallback(() => {
    void currentWindow()
      .then((appWindow) => appWindow.close())
      .catch((error: unknown) => console.warn("Could not close the window", error));
  }, []);

  return { custom, fullscreen, maximized, minimize, toggleMaximize, close };
}

/**
 * Window glyphs: the app's lucide set at title-bar weight, so the four marks
 * keep the same optical size. `Copy` is the two overlapping squares that mean
 * "restore" on every desktop.
 */
function Glyph({ kind }: { kind: "minimize" | "maximize" | "restore" | "close" }) {
  const className = "size-3.5";
  if (kind === "minimize") return <Minus className={className} strokeWidth={1.6} aria-hidden="true" />;
  if (kind === "maximize") return <Square className={className} strokeWidth={1.6} aria-hidden="true" />;
  if (kind === "restore") return <Copy className={className} strokeWidth={1.6} aria-hidden="true" />;
  return <X className={className} strokeWidth={1.6} aria-hidden="true" />;
}

/**
 * The three controls, flush against each other and separated from the rest of
 * the bar by a hairline. `height="bar"` makes them fill a title bar edge to
 * edge; `"compact"` keeps a 44 px square for screens that only float them.
 */
export function WindowControls({
  chrome,
  height = "compact",
  className,
}: {
  chrome: WindowChrome;
  height?: "bar" | "compact";
  className?: string;
}) {
  if (!chrome.custom) return null;

  // No two buttons share a hover colour: Tailwind resolves `hover:` utilities by
  // stylesheet order, so a shared `hover:text-ink` would beat `hover:text-danger`.
  const base = cn(
    "grid place-items-center text-ink-2 transition-colors duration-[120ms] ease-[var(--ease)]",
    // The outline would be clipped by the window edge; keep it inside.
    "focus-visible:outline-offset-[-3px]",
    height === "bar" ? "h-full w-11" : "h-11 w-11",
  );
  const control = cn(base, "hover:bg-surface-2 hover:text-ink active:bg-surface");
  const destructive = cn(base, "hover:bg-danger/20 hover:text-danger active:bg-danger/30");

  return (
    <div
      role="group"
      aria-label={t("Window controls")}
      // Interactive or not, nothing inside this cluster starts a window drag.
      data-tauri-drag-region="false"
      className={cn("flex shrink-0 items-stretch border-s border-line", className)}
    >
      <button type="button" className={control} aria-label={t("Minimize window")} title={t("Minimize window")} onClick={chrome.minimize}>
        <Glyph kind="minimize" />
      </button>
      <button
        type="button"
        className={control}
        aria-label={chrome.maximized ? t("Restore window") : t("Maximize window")}
        title={chrome.maximized ? t("Restore window") : t("Maximize window")}
        onClick={chrome.toggleMaximize}
      >
        <Glyph kind={chrome.maximized ? "restore" : "maximize"} />
      </button>
      <button
        type="button"
        className={destructive}
        aria-label={t("Close window")}
        title={t("Close window")}
        onClick={chrome.close}
      >
        <Glyph kind="close" />
      </button>
    </div>
  );
}

/**
 * Corner chrome for the screens that do not render the AppShell top bar (boot,
 * login, setup): the wordmark is already on screen there, so the cluster only
 * adds the controls and floats over the empty top-right corner.
 */
export function FloatingWindowControls() {
  const chrome = useWindowChrome();
  if (!chrome.custom || chrome.fullscreen) return null;
  return (
    <div className="fixed end-0 top-0 z-50 bg-bg">
      <WindowControls chrome={chrome} />
    </div>
  );
}
